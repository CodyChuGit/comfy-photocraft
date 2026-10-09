//! `select.*` commands backed by a segmentation model (SAM 3.1 through the generative backend):
//! **Select by Text** ("the dog", "red car", `eye:2`) and the ML **Select Subject**.
//!
//! The composite (or the active layer) goes to the backend as an image with the phrase; the
//! model returns one mask per instance. The command turns them into coverage over the canvas,
//! picks one instance or unions them, and combines the result with the current selection by
//! the usual `replace|add|subtract|intersect` modes, as one undo step. It is a background job
//! like the other backend commands (progress, Esc cancels). The model is a detector, so a run
//! takes well under a second once the checkpoint is loaded.
//!
//! The backend and its preferences are shared with `generate_cmds` (Preferences › AI
//! Integrations). Nothing here needs a diffusion model.

use std::collections::BTreeMap;

use photocraft_algo::selection::{self as sel, SelectionMode};
use photocraft_genai::{Request, Rgba8, Task};
use photocraft_geom::Rect;
use serde_json::{Map, Value, json};

use crate::commands::CommandSpec;
use crate::generate_cmds::{self, Common, JobProgress, bad, fit_pixels, gen_err, opt_num, opt_str, resize_gray8, resize_rgba8, short};
use crate::{EngineError, Result, Session};

pub const BY_TEXT: &str = "select.byText";
pub const SUBJECT_ML: &str = "select.subjectML";
pub const DEFAULT_SEGMENT_TEMPLATE: &str = "sam3.1/segment";
/// A detector request is sent at most this large (SAM 3.1 works at about 1 megapixel inside;
/// the masks come back at the request size and are resampled to the canvas).
const MAX_SEGMENT_REQUEST_PIXELS: u64 = 2048 * 1024;

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
}

