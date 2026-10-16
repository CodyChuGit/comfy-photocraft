//! `select.*` commands backed by a segmentation model (SAM 3.1 through the generative backend):
//! **Select by Text** ("the dog", "red car", `eye:2`), the ML **Select Subject**, **Select by
//! Point** (the object under canvas points, through the detector's point prompt, with points to
//! exclude and a box to search in), and the **object finder's probe** (`select.probe`: the mask a
//! point would select, kept for the click that follows; nothing is selected).
//!
//! The composite (or the active layer) goes to the backend as an image with the prompt; the
//! model returns one mask per instance. The command turns them into coverage over the canvas,
//! picks one instance or unions them, optionally softens the detector's hard edge with the
//! classical refinement (as Remove Background does), and combines the result with the current
//! selection by the usual `replace|add|subtract|intersect` modes, as one undo step. It is a
//! background job like the other backend commands (progress, Esc cancels). The model is a
//! detector, so a run takes well under a second once the checkpoint is loaded.
//!
//! The backend and its preferences are shared with `generate_cmds` (Preferences › AI
//! Integrations). Nothing here needs a diffusion model.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use photocraft_algo::selection::{self as sel, SelectionMode};
use photocraft_doc::{DocId, Document, LayerId};
use photocraft_genai::{Gray8, Request, Rgba8, Task};
use photocraft_geom::Rect;
use serde_json::{Map, Value, json};

use crate::commands::CommandSpec;
use crate::generate_cmds::{self, Common, JobProgress, bad, fit_pixels, gen_err, opt_bool, opt_num, opt_str, resize_gray8, resize_rgba8, short};
use crate::jobs::JobCtx;
use crate::{EngineError, Result, Session};

pub const BY_TEXT: &str = "select.byText";
pub const SUBJECT_ML: &str = "select.subjectML";
pub const BY_POINT: &str = "select.byPoint";
/// The object finder's hover probe: the mask a click would select, kept for that click.
pub const PROBE: &str = "select.probe";
pub const DEFAULT_SEGMENT_TEMPLATE: &str = "sam3.1/segment";
pub const DEFAULT_POINT_TEMPLATE: &str = "sam3.1/segment-point";
pub const DEFAULT_BOX_TEMPLATE: &str = "sam3.1/segment-box";
/// A detector request is sent at most this large (SAM 3.1 works at about 1 megapixel inside;
/// the masks come back at the request size and are resampled to the canvas).
const MAX_SEGMENT_REQUEST_PIXELS: u64 = 2048 * 1024;
/// A probe is sent smaller still: it runs on every pause of the pointer.
const MAX_PROBE_REQUEST_PIXELS: u64 = 768 * 768;
/// Probes kept for the clicks that follow them.
const PROBES_KEPT: usize = 8;

/// A probe's answer: the union of the masks the detector returned for its points, at the
/// request size, with what it was asked on so a click can tell whether it still applies.
#[derive(Clone, Debug)]
pub struct Probe {
    pub id: u64,
    pub doc: DocId,
    /// The document's history length when it was taken (an edit since makes it stale).
    pub past_len: usize,
    pub points: Vec<[f64; 2]>,
    pub mask: Gray8,
}

/// The last few probes (`Session::select_probes`).
#[derive(Clone, Debug, Default)]
pub struct ProbeCache {
    next: u64,
    probes: VecDeque<Probe>,
}

impl ProbeCache {
    fn push(&mut self, mut p: Probe) -> u64 {
        self.next += 1;
        p.id = self.next;
        self.probes.push_back(p);
        while self.probes.len() > PROBES_KEPT {
            self.probes.pop_front();
        }
        self.next
    }

    pub fn get(&self, id: u64) -> Option<&Probe> {
        self.probes.iter().find(|p| p.id == id)
    }

    pub fn len(&self) -> usize {
        self.probes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.probes.is_empty()
    }
}

