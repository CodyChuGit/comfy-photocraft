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

use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Document, Layer, LayerId, LayerMask, Size};
use photocraft_genai::template::{self, License, Template};
use photocraft_genai::{GenerativeBackend, Gray8, Health, Request, Rgba8, Task};
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::jobs::JobCtx;
use crate::{EngineError, Result, Session};

pub const FILL: &str = "generate.fill";
pub const IMAGE: &str = "generate.image";
pub const HEALTH: &str = "generate.health";
pub const MODELS: &str = "generate.models";
pub const VARIATION: &str = "generate.variation";
/// Most results one `generate.fill` call makes (`variations`).
pub const MAX_VARIATIONS: u32 = 4;
/// Seeds stay below 2^53 so they survive a round trip through JSON numbers.
const MAX_SEED: u64 = 1 << 53;
pub const DEFAULT_FILL_TEMPLATE: &str = "qwen-edit-2511/fill";
pub const DEFAULT_IMAGE_TEMPLATE: &str = "krea2-turbo/image";
/// Text-to-image sizes are rounded down to this grid (latent patches); the smallest side allowed.
const SIZE_STEP: u32 = 16;
const MIN_SIDE: u32 = 64;
const MAX_SIDE: u32 = 4096;
/// Largest request image sent to a backend; the selection plus its margin is refused above it.
const MAX_REQUEST_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_PROMPT_CHARS: usize = 4000;

pub(crate) fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

pub(crate) fn gen_err(e: photocraft_genai::Error) -> EngineError {
    match e {
        photocraft_genai::Error::Cancelled => EngineError::Cancelled,
        e => EngineError::Other(e.to_string()),
    }
}

/// The backend the preferences point at. The web build has none yet.
pub(crate) fn backend(s: &Session) -> Result<Arc<dyn GenerativeBackend>> {
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

pub(crate) fn web_unavailable() -> std::result::Result<(), String> {
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

/// Maps the backend's progress onto the job's bar: the `lo..hi` part of it, after the render step.
pub(crate) struct JobProgress<'a> {
    ctx: &'a JobCtx,
    lo: f32,
    hi: f32,
}

impl<'a> JobProgress<'a> {
    /// One backend run filling the bar from 5 % to 95 %.
    pub(crate) fn new(ctx: &'a JobCtx) -> Self {
        Self::span(ctx, 0.05, 0.95)
    }
    /// The part of the bar one of several backend runs occupies.
    pub(crate) fn span(ctx: &'a JobCtx, lo: f32, hi: f32) -> Self {
        Self { ctx, lo, hi }
    }
}

impl photocraft_genai::Progress for JobProgress<'_> {
    fn report(&self, fraction: f32, message: &str) {
        self.ctx.progress(self.lo + (self.hi - self.lo) * fraction.clamp(0.0, 1.0), message);
    }
    fn cancelled(&self) -> bool {
        self.ctx.cancelled()
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
    /// How many results to make (consecutive seeds); only the first is visible.
    variations: u32,
}

pub(crate) fn opt_str<'a>(cmd: &str, p: &'a Value, key: &str, max: usize) -> Result<Option<&'a str>> {
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