fn plan(s: &Session, cmd: &str, p: &Value) -> Result<Plan> {
    let common = generate_cmds::plan_common(s, cmd, p, DEFAULT_SEGMENT_TEMPLATE, Task::Segment, "")?;
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
    Ok(Plan { common, mode, instance, threshold, all_layers })
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

fn run_select(s: &mut Session, cmd: &str, p: &Value, label_prefix: &str) -> Result<Value> {
    let plan = plan(s, cmd, p)?;
    let backend = generate_cmds::backend(s)?;
    let server = generate_cmds::server_key(s);
    let Plan { common, mode, instance, threshold, all_layers } = plan;
    let Common { template, prompt, models, .. } = common;
    let label = format!("{label_prefix}: {}", short(&prompt));
    let template_id = template.meta.id.clone();
    let shown = prompt.clone();
    crate::jobs::edit_job(
        s,
        &label,
        move |doc, active, ctx| {
            ctx.progress(0.0, "Rendering");
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
            let (rw, rh) = fit_pixels(w, h, MAX_SEGMENT_REQUEST_PIXELS);
            let image = if (rw, rh) == (w, h) { image } else { resize_rgba8(&image, rw, rh)? };
            let mut params = BTreeMap::new();
            params.insert("threshold".to_string(), json!(threshold));
            let req = Request {
                template: template_id,
                prompt,
                negative: String::new(),
                seed: 0,
                steps: 0,
                guidance: 0.0,
                image: Some(image),
                mask: None,
                models,
                size: None,
                params,
            };
            let resp = match generate_cmds::run_switching(backend.as_ref(), &server, &req, &JobProgress::new(ctx)) {
                Ok(r) => r,
                Err(photocraft_genai::Error::NoOutput(_)) => return Err(EngineError::Other(format!("nothing matching \"{shown}\" was found"))),
                Err(e) => return Err(gen_err(e)),
            };
            ctx.check()?;
            ctx.progress(0.95, "Selecting");
            // Every result image is a mask: its red channel is the coverage of one instance,
            // brought to the canvas size with a tent filter (no ringing on a mask).
            let mut instances: Vec<(Vec<f32>, Rect, u64)> = Vec::new();
            for img in resp.images {
                let red = photocraft_genai::Gray8::new(img.width, img.height, img.data.as_chunks::<4>().0.iter().map(|p| p[0]).collect()).map_err(gen_err)?;
                let red = if (red.width, red.height) == (w, h) { red } else { resize_gray8(&red, w, h)? };
                let cov: Vec<f32> = red.data.iter().map(|v| f32::from(*v) / 255.0).collect();
                let (bounds, count) = coverage_bounds(&cov, area);
                if count > 0 {
                    instances.push((cov, bounds, count));
                }
            }
            if instances.is_empty() {
                return Err(EngineError::Other(format!("nothing matching \"{shown}\" was found")));
            }
            let mask: Vec<f32> = match instance {
                Some(i) => instances.get(i.saturating_sub(1)).map(|(cov, _, _)| cov.clone()).ok_or_else(|| {
                    EngineError::Other(format!("only {} instance(s) of \"{shown}\" were found, so there is no instance {i}", instances.len()))
                })?,
                None => {
                    let mut m = vec![0.0f32; n];
                    for (cov, _, _) in &instances {
                        for (d, c) in m.iter_mut().zip(cov) {
                            *d = d.max(*c);
                        }
                    }
                    m
                }
            };
            doc.selection = sel::combine(doc.selection.as_ref(), &mask, area, mode);
            let info: Vec<Value> = instances
                .iter()
                .enumerate()
                .map(|(i, (_, b, px))| json!({"index": i + 1, "bounds": [b.x0, b.y0, b.width(), b.height()], "pixels": px}))
                .collect();
            Ok((doc.selection.is_some(), info, resp.elapsed_ms))
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

fn run_subject(s: &mut Session, p: &Value) -> Result<Value> {
    let what = opt_str(SUBJECT_ML, p, "what", 20)?.unwrap_or("subject");
    let prompt = subject_prompt(what)
        .ok_or_else(|| bad(SUBJECT_ML, format!("`what` must be subject, person, face, hair, sky, animal, vehicle or text (got `{what}`)")))?;
    let mut q = p.as_object().cloned().unwrap_or_else(Map::new);
    q.insert("prompt".into(), Value::String(prompt.into()));
    run_select(s, SUBJECT_ML, &Value::Object(q), "Select Subject")
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: BY_TEXT,
            label: "Select by Text…",
            // Not a Photoshop menu item: listed after Photoshop's Select items (menus.rs "extra").
            menu: &["Select"],
            shortcut: None,
            params: r#"{"prompt":text,"mode":"replace|add|subtract|intersect","threshold":0..1=0.5,"sampleAllLayers":bool=true,"instance":{1..?=all},"template":{id?="sam3.1/segment"},"model":{file?}} → {"selected","count","instances":[{"index","bounds":[x,y,w,h],"pixels"}],"ms"} (prompt: a short phrase such as "the dog", "red car" or "eye:2", comma-separated terms allowed; instance picks one of the found instances, default all; a background job; needs a ComfyUI server with the SAM 3.1 checkpoint, see Preferences › AI Integrations)"#,
            enabled,
            run: run_by_text,
            journal: true,
        },
        CommandSpec {
            id: SUBJECT_ML,
            label: "Select Subject (ML)",
            menu: &[],
            shortcut: None,
            params: r#"{"what":"subject|person|face|hair|sky|animal|vehicle|text","mode":"replace|add|subtract|intersect","threshold":0..1=0.5,"sampleAllLayers":bool=true,"instance":{1..?=all}} → {"selected","count","instances","ms"} (Select by Text with a fixed phrase; the classical select.subject stays available without a server)"#,
            enabled,
            run: run_subject,
            journal: true,
        },
    ]
}

#[cfg(test)]
#[path = "select_ml_cmds_tests.rs"]
mod tests;