fn enabled(s: &Session) -> std::result::Result<(), String> {
    generate_cmds::web_unavailable()?;
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

struct Plan {
    common: Common,
    mode: SelectionMode,
    /// 1-based instance to select; `None` selects every instance.
    instance: Option<usize>,
    /// Kept as `f64` so the JSON the server receives is the number the caller gave.
    threshold: f64,
    all_layers: bool,
    /// Point prompts in canvas pixels (`select.byPoint`); empty for a text prompt.
    points: Vec<[f64; 2]>,
    /// Points the object must not include.
    negatives: Vec<[f64; 2]>,
    /// The canvas rectangle to search in (`select.byPoint` with `box`).
    bbox: Option<Rect>,
    /// Soften the detector's hard mask with the classical edge refinement.
    refine: bool,
    /// Apply a kept probe instead of asking the server (`select.byPoint` with `probe`).
    probe: Option<u64>,
}

/// `[[x, y], …]` on the canvas.
fn parse_points(cmd: &str, p: &Value, key: &str, canvas: Rect) -> Result<Vec<[f64; 2]>> {
    let mut points = Vec::new();
    let Some(list) = p.get(key).filter(|v| !v.is_null()) else { return Ok(points) };
    for pt in list.as_array().ok_or_else(|| bad(cmd, format!("`{key}` must be a list of [x, y]")))? {
        let (x, y) = (pt.get(0).and_then(Value::as_f64), pt.get(1).and_then(Value::as_f64));
        match (x, y) {
            (Some(x), Some(y)) if x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0 && x < f64::from(canvas.width()) && y < f64::from(canvas.height()) => {
                points.push([x, y]);
            }
            _ => return Err(bad(cmd, format!("every point of `{key}` must be an [x, y] inside the {}×{} canvas", canvas.width(), canvas.height()))),
        }
    }
    if points.len() > 64 {
        return Err(bad(cmd, format!("`{key}` takes at most 64 points")));
    }
    Ok(points)
}

/// `box: [x, y, w, h]` on the canvas (a negative size is normalised), clipped to it.
fn parse_box(cmd: &str, p: &Value, canvas: Rect) -> Result<Option<Rect>> {
    let Some(v) = p.get("box").filter(|v| !v.is_null()) else { return Ok(None) };
    let nums: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).filter(|x| x.is_finite()).collect()).unwrap_or_default();
    if nums.len() != 4 {
        return Err(bad(cmd, "`box` must be [x, y, width, height] in canvas pixels"));
    }
    let (x0, x1) = (nums[0].min(nums[0] + nums[2]), nums[0].max(nums[0] + nums[2]));
    let (y0, y1) = (nums[1].min(nums[1] + nums[3]), nums[1].max(nums[1] + nums[3]));
    let r = Rect::new(x0.floor() as i32, y0.floor() as i32, x1.ceil() as i32, y1.ceil() as i32).intersect(&canvas);
    if r.width() < 2 || r.height() < 2 {
        return Err(bad(cmd, "`box` must cover at least 2×2 canvas pixels"));
    }
    Ok(Some(r))
}

fn plan(s: &Session, cmd: &str, p: &Value) -> Result<Plan> {
    let canvas = s.active().ok_or(EngineError::NoDocument)?.doc.bounds();
    let bbox = if cmd == BY_POINT || cmd == PROBE { parse_box(cmd, p, canvas)? } else { None };
    let default_template = match cmd {
        BY_POINT | PROBE if bbox.is_some() => DEFAULT_BOX_TEMPLATE,
        BY_POINT | PROBE => DEFAULT_POINT_TEMPLATE,
        _ => DEFAULT_SEGMENT_TEMPLATE,
    };
    let common = generate_cmds::plan_common(s, cmd, p, default_template, Task::Segment, "")?;
    let points = parse_points(cmd, p, "points", canvas)?;
    let negatives = parse_points(cmd, p, "negativePoints", canvas)?;
    if (cmd == BY_POINT || cmd == PROBE) && points.is_empty() && bbox.is_none() {
        return Err(bad(cmd, "give `points` (or `x` and `y`) or a `box`"));
    }
    let mode = match opt_str(cmd, p, "mode", 20)? {
        None | Some("replace") | Some("new") => SelectionMode::Replace,
        Some("add") => SelectionMode::Add,
        Some("subtract") => SelectionMode::Subtract,
        Some("intersect") => SelectionMode::Intersect,
        Some(other) => return Err(bad(cmd, format!("`mode` must be replace, add, subtract or intersect (got `{other}`)"))),
    };
    let instance = match opt_num(cmd, p, "instance", 1.0, 10_000.0)? {
        None => None,
        Some(x) if x.fract() == 0.0 => Some(x as usize),
        Some(_) => return Err(bad(cmd, "`instance` must be a whole number starting at 1")),
    };
    let threshold = opt_num(cmd, p, "threshold", 0.0, 1.0)?.unwrap_or(0.5);
    let all_layers = match p.get("sampleAllLayers") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(cmd, "`sampleAllLayers` must be true or false")),
    };
    let refine = opt_bool(cmd, p, "refine", false)?;
    let probe = match p.get("probe") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().ok_or_else(|| bad(cmd, "`probe` must be a probe id from select.probe"))?),
    };
    Ok(Plan { common, mode, instance, threshold, all_layers, points, negatives, bbox, refine, probe })
}