pub(crate) fn opt_num(cmd: &str, p: &Value, key: &str, lo: f64, hi: f64) -> Result<Option<f64>> {
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
pub(crate) fn short(prompt: &str) -> String {
    let mut s: String = prompt.chars().take(40).collect();
    if prompt.chars().count() > 40 {
        s.push('…');
    }
    s
}

/// A template id from a preference, or the built-in default when the preference is empty.
fn template_or(pref: &str, fallback: &str) -> String {
    let t = pref.trim();
    if t.is_empty() { fallback.to_string() } else { t.to_string() }
}

fn plan_fill(s: &Session, p: &Value) -> Result<FillPlan> {
    let (default_template, default_model) = {
        let integrations = &s.prefs().integrations;
        (template_or(&integrations.default_fill_template, DEFAULT_FILL_TEMPLATE), integrations.default_edit_model.clone())
    };
    let Common { template, prompt, negative, seed, steps, guidance, models, name } = plan_common(s, FILL, p, &default_template, Task::Fill, &default_model)?;
    let margin = opt_num(FILL, p, "margin", 0.0, 1.0)?.unwrap_or(0.25);
    let variations = match opt_num(FILL, p, "variations", 1.0, f64::from(MAX_VARIATIONS))? {
        None => 1,
        Some(x) if x.fract() == 0.0 => x as u32,
        Some(_) => return Err(bad(FILL, format!("`variations` must be a whole number from 1 to {MAX_VARIATIONS}"))),
    };

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
    Ok(FillPlan { template, prompt, negative, seed, steps, guidance, name, models, rect, variations })
}

/// Bilinear resize, for a backend that returns a different size than it was given.
pub(crate) fn resize_rgba8(src: &Rgba8, w: u32, h: u32) -> Result<Rgba8> {
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

/// Write RGBA8 pixels into a raster layer over `rect`, converting to the layer's own format
/// (gray, CMYK, 16-bit, float…); `src` must have `rect`'s size.
fn write_rgba8(layer: &mut Layer, src: &Rgba8, rect: Rect) -> Result<()> {
    if (src.width, src.height) != (rect.width(), rect.height()) {
        return Err(EngineError::Other("internal error: the image and its rectangle differ in size".into()));
    }
    let surf = crate::pixels_mut(layer)?;
    let fmt = surf.format();
    let n = fmt.channels();
    let mut px = vec![0.0f32; src.width as usize * src.height as usize * n];
    let mut tmp = [0.0f32; 8];
    for (i, s4) in src.data.as_chunks::<4>().0.iter().enumerate() {
        let rgba = [f32::from(s4[0]) / 255.0, f32::from(s4[1]) / 255.0, f32::from(s4[2]) / 255.0, f32::from(s4[3]) / 255.0];
        let used = photocraft_raster::from_rgba_into(&fmt, rgba, &mut tmp).min(n);
        if let (Some(dst), Some(src)) = (px.get_mut(i * n..i * n + used), tmp.get(..used)) {
            dst.copy_from_slice(src);
        }
    }
    surf.write_region(rect, &px);
    Ok(())
}

/// Everything `generate.image` needs, validated before any network work.
struct ImagePlan {
    template: Template,
    prompt: String,
    negative: String,
    seed: u64,
    steps: u32,
    guidance: f32,
    name: String,
    models: Vec<(String, String)>,
    /// The size asked of the model (rounded to the latent grid).
    width: u32,
    height: u32,
    /// `true`: a new document of the result's size; `false`: a new layer over the whole canvas.
    to_document: bool,
}

/// Shared validation of the prompt, template, licence gate, seed, steps, guidance and model
/// override for both generative commands.
pub(crate) struct Common {
    pub(crate) template: Template,
    pub(crate) prompt: String,
    pub(crate) negative: String,
    pub(crate) seed: u64,
    pub(crate) steps: u32,
    pub(crate) guidance: f32,
    pub(crate) models: Vec<(String, String)>,
    pub(crate) name: Option<String>,
}

pub(crate) fn plan_common(s: &Session, cmd: &str, p: &Value, default_template: &str, task: Task, default_model: &str) -> Result<Common> {
    let prompt = opt_str(cmd, p, "prompt", MAX_PROMPT_CHARS)?
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| bad(cmd, "`prompt` is required: describe what to generate"))?
        .to_string();
    let negative = opt_str(cmd, p, "negative", MAX_PROMPT_CHARS)?.unwrap_or("").to_string();
    let template_id = opt_str(cmd, p, "template", 100)?.unwrap_or(default_template);
    let template = template::find(template_id).map_err(|e| bad(cmd, e.to_string()))?;
    if template.meta.task != task {
        return Err(bad(cmd, format!("template `{template_id}` is a {:?} template, not {task:?}", template.meta.task)));
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
                .ok_or_else(|| bad(cmd, "`seed` must be a non-negative integer below 2^53"))?;
            x as u64
        }
    };
    // 0 (the generated dialog's default) means the template's own step count.
    let steps = opt_num(cmd, p, "steps", 0.0, 250.0)?.map_or(0, |x| x.round() as u32);
    let guidance = opt_num(cmd, p, "guidance", 0.0, 30.0)?.map_or(0.0, |x| x as f32);
    let name = opt_str(cmd, p, "name", 200)?.map(str::to_string);
    let mut models = Vec::new();
    let model = opt_str(cmd, p, "model", 200)?.map(str::to_string).or_else(|| Some(default_model.trim().to_string()).filter(|m| !m.is_empty()));
    if let Some(m) = model {
        let slot =
            template.meta.models.first().map(|s| s.placeholder.clone()).ok_or_else(|| bad(cmd, format!("template `{template_id}` has no model slot")))?;
        models.push((slot, m));
    }
    template.model_bindings(&models).map_err(|e| bad(cmd, e.to_string()))?;
    Ok(Common { template, prompt, negative, seed, steps, guidance, models, name })
}

