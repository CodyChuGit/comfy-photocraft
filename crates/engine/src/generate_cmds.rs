//! `generate.*` commands: generative edits through a local backend (`photocraft-genai`).
//!
//! `generate.fill` is Generative Fill. The flattened composite around the selection and the
//! selection's coverage go to the backend (a ComfyUI server, Preferences › AI Integrations), and
//! the result comes back as a **new raster layer above the active one, masked to the selection**.
//! The original pixels never change. The command is a background job (`jobs::edit_job`): progress
//! comes from the server, Esc cancels and interrupts it, and the edit lands as one undo step.
//! `generate.health` and `generate.models` are queries for the UI, the CLI and agents.
//!
//! Preferences read here: `integrations.comfyServer`, `integrations.defaultEditModel`,
//! `integrations.generativeTimeoutSecs`, `integrations.allowResearchModels`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use photocraft_color::PixelFormat;
use photocraft_doc::{Layer, LayerMask};
use photocraft_genai::template::{self, License, Template};
use photocraft_genai::{GenerativeBackend, Gray8, Health, Request, Rgba8, Task};
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::jobs::JobCtx;
use crate::{EngineError, Result, Session};

pub const FILL: &str = "generate.fill";
pub const HEALTH: &str = "generate.health";
pub const MODELS: &str = "generate.models";
pub const DEFAULT_FILL_TEMPLATE: &str = "qwen-edit-2511/fill";
/// Largest request image sent to a backend; the selection plus its margin is refused above it.
const MAX_REQUEST_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_PROMPT_CHARS: usize = 4000;

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

fn gen_err(e: photocraft_genai::Error) -> EngineError {
    match e {
        photocraft_genai::Error::Cancelled => EngineError::Cancelled,
        e => EngineError::Other(e.to_string()),
    }
}

/// The backend the preferences point at. The web build has none yet.
fn backend(s: &Session) -> Result<Arc<dyn GenerativeBackend>> {
    let integrations = &s.prefs().integrations;
    let url = integrations.comfy_server.trim();
    if url.is_empty() {
        return Err(EngineError::Other("no generative server is configured (Preferences › AI Integrations › ComfyUI Server)".into()));
    }
    let timeout = Duration::from_secs(u64::from(integrations.generative_timeout_secs.max(5)));
    #[cfg(not(target_arch = "wasm32"))]
    {
        let b = photocraft_genai::comfy::ComfyBackend::new(url, timeout).map_err(gen_err)?;
        Ok(Arc::new(b))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = timeout;
        Err(EngineError::Other("generative features are not available in the web build yet".into()))
    }
}

fn web_unavailable() -> std::result::Result<(), String> {
    if cfg!(target_arch = "wasm32") { Err("not available in the web build yet".into()) } else { Ok(()) }
}

fn fill_enabled(s: &Session) -> std::result::Result<(), String> {
    web_unavailable()?;
    let d = s.active().ok_or("no document open")?;
    if d.doc.selection.is_none() {
        return Err("make a selection first".into());
    }
    Ok(())
}

/// Maps the backend's progress onto the job's bar after the render step.
struct JobProgress<'a>(&'a JobCtx);

impl photocraft_genai::Progress for JobProgress<'_> {
    fn report(&self, fraction: f32, message: &str) {
        self.0.progress(0.05 + 0.9 * fraction.clamp(0.0, 1.0), message);
    }
    fn cancelled(&self) -> bool {
        self.0.cancelled()
    }
}

/// Everything `generate.fill` needs, validated before any pixel or network work.
struct FillPlan {
    template: Template,
    prompt: String,
    negative: String,
    seed: u64,
    steps: u32,
    guidance: f32,
    name: String,
    models: Vec<(String, String)>,
    /// The area sent to the backend: the selection's bounds grown by the margin, on the canvas.
    rect: Rect,
}

fn opt_str<'a>(cmd: &str, p: &'a Value, key: &str, max: usize) -> Result<Option<&'a str>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            if s.chars().count() > max {
                return Err(bad(cmd, format!("`{key}` is longer than {max} characters")));
            }
            Ok(Some(s))
        }
        Some(_) => Err(bad(cmd, format!("`{key}` must be a string"))),
    }
}