/// Bounding box of the pixels with coverage ≥ 0.5, in document coordinates, and their count.
pub(crate) fn coverage_bounds(cov: &[f32], area: Rect) -> (Rect, u64) {
    let w = area.width() as usize;
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    let mut count = 0u64;
    for (i, c) in cov.iter().enumerate() {
        if *c >= 0.5 && w > 0 {
            let (x, y) = (area.x0 + (i % w) as i32, area.y0 + (i / w) as i32);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
            count += 1;
        }
    }
    if count == 0 { (Rect::EMPTY, 0) } else { (Rect::new(x0, y0, x1, y1), count) }
}

/// The canvas (or the active layer) as the detector's image, at most `cap` pixels, and the scale
/// from canvas to request pixels.
fn detector_image(doc: &Document, active: Option<LayerId>, all_layers: bool, cap: u64) -> Result<(Rgba8, (u32, u32))> {
    let area = doc.bounds();
    let (w, h) = (area.width(), area.height());
    let n = w as usize * h as usize;
    let layer = if all_layers { None } else { active.and_then(|id| doc.layer(id)).and_then(|l| l.surface()) };
    let rgba8: Vec<u8> = match layer {
        Some(surf) => {
            let mut px = vec![[0u8; 4]; n];
            surf.read_rgba8_into(area, &mut px);
            px.into_iter().flatten().collect()
        }
        None => photocraft_compose::render(doc, area).to_rgba8().pixels,
    };
    let image = Rgba8::new(w, h, rgba8).map_err(gen_err)?;
    let (rw, rh) = fit_pixels(w, h, cap);
    let image = if (rw, rh) == (w, h) { image } else { resize_rgba8(&image, rw, rh)? };
    Ok((image, (rw, rh)))
}

/// What one detector run needs besides the picture.
#[derive(Clone)]
struct Prompting {
    template_id: String,
    prompt: String,
    models: Vec<(String, String)>,
    /// The template's placeholders: only those are bound (the text template has no points).
    declared: BTreeSet<String>,
    threshold: f64,
    points: Vec<[f64; 2]>,
    negatives: Vec<[f64; 2]>,
    bbox: Option<Rect>,
}

/// Ask the detector: the masks it returned, at the request size.
fn detect(
    backend: &dyn photocraft_genai::GenerativeBackend,
    server: &str,
    image: Rgba8,
    canvas: Rect,
    pr: &Prompting,
    ctx: &JobCtx,
) -> Result<(Vec<Gray8>, u64)> {
    let (rw, rh) = (image.width, image.height);
    let (sx, sy) = (f64::from(rw) / f64::from(canvas.width().max(1)), f64::from(rh) / f64::from(canvas.height().max(1)));
    let scaled = |pts: &[[f64; 2]]| -> String {
        let v: Vec<Value> = pts
            .iter()
            .map(|[x, y]| {
                let px = (x * sx).round().clamp(0.0, f64::from(rw.saturating_sub(1)));
                let py = (y * sy).round().clamp(0.0, f64::from(rh.saturating_sub(1)));
                json!({"x": px as i64, "y": py as i64})
            })
            .collect();
        Value::Array(v).to_string()
    };
    let mut params = BTreeMap::new();
    params.insert("threshold".to_string(), json!(pr.threshold));
    if pr.declared.contains("points") {
        params.insert("points".to_string(), Value::String(scaled(&pr.points)));
    }
    if pr.declared.contains("negatives") {
        params.insert("negatives".to_string(), Value::String(scaled(&pr.negatives)));
    }
    if let Some(b) = pr.bbox.filter(|_| pr.declared.contains("bx")) {
        let x0 = (f64::from(b.x0) * sx).floor().clamp(0.0, f64::from(rw.saturating_sub(2)));
        let y0 = (f64::from(b.y0) * sy).floor().clamp(0.0, f64::from(rh.saturating_sub(2)));
        let x1 = (f64::from(b.x1) * sx).ceil().clamp(x0 + 1.0, f64::from(rw));
        let y1 = (f64::from(b.y1) * sy).ceil().clamp(y0 + 1.0, f64::from(rh));
        params.insert("bx".to_string(), json!(x0 as i64));
        params.insert("by".to_string(), json!(y0 as i64));
        params.insert("bw".to_string(), json!((x1 - x0) as i64));
        params.insert("bh".to_string(), json!((y1 - y0) as i64));
    }
    let req = Request {
        template: pr.template_id.clone(),
        prompt: pr.prompt.clone(),
        negative: String::new(),
        seed: 0,
        steps: 0,
        guidance: 0.0,
        image: Some(image),
        mask: None,
        models: pr.models.clone(),
        size: None,
        params,
        references: Vec::new(),
    };
    let shown = pr.prompt.clone();
    let resp = match generate_cmds::run_switching(backend, server, &req, &JobProgress::new(ctx)) {
        Ok(r) => r,
        Err(photocraft_genai::Error::NoOutput(_)) => return Err(EngineError::Other(format!("nothing matching \"{shown}\" was found"))),
        Err(e) => return Err(gen_err(e)),
    };
    ctx.check()?;
    // Every result image is a mask: its red channel is the coverage of one instance.
    let mut masks = Vec::new();
    for img in resp.images {
        let red = Gray8::new(img.width, img.height, img.data.as_chunks::<4>().0.iter().map(|p| p[0]).collect()).map_err(gen_err)?;
        masks.push(red);
    }
    Ok((masks, resp.elapsed_ms))
}