/// A side length rounded down to the latent grid, within the allowed range.
fn side(cmd: &str, p: &Value, key: &str, default: u32) -> Result<u32> {
    // 0 (the generated dialog's default) means "use the default size".
    let v = match opt_num(cmd, p, key, 0.0, f64::from(MAX_SIDE))?.map(|x| x.round() as u32) {
        None | Some(0) => default,
        Some(v) if v < MIN_SIDE => return Err(bad(cmd, format!("`{key}` must be 0 (default) or {MIN_SIDE}..{MAX_SIDE} (got {v})"))),
        Some(v) => v,
    };
    Ok((v / SIZE_STEP * SIZE_STEP).clamp(MIN_SIDE, MAX_SIDE))
}

fn plan_image(s: &Session, p: &Value) -> Result<ImagePlan> {
    let (default_template, default_model) = {
        let integrations = &s.prefs().integrations;
        (template_or(&integrations.default_image_template, DEFAULT_IMAGE_TEMPLATE), integrations.default_generate_model.clone())
    };
    let Common { template, prompt, negative, seed, steps, guidance, models, name } = plan_common(s, IMAGE, p, &default_template, Task::Image, &default_model)?;
    let doc_size = s.active().map(|d| (d.doc.bounds().width(), d.doc.bounds().height()));
    let to_document = match opt_str(IMAGE, p, "target", 20)? {
        None | Some("auto") => doc_size.is_none(),
        Some("layer") => {
            if doc_size.is_none() {
                return Err(EngineError::Other("open or create a document first, or use \"target\": \"document\"".into()));
            }
            false
        }
        Some("document") => true,
        Some(other) => return Err(bad(IMAGE, format!("`target` must be \"auto\", \"layer\" or \"document\" (got `{other}`)"))),
    };
    let (dw, dh) = if to_document { (1024, 1024) } else { doc_size.unwrap_or((1024, 1024)) };
    let width = side(IMAGE, p, "width", dw)?;
    let height = side(IMAGE, p, "height", dh)?;
    let name = name.unwrap_or_else(|| format!("Generated: {}", short(&prompt)));
    Ok(ImagePlan { template, prompt, negative, seed, steps, guidance, name, models, width, height, to_document })
}

fn image_enabled(_: &Session) -> std::result::Result<(), String> {
    web_unavailable()
}

fn run_image(s: &mut Session, p: &Value) -> Result<Value> {
    let plan = plan_image(s, p)?;
    let backend = backend(s)?;
    let ImagePlan { template, prompt, negative, seed, steps, guidance, name, models, width, height, to_document } = plan;
    let label = name.clone();
    let tid = template.meta.id.clone();
    let req = Request {
        template: template.meta.id.clone(),
        prompt,
        negative,
        seed,
        steps,
        guidance,
        image: None,
        mask: None,
        models,
        size: Some((width, height)),
        params: BTreeMap::new(),
    };
    if to_document {
        let doc_name = name.clone();
        return crate::jobs::run(
            s,
            &label,
            false,
            move |ctx| {
                let resp = backend.run(&req, &JobProgress::new(ctx)).map_err(gen_err)?;
                ctx.check()?;
                Ok(resp)
            },
            move |s, resp| {
                let out = resp.images.into_iter().next().ok_or_else(|| EngineError::Other("the backend returned no image".into()))?;
                let mut doc = Document::new(doc_name, Size::new(out.width, out.height), ColorMode::Rgb, SampleType::U8);
                let mut layer = Layer::raster("Generated", doc.pixel_format());
                write_rgba8(&mut layer, &out, doc.bounds())?;
                doc.layers.push(layer);
                let index = s.add_document(doc, None);
                Ok(
                    json!({"document": index, "seed": resp.seed, "template": tid, "runId": resp.run_id, "width": out.width, "height": out.height, "ms": resp.elapsed_ms}),
                )
            },
        );
    }
    crate::jobs::edit_job(
        s,
        &label,
        move |doc, active, ctx| {
            let resp = backend.run(&req, &JobProgress::new(ctx)).map_err(gen_err)?;
            ctx.check()?;
            let out = resp.images.into_iter().next().ok_or_else(|| EngineError::Other("the backend returned no image".into()))?;
            let canvas = doc.bounds();
            let (w, h) = (canvas.width(), canvas.height());
            let out = if (out.width, out.height) == (w, h) { out } else { resize_rgba8(&out, w, h)? };
            ctx.progress(0.96, "Placing");
            let mut layer = Layer::raster(name, doc.pixel_format());
            write_rgba8(&mut layer, &out, canvas)?;
            let nid = doc.insert_above(*active, layer);
            *active = Some(nid);
            Ok((nid, resp.seed, resp.run_id, resp.elapsed_ms, w, h))
        },
        move |(nid, seed, run_id, ms, w, h)| json!({"layer": nid.0, "seed": seed, "template": tid, "runId": run_id, "width": w, "height": h, "ms": ms}),
    )
}