fn opt_num(cmd: &str, p: &Value, key: &str, lo: f64, hi: f64) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let x = v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| bad(cmd, format!("`{key}` must be a number")))?;
            if !(lo..=hi).contains(&x) {
                return Err(bad(cmd, format!("`{key}` must be within {lo}..{hi} (got {x})")));
            }
            Ok(Some(x))
        }
    }
}

/// The first 40 characters of a prompt, for layer names and job labels.
fn short(prompt: &str) -> String {
    let mut s: String = prompt.chars().take(40).collect();
    if prompt.chars().count() > 40 {
        s.push('…');
    }
    s
}

fn plan_fill(s: &Session, p: &Value) -> Result<FillPlan> {
    let prompt = opt_str(FILL, p, "prompt", MAX_PROMPT_CHARS)?
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| bad(FILL, "`prompt` is required: describe what to generate"))?
        .to_string();
    let negative = opt_str(FILL, p, "negative", MAX_PROMPT_CHARS)?.unwrap_or("").to_string();
    let template_id = opt_str(FILL, p, "template", 100)?.unwrap_or(DEFAULT_FILL_TEMPLATE);
    let template = template::find(template_id).map_err(|e| bad(FILL, e.to_string()))?;
    if template.meta.task != Task::Fill {
        return Err(bad(FILL, format!("template `{template_id}` is not a fill template")));
    }
    let integrations = &s.prefs().integrations;
    if template.meta.license == License::Research && !integrations.allow_research_models {
        return Err(EngineError::Other(format!(
            "`{}` uses a research-only model; turn on Allow Research-Only Models in Preferences › AI Integrations to use it",
            template.meta.id
        )));
    }
    let seed = match p.get("seed") {
        None | Some(Value::Null) => photocraft_genai::random_seed(),
        Some(v) => {
            let x = v
                .as_f64()
                .filter(|x| x.is_finite() && *x >= 0.0 && x.fract() == 0.0 && *x < 9_007_199_254_740_992.0)
                .ok_or_else(|| bad(FILL, "`seed` must be a non-negative integer below 2^53"))?;
            x as u64
        }
    };
    let steps = opt_num(FILL, p, "steps", 1.0, 250.0)?.map_or(0, |x| x.round() as u32);
    let guidance = opt_num(FILL, p, "guidance", 0.0, 30.0)?.map_or(0.0, |x| x as f32);
    let margin = opt_num(FILL, p, "margin", 0.0, 1.0)?.unwrap_or(0.25);
    let name = opt_str(FILL, p, "name", 200)?.map(str::to_string);
    // The diffusion model: the `model` param, else the preference, else the template's default.
    let mut models = Vec::new();
    let model =
        opt_str(FILL, p, "model", 200)?.map(str::to_string).or_else(|| Some(integrations.default_edit_model.trim().to_string()).filter(|m| !m.is_empty()));
    if let Some(m) = model {
        let slot =
            template.meta.models.first().map(|s| s.placeholder.clone()).ok_or_else(|| bad(FILL, format!("template `{template_id}` has no model slot")))?;
        models.push((slot, m));
    }
    template.model_bindings(&models).map_err(|e| bad(FILL, e.to_string()))?;

    let d = s.active().ok_or(EngineError::NoDocument)?;
    let sel = d.doc.selection.as_ref().ok_or_else(|| EngineError::Other("make a selection first".into()))?;
    let canvas = d.doc.bounds();
    let bounds = sel.content_bounds().intersect(&canvas);
    if bounds.is_empty() {
        return Err(EngineError::Other("the selection is empty".into()));
    }
    let longer = i64::from(bounds.width()).max(i64::from(bounds.height()));
    let m = (margin * longer as f64).round().clamp(0.0, 65_536.0) as i32;
    let rect = Rect::new(bounds.x0.saturating_sub(m), bounds.y0.saturating_sub(m), bounds.x1.saturating_add(m), bounds.y1.saturating_add(m)).intersect(&canvas);
    if rect.is_empty() {
        return Err(EngineError::Other("the selection is empty".into()));
    }
    let pixels = u64::from(rect.width()) * u64::from(rect.height());
    if pixels > MAX_REQUEST_PIXELS {
        return Err(EngineError::Other(format!(
            "the selected area with its margin is {}×{} pixels; Generative Fill handles up to 16 megapixels at a time: select a smaller area or lower `margin`",
            rect.width(),
            rect.height()
        )));
    }
    let name = name.unwrap_or_else(|| format!("Generative Fill: {}", short(&prompt)));
    Ok(FillPlan { template, prompt, negative, seed, steps, guidance, name, models, rect })
}