/// Soften a detector's hard mask with the classical edge refinement (a narrow smart radius, a
/// little smoothing and feather, as Remove Background and Select Subject do): hair and soft
/// edges get partial coverage from the pixels themselves.
pub(crate) fn refine_coverage(doc: &Document, active: Option<LayerId>, all_layers: bool, cov: Vec<f32>, area: Rect) -> Vec<f32> {
    let (content, found) = coverage_bounds(&cov, area);
    if found == 0 {
        return cov;
    }
    let wv = area.width() as usize;
    let reader = |r: Rect| -> Vec<f32> {
        let mut v = Vec::with_capacity(r.width() as usize * r.height() as usize);
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                let inside = x >= area.x0 && x < area.x1 && y >= area.y0 && y < area.y1;
                v.push(if inside { cov.get((y - area.y0) as usize * wv + (x - area.x0) as usize).copied().unwrap_or(0.0) } else { 0.0 });
            }
        }
        v
    };
    let refined = crate::smartselect_cmds::with_doc_sampler(doc, active, all_layers, |smp, d| {
        photocraft_algo::matting::refine_mask(smp, &reader, content, d.bounds(), &crate::cutout_cmds::REFINE)
    });
    match refined {
        Some(reg) => photocraft_algo::matting::region_reader(&reg)(area),
        None => cov,
    }
}

/// The masks as canvas-sized coverages (a tent filter: no ringing on a mask), empty ones dropped.
fn instances_of(masks: &[Gray8], area: Rect) -> Result<Vec<(Vec<f32>, Rect, u64)>> {
    let (w, h) = (area.width(), area.height());
    let mut out = Vec::new();
    for m in masks {
        let red = if (m.width, m.height) == (w, h) { m.clone() } else { resize_gray8(m, w, h)? };
        let cov: Vec<f32> = red.data.iter().map(|v| f32::from(*v) / 255.0).collect();
        let (bounds, count) = coverage_bounds(&cov, area);
        if count > 0 {
            out.push((cov, bounds, count));
        }
    }
    Ok(out)
}

/// Pick one instance or union them all.
fn pick(instances: &[(Vec<f32>, Rect, u64)], instance: Option<usize>, n: usize, shown: &str) -> Result<Vec<f32>> {
    match instance {
        Some(i) => instances
            .get(i.saturating_sub(1))
            .map(|(cov, _, _)| cov.clone())
            .ok_or_else(|| EngineError::Other(format!("only {} instance(s) of \"{shown}\" were found, so there is no instance {i}", instances.len()))),
        None => {
            let mut m = vec![0.0f32; n];
            for (cov, _, _) in instances {
                for (d, c) in m.iter_mut().zip(cov) {
                    *d = d.max(*c);
                }
            }
            Ok(m)
        }
    }
}