fn run_fill(s: &mut Session, p: &Value) -> Result<Value> {
    let plan = plan_fill(s, p)?;
    let backend = backend(s)?;
    let FillPlan { template, prompt, negative, seed, steps, guidance, name, models, rect, variations } = plan;
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
            let mut req = Request {
                template: template_id,
                prompt,
                negative,
                seed,
                steps,
                guidance,
                image: Some(image),
                mask: Some(mask),
                models,
                size: None,
                params: BTreeMap::new(),
            };
            // One backend run per variation, consecutive seeds, each its own masked layer; the
            // first is visible, the alternates sit hidden above it (`generate.variation` switches).
            let n = variations.max(1);
            let mut made: Vec<(LayerId, u64, String, u64)> = Vec::with_capacity(n as usize);
            for i in 0..n {
                req.seed = seed.wrapping_add(u64::from(i)) % MAX_SEED;
                let (lo, hi) = (0.05 + 0.9 * i as f32 / n as f32, 0.05 + 0.9 * (i + 1) as f32 / n as f32);
                if n > 1 {
                    ctx.progress(lo, &format!("Variation {}/{n}", i + 1));
                }
                let resp = backend.run(&req, &JobProgress::span(ctx, lo, hi)).map_err(gen_err)?;
                ctx.check()?;
                let out = resp.images.into_iter().next().ok_or_else(|| EngineError::Other("the backend returned no image".into()))?;
                let out = if (out.width, out.height) == (w, h) { out } else { resize_rgba8(&out, w, h)? };
                // The result becomes a layer in the document's own format, masked to the selection.
                let layer_name = if n > 1 { format!("{name} ({}/{n})", i + 1) } else { name.clone() };
                let mut layer = Layer::raster(layer_name, doc.pixel_format());
                write_rgba8(&mut layer, &out, rect)?;
                let mut mask_surface = Surface::with_default(PixelFormat::GRAY8, &[0.0]);
                mask_surface.write_region(rect, &coverage);
                layer.mask = Some(LayerMask { surface: mask_surface, enabled: true, linked: true, density: 1.0, feather: 0.0 });
                layer.visible = i == 0;
                let nid = doc.insert_above(*active, layer);
                *active = Some(nid);
                made.push((nid, resp.seed, resp.run_id, resp.elapsed_ms));
            }
            ctx.progress(0.96, "Placing");
            // The visible result is the active layer.
            *active = made.first().map(|m| m.0);
            Ok((made, w, h))
        },
        move |(made, w, h)| {
            let first = made.first();
            json!({
                "layer": first.map(|m| m.0.0), "seed": first.map(|m| m.1), "runId": first.map(|m| m.2.clone()), "template": tid,
                "layers": made.iter().map(|m| m.0.0).collect::<Vec<_>>(), "seeds": made.iter().map(|m| m.1).collect::<Vec<_>>(),
                "width": w, "height": h, "ms": made.iter().map(|m| m.3).sum::<u64>(),
            })
        },
    )
}