/// Bilinear resize, for a backend that returns a different size than it was given.
fn resize_rgba8(src: &Rgba8, w: u32, h: u32) -> Result<Rgba8> {
    let (sw, sh) = (src.width as usize, src.height as usize);
    if sw == 0 || sh == 0 || w == 0 || h == 0 {
        return Err(EngineError::Other("cannot resize an empty image".into()));
    }
    let at = |x: usize, y: usize, c: usize| -> f32 { src.data.get((y * sw + x) * 4 + c).map_or(0.0, |&v| f32::from(v)) };
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as usize {
        let fy = ((y as f32 + 0.5) * sh as f32 / h as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = fy as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = fy - y0 as f32;
        for x in 0..w as usize {
            let fx = ((x as f32 + 0.5) * sw as f32 / w as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = fx as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let tx = fx - x0 as f32;
            for c in 0..4 {
                let top = at(x0, y0, c) * (1.0 - tx) + at(x1, y0, c) * tx;
                let bottom = at(x0, y1, c) * (1.0 - tx) + at(x1, y1, c) * tx;
                out.push((top * (1.0 - ty) + bottom * ty + 0.5).clamp(0.0, 255.0) as u8);
            }
        }
    }
    Rgba8::new(w, h, out).map_err(gen_err)
}

fn run_fill(s: &mut Session, p: &Value) -> Result<Value> {
    let plan = plan_fill(s, p)?;
    let backend = backend(s)?;
    let FillPlan { template, prompt, negative, seed, steps, guidance, name, models, rect } = plan;
    let label = name.clone();
    let template_id = template.meta.id.clone();
    let tid = template_id.clone();
    crate::jobs::edit_job(
        s,
        &label,
        move |doc, active, ctx| {
            ctx.progress(0.0, "Rendering");
            let (w, h) = (rect.width(), rect.height());
            let composite = photocraft_compose::render(doc, rect).to_rgba8();
            let image = Rgba8::new(composite.width, composite.height, composite.pixels).map_err(gen_err)?;
            let sel = doc.selection.as_ref().ok_or_else(|| EngineError::Other("the selection is gone".into()))?;
            let k = sel.channels().max(1);
            let coverage: Vec<f32> = sel.read_region(rect).chunks_exact(k).map(|c| c.first().copied().unwrap_or(0.0).clamp(0.0, 1.0)).collect();
            let mask = Gray8::new(w, h, coverage.iter().map(|c| (c * 255.0 + 0.5) as u8).collect()).map_err(gen_err)?;
            ctx.check()?;
            let req = Request { template: template_id, prompt, negative, seed, steps, guidance, image: Some(image), mask: Some(mask), models };
            let resp = backend.run(&req, &JobProgress(ctx)).map_err(gen_err)?;
            ctx.check()?;
            let out = resp.images.into_iter().next().ok_or_else(|| EngineError::Other("the backend returned no image".into()))?;
            let out = if (out.width, out.height) == (w, h) { out } else { resize_rgba8(&out, w, h)? };
            ctx.progress(0.96, "Placing");
            // The result becomes a layer in the document's own format, masked to the selection.
            let fmt = doc.pixel_format();
            let mut layer = Layer::raster(name, fmt);
            let surf = crate::pixels_mut(&mut layer)?;
            let n = fmt.channels();
            let mut px = vec![0.0f32; w as usize * h as usize * n];
            let mut tmp = [0.0f32; 8];
            for (i, s4) in out.data.as_chunks::<4>().0.iter().enumerate() {
                let rgba = [f32::from(s4[0]) / 255.0, f32::from(s4[1]) / 255.0, f32::from(s4[2]) / 255.0, f32::from(s4[3]) / 255.0];
                let used = photocraft_raster::from_rgba_into(&fmt, rgba, &mut tmp).min(n);
                if let (Some(dst), Some(src)) = (px.get_mut(i * n..i * n + used), tmp.get(..used)) {
                    dst.copy_from_slice(src);
                }
            }
            surf.write_region(rect, &px);
            let mut mask_surface = Surface::with_default(PixelFormat::GRAY8, &[0.0]);
            mask_surface.write_region(rect, &coverage);
            layer.mask = Some(LayerMask { surface: mask_surface, enabled: true, linked: true, density: 1.0, feather: 0.0 });
            let nid = doc.insert_above(*active, layer);
            *active = Some(nid);
            Ok((nid, resp.seed, resp.run_id, resp.elapsed_ms, w, h))
        },
        move |(nid, seed, run_id, ms, w, h)| json!({"layer": nid.0, "seed": seed, "template": tid, "runId": run_id, "width": w, "height": h, "ms": ms}),
    )
}

fn run_health(s: &mut Session, _p: &Value) -> Result<Value> {
    let h = match backend(s) {
        Ok(b) => b.health(),
        Err(e) => Health { backend: "comfyui".into(), url: s.prefs().integrations.comfy_server.clone(), error: Some(e.to_string()), ..Health::default() },
    };
    serde_json::to_value(&h).map_err(|e| EngineError::Other(e.to_string()))
}

fn run_models(s: &mut Session, _p: &Value) -> Result<Value> {
    let allow_research = s.prefs().integrations.allow_research_models;
    let backend = backend(s).ok();
    let health = backend.as_ref().map(|b| b.health());
    let online = health.as_ref().is_some_and(|h| h.ok);
    let mut folders: BTreeMap<String, Option<Vec<String>>> = BTreeMap::new();
    let mut templates = Vec::new();
    for t in template::builtin() {
        let m = &t.meta;
        let mut slots = Vec::new();
        for slot in &m.models {
            let files = if online {
                folders.entry(slot.folder.clone()).or_insert_with(|| backend.as_ref().and_then(|b| b.model_files(&slot.folder).ok())).clone()
            } else {
                None
            };
            let installed = files.as_ref().map(|f| f.iter().any(|x| x == &slot.default));
            slots.push(json!({"placeholder": slot.placeholder, "folder": slot.folder, "file": slot.default, "installed": installed}));
        }
        templates.push(json!({
            "id": m.id, "name": m.name, "family": m.family, "task": m.task, "license": m.license, "licenseNote": m.license_note,
            "allowed": m.license != License::Research || allow_research,
            "defaults": {"steps": m.defaults.steps, "guidance": m.defaults.guidance},
            "needsImage": m.needs_image, "needsMask": m.needs_mask, "models": slots,
        }));
    }
    Ok(json!({"server": health, "templates": templates}))
}

fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: FILL,
            label: "Generative Fill…",
            menu: &[],
            shortcut: None,
            params: r#"{"prompt":str,"negative":str?="","template":id?="qwen-edit-2511/fill","model":file?=Preferences,"seed":u64?=random,"steps":1..250?=template,"guidance":0..30?=template,"margin":0..1=0.25 (context around the selection, as a fraction of its larger side),"name":str?} → {"layer","seed","template","runId","width","height","ms"} (a background job: the result is a new layer above the active one, masked to the selection; needs a ComfyUI server, see Preferences › AI Integrations)"#,
            enabled: fill_enabled,
            run: run_fill,
            journal: true,
        },
        CommandSpec {
            id: HEALTH,
            label: "Generative Server Status",
            menu: &[],
            shortcut: None,
            params: r#"{} → {"ok","backend","url","version","vramTotal","vramFree","queueRemaining","error"?}"#,
            enabled: always,
            run: run_health,
            journal: false,
        },
        CommandSpec {
            id: MODELS,
            label: "List Generative Models",
            menu: &[],
            shortcut: None,
            params: r#"{} → {"server","templates":[{"id","name","family","task","license","allowed","defaults","models":[{"placeholder","folder","file","installed"}]}]}"#,
            enabled: always,
            run: run_models,
            journal: false,
        },
    ]
}

#[cfg(test)]
#[path = "generate_cmds_tests.rs"]
mod tests;