fn instances_json(instances: &[(Vec<f32>, Rect, u64)]) -> Vec<Value> {
    instances.iter().enumerate().map(|(i, (_, b, px))| json!({"index": i + 1, "bounds": [b.x0, b.y0, b.width(), b.height()], "pixels": px})).collect()
}

fn prompting(plan: &Plan) -> Prompting {
    let Common { template, prompt, models, .. } = &plan.common;
    Prompting {
        template_id: template.meta.id.clone(),
        prompt: prompt.clone(),
        models: models.clone(),
        declared: template.meta.placeholders.keys().cloned().collect(),
        threshold: plan.threshold,
        points: plan.points.clone(),
        negatives: plan.negatives.clone(),
        bbox: plan.bbox,
    }
}

/// A kept probe that still applies to the active document: its mask as canvas coverage.
fn probe_coverage(s: &Session, id: u64) -> Option<Vec<f32>> {
    let st = s.active()?;
    let p = s.select_probes.get(id)?;
    if p.doc != st.doc.id || p.past_len != st.history.past_len() {
        return None;
    }
    let area = st.doc.bounds();
    let red = resize_gray8(&p.mask, area.width(), area.height()).ok()?;
    Some(red.data.iter().map(|v| f32::from(*v) / 255.0).collect())
}

fn run_select(s: &mut Session, cmd: &str, p: &Value, label_prefix: &str) -> Result<Value> {
    let plan = plan(s, cmd, p)?;
    // A kept probe: the click applies what the hover found, no server round trip.
    if let Some(cov) = plan.probe.and_then(|id| probe_coverage(s, id)) {
        let Plan { mode, all_layers, refine, .. } = plan;
        let (bounds, count, selected) = s.edit(label_prefix, |doc, active| {
            let area = doc.bounds();
            let cov = if refine { refine_coverage(doc, *active, all_layers, cov, area) } else { cov };
            let (bounds, count) = coverage_bounds(&cov, area);
            doc.selection = sel::combine(doc.selection.as_ref(), &cov, area, mode);
            Ok((bounds, count, doc.selection.is_some()))
        })?;
        let inst = json!([{"index": 1, "bounds": [bounds.x0, bounds.y0, bounds.width(), bounds.height()], "pixels": count}]);
        return Ok(json!({"selected": selected, "count": 1, "instances": inst, "ms": 0, "probe": true}));
    }
    let backend = generate_cmds::backend(s)?;
    let server = generate_cmds::server_key(s);
    let pr = prompting(&plan);
    let Plan { mode, instance, all_layers, points, bbox, refine, .. } = plan;
    let label = if bbox.is_some() {
        "Select by Box".to_string()
    } else if points.is_empty() {
        format!("{label_prefix}: {}", short(&pr.prompt))
    } else {
        label_prefix.to_string()
    };
    let shown = pr.prompt.clone();
    crate::jobs::edit_job(
        s,
        &label,
        move |doc, active, ctx| {
            ctx.progress(0.0, "Rendering");
            let area = doc.bounds();
            let n = area.width() as usize * area.height() as usize;
            let (image, _) = detector_image(doc, *active, all_layers, MAX_SEGMENT_REQUEST_PIXELS)?;
            let (masks, ms) = detect(backend.as_ref(), &server, image, area, &pr, ctx)?;
            ctx.progress(0.95, "Selecting");
            let instances = instances_of(&masks, area)?;
            if instances.is_empty() {
                return Err(EngineError::Other(format!("nothing matching \"{shown}\" was found")));
            }
            let mut mask = pick(&instances, instance, n, &shown)?;
            if refine {
                ctx.progress(0.97, "Refining edge");
                mask = refine_coverage(doc, *active, all_layers, mask, area);
            }
            doc.selection = sel::combine(doc.selection.as_ref(), &mask, area, mode);
            Ok((doc.selection.is_some(), instances_json(&instances), ms))
        },
        move |(selected, instances, ms)| json!({"selected": selected, "count": instances.len(), "instances": instances, "ms": ms}),
    )
}

fn run_by_text(s: &mut Session, p: &Value) -> Result<Value> {
    run_select(s, BY_TEXT, p, "Select by Text")
}

/// What `select.subjectML` asks the model for, by `what`.
fn subject_prompt(what: &str) -> Option<&'static str> {
    Some(match what {
        "subject" => "the main subject",
        "person" => "person",
        "face" => "face",
        "hair" => "hair",
        "sky" => "sky",
        "animal" => "animal",
        "vehicle" => "vehicle",
        "text" => "text",
        _ => return None,
    })
}