/// `generate.variation`: show one of the layers a fill made as variations, hide the others.
fn run_variation(s: &mut Session, p: &Value) -> Result<Value> {
    let bad_layers = || bad(VARIATION, "`layers` must be a list of 1 to 16 layer ids");
    let layers: Vec<LayerId> = p
        .get("layers")
        .and_then(Value::as_array)
        .ok_or_else(bad_layers)?
        .iter()
        .map(|v| v.as_u64().map(LayerId).ok_or_else(bad_layers))
        .collect::<Result<_>>()?;
    if layers.is_empty() || layers.len() > 16 {
        return Err(bad_layers());
    }
    let index = match opt_num(VARIATION, p, "index", 1.0, layers.len() as f64)? {
        Some(x) if x.fract() == 0.0 => x as usize,
        Some(_) => return Err(bad(VARIATION, "`index` must be a whole number (1 = the first variation)")),
        None => return Err(bad(VARIATION, "`index` is required (1 = the first variation)")),
    };
    let d = s.active().ok_or(EngineError::NoDocument)?;
    if let Some(missing) = layers.iter().find(|id| d.doc.layer(**id).is_none()) {
        return Err(EngineError::NoLayer(*missing));
    }
    let shown = layers.get(index.saturating_sub(1)).copied();
    s.edit(&format!("Variation {index}"), |doc, active| {
        for (i, id) in layers.iter().enumerate() {
            doc.layer_mut(*id).ok_or(EngineError::NoLayer(*id))?.visible = i + 1 == index;
        }
        *active = shown;
        Ok(())
    })?;
    Ok(json!({"shown": shown.map(|l| l.0), "index": index, "count": layers.len()}))
}

fn doc_enabled(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

fn run_health(s: &mut Session, _p: &Value) -> Result<Value> {
    let h = match backend(s) {
        Ok(b) => b.health(),
        Err(e) => Health { backend: "comfyui".into(), url: s.prefs().integrations.comfy_server.clone(), error: Some(e.to_string()), ..Health::default() },
    };
    serde_json::to_value(&h).map_err(|e| EngineError::Other(e.to_string()))
}

fn run_models(s: &mut Session, p: &Value) -> Result<Value> {
    let allow_research = s.prefs().integrations.allow_research_models;
    // `probe: false` lists the templates without contacting the server (the UI's picker).
    let probe = match p.get("probe") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(MODELS, "`probe` must be true or false")),
    };
    let backend = if probe { backend(s).ok() } else { None };
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
            params: r#"{"prompt":text,"negative":text,"steps":0..250=0,"guidance":0..30=0,"margin":0..1=0.25,"variations":int=1,"seed":{u64?=random},"template":{id?=Preferences › Default Fill Template},"model":{file?=Preferences},"name":{str?}} → {"layer","layers":[id],"seed","seeds":[u64],"template","runId","width","height","ms"} (steps and guidance 0 = the template's defaults; margin = context around the selection as a fraction of its larger side; variations 1..4 = results with consecutive seeds, each a layer, only the first visible; a background job: the result is a new layer above the active one, masked to the selection; needs a ComfyUI server, see Preferences › AI Integrations)"#,
            enabled: fill_enabled,
            run: run_fill,
            journal: true,
        },
        CommandSpec {
            id: IMAGE,
            label: "Generate Image…",
            menu: &[],
            shortcut: None,
            params: r#"{"prompt":text,"negative":text,"target":"auto|layer|document","width":int=0,"height":int=0,"steps":0..250=0,"guidance":0..30=0,"seed":{u64?=random},"template":{id?=Preferences › Default Image Template},"model":{file?=Preferences},"name":{str?}} → {"layer"|"document","seed","template","runId","width","height","ms"} (target auto = a layer over the whole canvas when a document is open, else a new document; width/height 0 = the document's size or 1024, otherwise 64..4096 rounded down to multiples of 16; steps and guidance 0 = the template's defaults; a background job; needs a ComfyUI server, see Preferences › AI Integrations)"#,
            enabled: image_enabled,
            run: run_image,
            journal: true,
        },
        CommandSpec {
            id: VARIATION,
            label: "Show Variation",
            menu: &[],
            shortcut: None,
            params: r#"{"layers":[id],"index":int=1} → {"shown","index","count"} (the layers a Generative Fill made as variations: shows the index-th (1-based) and hides the others, as one undo step)"#,
            enabled: doc_enabled,
            run: run_variation,
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
            params: r#"{"probe":bool=true} → {"server","templates":[{"id","name","family","task","license","allowed","defaults","models":[{"placeholder","folder","file","installed"}]}]} (probe false = list the templates without contacting the server: no server status, installed unknown)"#,
            enabled: always,
            run: run_models,
            journal: false,
        },
    ]
}

#[cfg(test)]
#[path = "generate_cmds_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "generate_image_tests.rs"]
mod image_tests;

#[cfg(test)]
#[path = "generate_template_tests.rs"]
mod template_tests;