/// `x`/`y` become the first of `points`; the backend wants a prompt the point templates ignore.
fn point_params(cmd: &str, p: &Value) -> Result<Value> {
    let mut q = p.as_object().cloned().unwrap_or_else(Map::new);
    let num = |key: &str| -> Result<Option<f64>> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_f64().filter(|v| v.is_finite()).map(Some).ok_or_else(|| bad(cmd, format!("`{key}` must be a canvas coordinate"))),
        }
    };
    if let (Some(x), Some(y)) = (num("x")?, num("y")?) {
        let mut pts = q.get("points").and_then(Value::as_array).cloned().unwrap_or_default();
        pts.insert(0, json!([x, y]));
        q.insert("points".into(), Value::Array(pts));
    }
    q.insert("prompt".into(), Value::String("point".into()));
    Ok(Value::Object(q))
}

/// `select.byPoint`: the object under canvas points (clear of the points to exclude, inside the
/// box), through the detector's prompts; or a kept probe.
fn run_by_point(s: &mut Session, p: &Value) -> Result<Value> {
    let q = point_params(BY_POINT, p)?;
    run_select(s, BY_POINT, &q, "Select by Point")
}

fn run_subject(s: &mut Session, p: &Value) -> Result<Value> {
    let what = opt_str(SUBJECT_ML, p, "what", 20)?.unwrap_or("subject");
    let prompt = subject_prompt(what)
        .ok_or_else(|| bad(SUBJECT_ML, format!("`what` must be subject, person, face, hair, sky, animal, vehicle or text (got `{what}`)")))?;
    let mut q = p.as_object().cloned().unwrap_or_else(Map::new);
    q.insert("prompt".into(), Value::String(prompt.into()));
    run_select(s, SUBJECT_ML, &Value::Object(q), "Select Subject")
}

/// `select.probe`: what `select.byPoint` would select for these points, as a mask the UI can
/// show under the pointer, kept for the click that follows (`probe` on `select.byPoint`). Not an
/// edit: nothing is selected, nothing journaled, the document is not locked.
fn run_probe(s: &mut Session, p: &Value) -> Result<Value> {
    let q = point_params(PROBE, p)?;
    let plan = plan(s, PROBE, &q)?;
    let backend = generate_cmds::backend(s)?;
    let server = generate_cmds::server_key(s);
    let pr = prompting(&plan);
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let (base, active, doc_id, past_len) = (Arc::clone(&st.doc), st.active_layer, st.doc.id, st.history.past_len());
    let all_layers = plan.all_layers;
    let points = plan.points.clone();
    crate::jobs::run(
        s,
        "Object finder",
        false,
        move |ctx| {
            let area = base.bounds();
            let (image, (rw, rh)) = detector_image(&base, active, all_layers, MAX_PROBE_REQUEST_PIXELS)?;
            let (masks, ms) = detect(backend.as_ref(), &server, image, area, &pr, ctx)?;
            // The union of the instances, at the request size.
            let mut data = vec![0u8; rw as usize * rh as usize];
            let mut any = false;
            for m in &masks {
                let m = if (m.width, m.height) == (rw, rh) { m.clone() } else { resize_gray8(m, rw, rh)? };
                for (d, v) in data.iter_mut().zip(&m.data) {
                    *d = (*d).max(*v);
                    any |= *v >= 128;
                }
            }
            if !any {
                return Err(EngineError::Other("nothing under the pointer".into()));
            }
            let mask = Gray8::new(rw, rh, data).map_err(gen_err)?;
            let (bounds, _) = coverage_bounds(&mask.data.iter().map(|v| f32::from(*v) / 255.0).collect::<Vec<_>>(), Rect::new(0, 0, rw as i32, rh as i32));
            Ok((mask, bounds, ms))
        },
        move |s, (mask, rb, ms)| {
            // Request → canvas pixels for the bounds.
            let (w, h) = s.active().map(|d| (d.doc.size.width, d.doc.size.height)).unwrap_or((1, 1));
            let (sx, sy) = (f64::from(w) / f64::from(mask.width.max(1)), f64::from(h) / f64::from(mask.height.max(1)));
            let bounds = [
                (f64::from(rb.x0) * sx).floor() as i32,
                (f64::from(rb.y0) * sy).floor() as i32,
                (f64::from(rb.width()) * sx).ceil() as i32,
                (f64::from(rb.height()) * sy).ceil() as i32,
            ];
            let (mw, mh) = (mask.width, mask.height);
            let encoded = b64_encode(&mask.data);
            let id = s.select_probes.push(Probe { id: 0, doc: doc_id, past_len, points, mask });
            Ok(json!({"probe": id, "width": mw, "height": mh, "bounds": bounds, "mask": encoded, "ms": ms}))
        },
    )
}

/// Standard base64 with padding (a probe mask is at most a few hundred kilobytes).
pub(crate) fn b64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8) | u32::from(chunk.get(2).copied().unwrap_or(0));
        let c = |i: u32| char::from(T[((n >> (18 - 6 * i)) & 63) as usize]);
        out.push(c(0));
        out.push(c(1));
        out.push(if chunk.len() > 1 { c(2) } else { '=' });
        out.push(if chunk.len() > 2 { c(3) } else { '=' });
    }
    out
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: BY_TEXT,
            label: "Select by Text…",
            // Not a Photoshop menu item: listed after Photoshop's Select items (menus.rs "extra").
            menu: &["Select"],
            shortcut: None,
            params: r#"{"prompt":text,"mode":"replace|add|subtract|intersect","threshold":0..1=0.5,"sampleAllLayers":bool=true,"instance":{1..?=all},"refine":bool=false,"template":{id?="sam3.1/segment"},"model":{file?}} → {"selected","count","instances":[{"index","bounds":[x,y,w,h],"pixels"}],"ms"} (prompt: a short phrase such as "the dog", "red car" or "eye:2", comma-separated terms allowed; instance picks one of the found instances, default all; refine softens the detector's hard edge with the classical refinement; a background job; needs a ComfyUI server with the SAM 3.1 checkpoint, see Preferences › AI Integrations)"#,
            enabled,
            run: run_by_text,
            journal: true,
        },
        CommandSpec {
            id: BY_POINT,
            label: "Select by Point",
            menu: &[],
            shortcut: None,
            params: r#"{"x":int?,"y":int?,"points":[[x,y]]?,"negativePoints":[[x,y]]?,"box":[x,y,w,h]?,"mode":"replace|add|subtract|intersect","threshold":0..1=0.5,"sampleAllLayers":bool=true,"refine":bool=false,"probe":{u64?},"template":{id?="sam3.1/segment-point", "sam3.1/segment-box" with a box},"model":{file?}} → {"selected","count","instances":[{"index","bounds":[x,y,w,h],"pixels"}],"ms","probe"?} (the object under the point(s) (x/y or points, up to 64), kept clear of negativePoints and searched for inside box, through SAM 3.1's prompts; probe = apply a mask select.probe kept for this document instead of asking the server (stale or unknown probes are ignored and the server asked); refine softens the hard edge; a background job; needs a ComfyUI server with the SAM 3.1 checkpoint)"#,
            enabled,
            run: run_by_point,
            journal: true,
        },
        CommandSpec {
            id: PROBE,
            label: "Object Finder",
            menu: &[],
            shortcut: None,
            params: r#"{"x":int?,"y":int?,"points":[[x,y]]?,"negativePoints":[[x,y]]?,"box":[x,y,w,h]?,"threshold":0..1=0.5,"sampleAllLayers":bool=true} → {"probe","width","height","bounds":[x,y,w,h],"mask":base64,"ms"} (what select.byPoint would select for these prompts, as a grayscale mask of width × height (base64, row-major, 255 = selected) over the whole canvas, kept as probe for a select.byPoint that follows on this document before its next edit; selects nothing, journals nothing, locks nothing: the Object Selection tool runs it when the pointer rests; needs a ComfyUI server with the SAM 3.1 checkpoint)"#,
            enabled,
            run: run_probe,
            journal: false,
        },
        CommandSpec {
            id: SUBJECT_ML,
            label: "Select Subject (ML)",
            menu: &[],
            shortcut: None,
            params: r#"{"what":"subject|person|face|hair|sky|animal|vehicle|text","mode":"replace|add|subtract|intersect","threshold":0..1=0.5,"sampleAllLayers":bool=true,"instance":{1..?=all},"refine":bool=false} → {"selected","count","instances","ms"} (Select by Text with a fixed phrase; the classical select.subject stays available without a server)"#,
            enabled,
            run: run_subject,
            journal: true,
        },
    ]
}

#[cfg(test)]
#[path = "select_ml_cmds_tests.rs"]
mod tests;
