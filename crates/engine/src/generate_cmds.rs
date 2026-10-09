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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use photocraft_algo::selection::{self as sel, SelectionMode};
use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Document, Layer, LayerId, LayerMask, Size};
use photocraft_genai::template::{self, License, Template};
use photocraft_genai::{GenerativeBackend, Gray8, Health, Request, Rgba8, Task};
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, layer_param};
use crate::jobs::JobCtx;
use crate::{EngineError, Result, Session};

pub const FILL: &str = "generate.fill";
pub const IMAGE: &str = "generate.image";
pub const HEALTH: &str = "generate.health";
pub const MODELS: &str = "generate.models";
pub const VARIATION: &str = "generate.variation";
pub const FREE: &str = "generate.free";
pub const EXPAND: &str = "generate.expand";
/// Most results one `generate.fill` call makes (`variations`).
pub const MAX_VARIATIONS: u32 = 4;
/// The template value (and preference) that picks a fill template by what the server has.
pub const AUTO_TEMPLATE: &str = "auto";
/// `auto` tries these in order and takes the first whose model files the server lists: the
/// Lightning tier (8 steps, Apache-2.0 LoRA) when its LoRA is installed, else the 40-step base.
pub const AUTO_FILL_ORDER: &[&str] = &["qwen-edit-2511/fill-lightning-8", DEFAULT_FILL_TEMPLATE];
/// The same for Generative Expand, whose templates show the model only the picture as a
/// reference while the padded canvas is the sampling latent.
pub const DEFAULT_EXPAND_TEMPLATE: &str = "qwen-edit-2511/expand";
pub const AUTO_EXPAND_ORDER: &[&str] = &["qwen-edit-2511/expand-lightning-8", DEFAULT_EXPAND_TEMPLATE];
/// A fill request is downscaled to this many pixels before it is sent (the editing models work
/// at about one megapixel, Photoshop's Generative Fill renders at most 1024 px on a side) and
/// the result is resampled back; the layer mask keeps the selection's full-resolution edge.
/// 0.75 MP rather than 1 MP: on a 32 GB card with Qwen-Image-Edit-2511 fp8 and its 7.9 GB text
/// encoder resident, a 1 MP latent plus the 1 MP reference pushed the server into offloading
/// part of the model (a 45 s run instead of 18 s); at 0.75 MP it stays resident.
const MAX_FILL_REQUEST_PIXELS: u64 = 768 * 1024;
/// A small request is upscaled until its longer side is this long: the 2511 templates sample at
/// the request's own size, and a handful of latent tokens cannot carry structure.
const MIN_FILL_REQUEST_SIDE: u32 = 512;
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

pub(crate) const REMOVE_BG: &str = "generate.removeBackground";
pub(crate) const DEFAULT_MATTE_TEMPLATE: &str = "qwen-2.1/matte";
/// ComfyUI's official background-removal instruction for Qwen-Image-2.1, sent when the prompt
/// does not name a subject (it opens with an imperative verb, so the template passes it through).
pub(crate) const MATTE_DEFAULT_PROMPT: &str = "Remove the background, and output a PNG image";
/// A matte request goes out at up to this many pixels: the model's own working size, and the
/// matte only needs to be resampled back, not the pixels.
const MAX_MATTE_REQUEST_PIXELS: u64 = 1024 * 1024;

pub(crate) const INFO: &str = "generate.info";
pub(crate) const SIMILAR: &str = "generate.similar";

/// The PSD additional-layer-info key a generative layer keeps its [`GenerativeInfo`] under
/// (JSON). An unmodelled block, so it survives PSD round trips and `Layer::duplicate`.
pub const GENERATIVE_BLOCK: [u8; 4] = *b"cpGn";

/// What a generative layer remembers about the run that made it: `generate.info` reads it,
/// `generate.similar` re-runs it with a new seed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GenerativeInfo {
    /// `generate.fill`, `generate.expand`, `generate.edit` or `generate.image`.
    pub command: String,
    /// The prompt as typed (the template's wrapper is applied again on a re-run).
    pub prompt: String,
    pub negative: String,
    /// The template that ran (`auto` resolved).
    pub template: String,
    pub seed: u64,
    pub steps: u32,
    pub guidance: f32,
    pub edge: String,
    /// The request rectangle on the canvas: x, y, width, height.
    pub rect: [i32; 4],
    pub name: String,
    /// `generate.image` only: the size asked for and whether transparency was.
    pub width: u32,
    pub height: u32,
    pub transparent: bool,
}

/// Remember `info` on `layer` (replacing an earlier one).
pub(crate) fn set_generative_info(layer: &mut Layer, info: &GenerativeInfo) {
    layer.psd_blocks.retain(|(k, _)| *k != GENERATIVE_BLOCK);
    if let Ok(bytes) = serde_json::to_vec(info) {
        layer.psd_blocks.push((GENERATIVE_BLOCK, Arc::new(bytes)));
    }
}

/// The run that made `layer`, if a generative command did.
pub fn generative_info(layer: &Layer) -> Option<GenerativeInfo> {
    layer.psd_blocks.iter().find(|(k, _)| *k == GENERATIVE_BLOCK).and_then(|(_, d)| serde_json::from_slice(d).ok())
}

pub(crate) const EDIT: &str = "generate.edit";
pub(crate) const DEFAULT_EDIT_TEMPLATE: &str = "qwen-edit-2511/edit";
/// What `auto` tries for an edit, fastest permissive tier first.
pub(crate) const AUTO_EDIT_ORDER: &[&str] = &["qwen-edit-2511/edit-lightning-8", DEFAULT_EDIT_TEMPLATE];

/// The model files (`folder/file`) the last run on each server loaded, for [`run_switching`].
static LAST_MODELS: Mutex<BTreeMap<String, BTreeSet<String>>> = Mutex::new(BTreeMap::new());

/// The server a session's generative requests go to: the key of [`LAST_MODELS`].
pub(crate) fn server_key(s: &Session) -> String {
    s.prefs().integrations.comfy_server.trim().to_string()
}

/// The model files (`folder/file`) a request loads: the template's slots with the request's
/// overrides.
fn model_set(req: &Request) -> BTreeSet<String> {
    let Ok(t) = template::find(&req.template) else { return BTreeSet::new() };
    let bound = t.model_bindings(&req.models).unwrap_or_default();
    t.meta.models.iter().map(|slot| format!("{}/{}", slot.folder, bound.get(&slot.placeholder).and_then(Value::as_str).unwrap_or(&slot.default))).collect()
}

/// Run `req` on `backend`, first asking the server to unload its models when the request's
/// model files differ from what the previous run on `server` loaded. ComfyUI keeps the earlier
/// models resident and, short of memory for the new set, loads it partially and streams the
/// rest on every step: the first run after a switch between the 2511 base and its Lightning
/// tier, or from 2511 to Qwen-Image-2.1, took 31–131 s on the 32 GB card against 13–20 s for
/// the runs after it. A purge costs one reload instead. The first run on a server never purges.
pub(crate) fn run_switching(
    backend: &dyn GenerativeBackend,
    server: &str,
    req: &Request,
    progress: &JobProgress,
) -> photocraft_genai::Result<photocraft_genai::Response> {
    let set = model_set(req);
    let switch = !set.is_empty() && LAST_MODELS.lock().unwrap_or_else(PoisonError::into_inner).get(server).is_some_and(|last| *last != set);
    if switch {
        photocraft_genai::Progress::report(progress, 0.0, "Unloading the previous models");
        // A server that cannot purge just runs as before.
        let _ = backend.free();
    }
    let resp = backend.run(req, progress)?;
    if !set.is_empty() {
        LAST_MODELS.lock().unwrap_or_else(PoisonError::into_inner).insert(server.to_string(), set);
    }
    Ok(resp)
}

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
    /// Pick the template inside the job from what the server has (`AUTO_FILL_ORDER`).
    auto: bool,
    /// How the result's layer mask meets the surroundings.
    edge: Edge,
}

/// How a generated layer's mask ends: softly, across the band the model re-rendered around the
/// selection, with per-pixel noise so the ramp reads as grain rather than a gradient or a line
/// (the default), or hard, exactly on the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Soft,
    Hard,
}

fn parse_edge(cmd: &str, p: &Value) -> Result<Edge> {
    match opt_str(cmd, p, "edge", 10)? {
        None | Some("soft") => Ok(Edge::Soft),
        Some("hard") => Ok(Edge::Hard),
        Some(other) => Err(bad(cmd, format!("`edge` must be soft or hard (got `{other}`)"))),
    }
}

/// The layer mask of a soft edge: the feathered request mask, its ramp dithered with
/// per-pixel noise (zero inside and outside, strongest mid-ramp) so the transition has no
/// visible line and no banding. Deterministic for a seed, so variations differ.
fn dither_edge(mask: &[u8], seed: u64) -> Vec<f32> {
    mask.iter()
        .enumerate()
        .map(|(i, v)| {
            let m = f32::from(*v) / 255.0;
            if *v == 0 || *v == 255 {
                return m;
            }
            // splitmix64 of the pixel index and the seed → a uniform value in 0..1.
            let mut z = (i as u64).wrapping_add(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            let n = (z >> 40) as f32 / (1u64 << 24) as f32;
            let amplitude = 0.6 * (1.0 - (2.0 * m - 1.0).abs());
            (m + (n - 0.5) * amplitude).clamp(0.0, 1.0)
        })
        .collect()
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

/// An optional boolean parameter: absent or null = `default`.
pub(crate) fn opt_bool(cmd: &str, p: &Value, key: &str, default: bool) -> Result<bool> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(bad(cmd, format!("`{key}` must be true or false"))),
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
        (template_or(&integrations.default_fill_template, AUTO_TEMPLATE), integrations.default_edit_model.clone())
    };
    // `auto` (the preference's default) is resolved inside the job from the server's model
    // list; the plan validates the base template it falls back to.
    let requested = opt_str(FILL, p, "template", 200)?.map(str::to_string).unwrap_or(default_template);
    let auto = requested == AUTO_TEMPLATE;
    let mut q = p.clone();
    if auto && let Some(o) = q.as_object_mut() {
        o.remove("template");
    }
    let Common { template, prompt, negative, seed, steps, guidance, models, name } =
        plan_common(s, FILL, &q, if auto { DEFAULT_FILL_TEMPLATE } else { &requested }, Task::Fill, &default_model)?;
    let margin = opt_num(FILL, p, "margin", 0.0, 1.0)?.unwrap_or(0.25);
    let edge = parse_edge(FILL, p)?;
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
    // Sides on the 16-px grid the VAE and the patch grid want, so a template that samples at
    // the request's own size needs no padding and the result needs no resampling.
    let rect = align_to_grid(rect, canvas);
    let pixels = u64::from(rect.width()) * u64::from(rect.height());
    if pixels > MAX_REQUEST_PIXELS {
        return Err(EngineError::Other(format!(
            "the selected area with its margin is {}×{} pixels; Generative Fill handles up to 16 megapixels at a time: select a smaller area or lower `margin`",
            rect.width(),
            rect.height()
        )));
    }
    let name = name.unwrap_or_else(|| format!("Generative Fill: {}", short(&prompt)));
    Ok(FillPlan { template, prompt, negative, seed, steps, guidance, name, models, rect, variations, auto, edge })
}

/// The first of `candidates` whose model files the server lists (one `/models/<folder>` call per
/// folder), else `fallback`. Only permissive templates belong in `candidates`.
pub(crate) fn resolve_auto(backend: &dyn GenerativeBackend, candidates: &[&str], fallback: &str) -> String {
    let mut folders: BTreeMap<String, Option<Vec<String>>> = BTreeMap::new();
    for id in candidates {
        let Ok(t) = template::find(id) else { continue };
        let installed = t.meta.models.iter().all(|slot| {
            let files = folders.entry(slot.folder.clone()).or_insert_with(|| backend.model_files(&slot.folder).ok());
            files.as_ref().is_some_and(|f| f.iter().any(|x| x == &slot.default))
        });
        if installed {
            return (*id).to_string();
        }
    }
    fallback.to_string()
}

/// Grow `rect` inside `canvas` until its sides are multiples of [`SIZE_STEP`], extending right
/// and down first, then left and up. A canvas too small for a side leaves that side as it is
/// (the server crops to its grid and the result is resampled back).
pub(crate) fn align_to_grid(rect: Rect, canvas: Rect) -> Rect {
    let step = SIZE_STEP as i32;
    let grow = |lo: i32, hi: i32, clo: i32, chi: i32| -> (i32, i32) {
        let len = (hi - lo).max(1) as u32;
        let want = (len.div_ceil(SIZE_STEP) * SIZE_STEP) as i32;
        if want > chi - clo {
            return (lo, hi);
        }
        let hi2 = (lo + want).min(chi);
        let lo2 = (hi2 - want).max(clo);
        (lo2, lo2 + want)
    };
    let _ = step;
    let (x0, x1) = grow(rect.x0, rect.x1, canvas.x0, canvas.x1);
    let (y0, y1) = grow(rect.y0, rect.y1, canvas.y0, canvas.y1);
    Rect::new(x0, y0, x1, y1)
}

/// The size a `w`×`h` fill request is sent at: scaled down (aspect kept, multiples of 16) over
/// [`MAX_FILL_REQUEST_PIXELS`], scaled up when the longer side is under
/// [`MIN_FILL_REQUEST_SIDE`], otherwise unchanged.
pub(crate) fn request_size(w: u32, h: u32) -> (u32, u32) {
    let (w, h) = fit_pixels(w, h, MAX_FILL_REQUEST_PIXELS);
    let longer = w.max(h);
    if longer == 0 || longer >= MIN_FILL_REQUEST_SIDE {
        return (w, h);
    }
    let scale = f64::from(MIN_FILL_REQUEST_SIDE) / f64::from(longer);
    let fit = |v: u32| (((f64::from(v) * scale).round() as u32) / SIZE_STEP * SIZE_STEP).max(MIN_SIDE);
    (fit(w), fit(h))
}

/// `w`×`h` shrunk (aspect kept, multiples of 16, at least 64 on a side) until it has at most
/// `max_pixels` pixels; unchanged when it already fits.
pub(crate) fn fit_pixels(w: u32, h: u32, max_pixels: u64) -> (u32, u32) {
    let pixels = u64::from(w) * u64::from(h);
    if pixels <= max_pixels || w == 0 || h == 0 {
        return (w, h);
    }
    let scale = (max_pixels as f64 / pixels as f64).sqrt();
    let fit = |v: u32| (((f64::from(v) * scale) as u32) / SIZE_STEP * SIZE_STEP).max(MIN_SIDE);
    (fit(w), fit(h))
}

/// Lanczos-3 kernel, widened by `scale` (> 1 when shrinking) so downsampling averages instead of
/// aliasing.
fn lanczos3(x: f32) -> f32 {
    let x = x.abs();
    if x < 1e-6 {
        1.0
    } else if x >= 3.0 {
        0.0
    } else {
        let px = std::f32::consts::PI * x;
        3.0 * px.sin() * (px / 3.0).sin() / (px * px)
    }
}

/// Triangle (tent) kernel: no negative lobes, for masks.
fn triangle(x: f32) -> f32 {
    (1.0 - x.abs()).max(0.0)
}

/// A resampling kernel and its radius in source pixels (at 1:1).
#[derive(Clone, Copy)]
struct Filter {
    kernel: fn(f32) -> f32,
    radius: f32,
}
const LANCZOS: Filter = Filter { kernel: lanczos3, radius: 3.0 };
const TENT: Filter = Filter { kernel: triangle, radius: 1.0 };

/// Per-output-index taps of a separable resampler: (first source index, normalised weights).
fn taps(src_len: usize, dst_len: usize, f: Filter) -> Vec<(usize, Vec<f32>)> {
    let scale = src_len as f32 / dst_len as f32;
    let widen = scale.max(1.0);
    let support = f.radius * widen;
    (0..dst_len)
        .map(|i| {
            let centre = (i as f32 + 0.5) * scale - 0.5;
            let lo = ((centre - support).floor().max(0.0)) as usize;
            let hi = ((centre + support).ceil() as usize).min(src_len.saturating_sub(1));
            let mut weights: Vec<f32> = (lo..=hi).map(|j| (f.kernel)((j as f32 - centre) / widen)).collect();
            let sum: f32 = weights.iter().sum();
            if sum.abs() > 1e-6 {
                for w in &mut weights {
                    *w /= sum;
                }
            } else if let Some(w) = weights.first_mut() {
                *w = 1.0;
            }
            (lo, weights)
        })
        .collect()
}

/// Separable resample of interleaved 8-bit samples (`ch` per pixel) from `sw`×`sh` to `dw`×`dh`.
fn resample_u8(src: &[u8], sw: usize, sh: usize, ch: usize, dw: usize, dh: usize, f: Filter) -> Vec<u8> {
    let xt = taps(sw, dw, f);
    let yt = taps(sh, dh, f);
    // Horizontal pass into f32 rows, then vertical.
    let mut mid = vec![0.0f32; dw * sh * ch];
    for y in 0..sh {
        let row = &src[y * sw * ch..((y + 1) * sw * ch).min(src.len())];
        for (x, (lo, ws)) in xt.iter().enumerate() {
            let out = &mut mid[(y * dw + x) * ch..(y * dw + x + 1) * ch];
            for (k, w) in ws.iter().enumerate() {
                let start = ((lo + k) * ch).min(row.len().saturating_sub(ch));
                for (o, p) in out.iter_mut().zip(row.iter().skip(start)) {
                    *o += w * f32::from(*p);
                }
            }
        }
    }
    let mut dst = vec![0u8; dw * dh * ch];
    for (y, (lo, ws)) in yt.iter().enumerate() {
        for x in 0..dw {
            for c in 0..ch {
                let mut acc = 0.0f32;
                for (k, w) in ws.iter().enumerate() {
                    acc += w * mid.get(((lo + k) * dw + x) * ch + c).copied().unwrap_or(0.0);
                }
                if let Some(d) = dst.get_mut((y * dw + x) * ch + c) {
                    *d = (acc + 0.5).clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    dst
}

/// Lanczos-3 resize (both ways), for requests over the size cap and for a backend that returns
/// a different size than it was given.
pub(crate) fn resize_rgba8(src: &Rgba8, w: u32, h: u32) -> Result<Rgba8> {
    if src.width == 0 || src.height == 0 || w == 0 || h == 0 {
        return Err(EngineError::Other("cannot resize an empty image".into()));
    }
    if (src.width, src.height) == (w, h) {
        return Ok(src.clone());
    }
    let out = resample_u8(&src.data, src.width as usize, src.height as usize, 4, w as usize, h as usize, LANCZOS);
    Rgba8::new(w, h, out).map_err(gen_err)
}

/// Soften a mask outward: full coverage at its edge falling to nothing about `2 × radius` pixels
/// beyond it, the inside untouched (two box blurs ≈ a triangle filter, the outer half of its
/// ramp doubled, then `max` with the original). The same shape ComfyUI's GrowMask + FeatherMask
/// make. As the latent noise mask it has the model re-render the band around the area and blend
/// it, with no step at the edge: a ramp that started at half coverage there (the first version)
/// left half of the tone difference as a line along the edge. As the layer mask (dithered, see
/// `dither_edge`) it fades the result into its surroundings across the same band.
pub(crate) fn feather_outward(mask: &Gray8, radius: usize) -> Result<Gray8> {
    let (w, h) = (mask.width as usize, mask.height as usize);
    if radius == 0 || w == 0 || h == 0 {
        return Ok(mask.clone());
    }
    let mut cur: Vec<f32> = mask.data.iter().map(|v| f32::from(*v)).collect();
    for _ in 0..2 {
        cur = box_blur(&cur, w, h, radius, true);
        cur = box_blur(&cur, w, h, radius, false);
    }
    let out: Vec<u8> = cur.iter().zip(&mask.data).map(|(b, o)| (*o).max((b * 2.0 + 0.5).clamp(0.0, 255.0) as u8)).collect();
    Gray8::new(mask.width, mask.height, out).map_err(gen_err)
}

/// One box-blur pass of radius `r` along rows (`horizontal`) or columns, edges clamped.
fn box_blur(src: &[f32], w: usize, h: usize, r: usize, horizontal: bool) -> Vec<f32> {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let at = |line: usize, i: usize| -> f32 {
        let idx = if horizontal { line * w + i } else { i * w + line };
        src.get(idx).copied().unwrap_or(0.0)
    };
    let mut out = vec![0.0f32; src.len()];
    let window = (2 * r + 1) as f32;
    for line in 0..lines {
        // Running sum over a window clamped to the line's ends.
        let mut sum = 0.0f32;
        for i in 0..=r.min(len.saturating_sub(1)) {
            sum += at(line, i);
        }
        sum += at(line, 0) * r as f32;
        for i in 0..len {
            let idx = if horizontal { line * w + i } else { i * w + line };
            if let Some(o) = out.get_mut(idx) {
                *o = sum / window;
            }
            let leaving = at(line, i.saturating_sub(r));
            let entering = at(line, (i + r + 1).min(len.saturating_sub(1)));
            sum += entering - leaving;
        }
    }
    out
}

/// The outward feather of a fill's request mask: 4 % of the longer side, 6 to 48 pixels. The
/// model re-renders that band around the selection and the soft layer mask fades across it.
pub(crate) fn feather_radius(w: u32, h: u32) -> usize {
    ((f64::from(w.max(h)) * 0.04).round() as usize).clamp(6, 48)
}

/// The feather of an expand's request mask into the picture: 4 % of the longer side, 8 to 80
/// pixels (a band of twice that). The model re-renders that band of the original, so the new
/// area's tone is carried across the old edge instead of meeting it. Measured on the 1024-px
/// lighthouse: at 8 % the model took the licence to move the horizon inside the band (a ghosted
/// double horizon at one seed), at 2 % the old edge still read as a line; 4 % did neither.
pub(crate) fn outpaint_feather_radius(w: u32, h: u32) -> usize {
    ((f64::from(w.max(h)) * 0.04).round() as usize).clamp(8, 80)
}

/// Mask resize with a tent filter (no ringing, coverage stays within 0..=255).
pub(crate) fn resize_gray8(src: &Gray8, w: u32, h: u32) -> Result<Gray8> {
    if src.width == 0 || src.height == 0 || w == 0 || h == 0 {
        return Err(EngineError::Other("cannot resize an empty mask".into()));
    }
    if (src.width, src.height) == (w, h) {
        return Ok(src.clone());
    }
    let out = resample_u8(&src.data, src.width as usize, src.height as usize, 1, w as usize, h as usize, TENT);
    Gray8::new(w, h, out).map_err(gen_err)
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
    /// The prompt the model gets (wrapped for transparency when asked).
    prompt: String,
    /// The prompt as typed, remembered on the layer.
    typed: String,
    transparent: bool,
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
    let seed = parse_seed(cmd, p)?;
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

/// `seed`: absent or null = random; else a whole number below 2^53.
pub(crate) fn parse_seed(cmd: &str, p: &Value) -> Result<u64> {
    match p.get("seed") {
        None | Some(Value::Null) => Ok(photocraft_genai::random_seed()),
        Some(v) => {
            let x = v
                .as_f64()
                .filter(|x| x.is_finite() && *x >= 0.0 && x.fract() == 0.0 && *x < 9_007_199_254_740_992.0)
                .ok_or_else(|| bad(cmd, "`seed` must be a non-negative integer below 2^53"))?;
            Ok(x as u64)
        }
    }
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
    // A transparent result: the template says how its model is asked for one (Qwen-Image-2.1
    // decodes RGBA); the layer keeps the alpha the PNG comes back with.
    let transparent = opt_bool(IMAGE, p, "transparent", false)?;
    let typed = prompt.clone();
    let prompt = if transparent {
        let format = template.meta.prompt_format_transparent.trim();
        if format.is_empty() {
            return Err(bad(IMAGE, format!("template `{}` cannot output transparency; `qwen-2.1/image` can", template.meta.id)));
        }
        format.replace("{prompt}", &prompt)
    } else {
        prompt
    };
    Ok(ImagePlan { template, prompt, typed, transparent, negative, seed, steps, guidance, name, models, width, height, to_document })
}

fn image_enabled(_: &Session) -> std::result::Result<(), String> {
    web_unavailable()
}

fn run_image(s: &mut Session, p: &Value) -> Result<Value> {
    let plan = plan_image(s, p)?;
    let backend = backend(s)?;
    let server = server_key(s);
    let server_doc = server.clone();
    let ImagePlan { template, prompt, typed, transparent, negative, seed, steps, guidance, name, models, width, height, to_document } = plan;
    let label = name.clone();
    let tid = template.meta.id.clone();
    // What the result layer remembers (`generate.similar` re-runs it).
    let info = GenerativeInfo {
        command: IMAGE.to_string(),
        prompt: typed,
        negative: negative.clone(),
        template: tid.clone(),
        seed,
        steps,
        guidance,
        name: name.clone(),
        width,
        height,
        transparent,
        ..GenerativeInfo::default()
    };
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
                let resp = run_switching(backend.as_ref(), &server_doc, &req, &JobProgress::new(ctx)).map_err(gen_err)?;
                ctx.check()?;
                Ok(resp)
            },
            move |s, resp| {
                let out = resp.images.into_iter().next().ok_or_else(|| EngineError::Other("the backend returned no image".into()))?;
                let mut doc = Document::new(doc_name, Size::new(out.width, out.height), ColorMode::Rgb, SampleType::U8);
                let mut layer = Layer::raster("Generated", doc.pixel_format());
                write_rgba8(&mut layer, &out, doc.bounds())?;
                set_generative_info(&mut layer, &GenerativeInfo { seed: resp.seed, rect: [0, 0, out.width as i32, out.height as i32], ..info });
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
            let resp = run_switching(backend.as_ref(), &server, &req, &JobProgress::new(ctx)).map_err(gen_err)?;
            ctx.check()?;
            let out = resp.images.into_iter().next().ok_or_else(|| EngineError::Other("the backend returned no image".into()))?;
            let canvas = doc.bounds();
            let (w, h) = (canvas.width(), canvas.height());
            let out = if (out.width, out.height) == (w, h) { out } else { resize_rgba8(&out, w, h)? };
            ctx.progress(0.96, "Placing");
            let mut layer = Layer::raster(name, doc.pixel_format());
            write_rgba8(&mut layer, &out, canvas)?;
            set_generative_info(&mut layer, &GenerativeInfo { seed: resp.seed, rect: [canvas.x0, canvas.y0, w as i32, h as i32], ..info });
            let nid = doc.insert_above(*active, layer);
            *active = Some(nid);
            Ok((nid, resp.seed, resp.run_id, resp.elapsed_ms, w, h))
        },
        move |(nid, seed, run_id, ms, w, h)| json!({"layer": nid.0, "seed": seed, "template": tid, "runId": run_id, "width": w, "height": h, "ms": ms}),
    )
}

/// What a fill or expand job needs once its plan is made and the backend is in hand.
struct FillJob {
    backend: Arc<dyn GenerativeBackend>,
    /// The server's key for [`run_switching`].
    server: String,
    /// The command the layers remember ([`GenerativeInfo::command`]).
    command: &'static str,
    template_id: String,
    auto: bool,
    /// The candidates `auto` picks from ([`AUTO_FILL_ORDER`] or [`AUTO_EXPAND_ORDER`]).
    auto_order: &'static [&'static str],
    prompt: String,
    negative: String,
    seed: u64,
    steps: u32,
    guidance: f32,
    name: String,
    models: Vec<(String, String)>,
    variations: u32,
    edge: Edge,
}

/// The layers a job made: (id, seed, run id, ms, timings), the primary one first.
type Made = Vec<(LayerId, u64, String, u64, photocraft_genai::Timings)>;
/// A job's outcome: the layers, the rect's size, the request's size and the template used.
type Filled = (Made, u32, u32, (u32, u32), String);

/// Regenerate the pixels `coverage` marks (one value per pixel of `rect`) through the backend
/// Generative Expand's extras for [`fill_region`]: the part of the request that holds the
/// picture (the rest is empty canvas, painted mid-grey so the model sees "nothing here yet"
/// rather than black or transparent), and a wider blend: the result layer's mask is the soft
/// request mask, so the band of the picture the model re-rendered fades into the original
/// instead of meeting it at a hard line (the classic outpainting seam).
struct Outpaint {
    picture: Rect,
}

/// Regenerate the pixels `coverage` marks (one value per pixel of `rect`) through the backend
/// and add the results as layers above the active one, each masked to `coverage` (or to the
/// soft request mask when `outpaint` is set).
fn fill_region(
    doc: &mut Document,
    active: &mut Option<LayerId>,
    ctx: &JobCtx,
    job: FillJob,
    rect: Rect,
    coverage: Vec<f32>,
    outpaint: Option<Outpaint>,
) -> Result<Filled> {
    ctx.progress(0.0, "Rendering");
    let (w, h) = (rect.width(), rect.height());
    let composite = photocraft_compose::render(doc, rect).to_rgba8();
    let mut image = Rgba8::new(composite.width, composite.height, composite.pixels).map_err(gen_err)?;
    if let Some(o) = &outpaint {
        let p = o.picture;
        prefill_outside(&mut image, Rect::new(p.x0 - rect.x0, p.y0 - rect.y0, p.x1 - rect.x0, p.y1 - rect.y0));
    }
    let FillJob { backend, server, command, template_id, auto, auto_order, prompt, negative, seed, steps, guidance, name, models, variations, edge } = job;
    let mask = Gray8::new(w, h, coverage.iter().map(|c| (c * 255.0 + 0.5) as u8).collect()).map_err(gen_err)?;
    // The model gets a softened mask so it re-renders a band around the area and blends it (much
    // wider for an expand, whose seam runs along a whole picture edge and whose sky or water tone
    // has to carry across it).
    let radius = if outpaint.is_some() { outpaint_feather_radius(w, h) } else { feather_radius(w, h) };
    let mask = feather_outward(&mask, radius)?;
    // The layer mask: that same band, dithered, so the result fades into its surroundings with
    // no line; or exactly the selection when the caller asked for a hard edge.
    let layer_coverage: Vec<f32> = match edge {
        Edge::Soft => dither_edge(&mask.data, seed),
        Edge::Hard => coverage,
    };
    ctx.check()?;
    // Large areas go out at the models' working size and come back resampled; the layer mask
    // below keeps the selection's full-resolution edge either way.
    let (rw, rh) = request_size(w, h);
    let (image, mask) = if (rw, rh) == (w, h) { (image, mask) } else { (resize_rgba8(&image, rw, rh)?, resize_gray8(&mask, rw, rh)?) };
    let template_id = if auto { resolve_auto(backend.as_ref(), auto_order, &template_id) } else { template_id };
    // An expand template crops the model's reference to the picture: its rectangle inside the
    // request, in the request's (possibly resampled) pixels.
    let mut params = BTreeMap::new();
    if let Some(o) = &outpaint {
        let (sx, sy) = (f64::from(rw) / f64::from(w.max(1)), f64::from(rh) / f64::from(h.max(1)));
        let p = o.picture;
        let x0 = (f64::from(p.x0 - rect.x0) * sx).floor().clamp(0.0, f64::from(rw.saturating_sub(16)));
        let y0 = (f64::from(p.y0 - rect.y0) * sy).floor().clamp(0.0, f64::from(rh.saturating_sub(16)));
        let x1 = (f64::from(p.x1 - rect.x0) * sx).ceil().clamp(x0 + 16.0, f64::from(rw));
        let y1 = (f64::from(p.y1 - rect.y0) * sy).ceil().clamp(y0 + 16.0, f64::from(rh));
        params.insert("ref_x".to_string(), json!(x0 as u32));
        params.insert("ref_y".to_string(), json!(y0 as u32));
        params.insert("ref_w".to_string(), json!((x1 - x0) as u32));
        params.insert("ref_h".to_string(), json!((y1 - y0) as u32));
    }
    // An edit template has no noise mask: the picture goes alone and the selection only masks
    // the result layer.
    let wants_mask = template::find(&template_id).map(|t| t.meta.needs_mask).unwrap_or(true);
    let mut req = Request {
        template: template_id.clone(),
        prompt,
        negative,
        seed,
        steps,
        guidance,
        image: Some(image),
        mask: wants_mask.then_some(mask),
        models,
        size: None,
        params,
    };
    // One backend run per variation, consecutive seeds, each its own masked layer; the first
    // is visible, the alternates sit hidden above it (`generate.variation` switches).
    let n = variations.max(1);
    let mut made: Made = Vec::with_capacity(n as usize);
    for i in 0..n {
        req.seed = seed.wrapping_add(u64::from(i)) % MAX_SEED;
        let (lo, hi) = (0.05 + 0.9 * i as f32 / n as f32, 0.05 + 0.9 * (i + 1) as f32 / n as f32);
        if n > 1 {
            ctx.progress(lo, &format!("Variation {}/{n}", i + 1));
        }
        let resp = run_switching(backend.as_ref(), &server, &req, &JobProgress::span(ctx, lo, hi)).map_err(gen_err)?;
        ctx.check()?;
        let out = resp.images.into_iter().next().ok_or_else(|| EngineError::Other("the backend returned no image".into()))?;
        let out = if (out.width, out.height) == (w, h) { out } else { resize_rgba8(&out, w, h)? };
        // The result becomes a layer in the document's own format, masked to the coverage.
        let layer_name = if n > 1 { format!("{name} ({}/{n})", i + 1) } else { name.clone() };
        let mut layer = Layer::raster(layer_name, doc.pixel_format());
        write_rgba8(&mut layer, &out, rect)?;
        let mut mask_surface = Surface::with_default(PixelFormat::GRAY8, &[0.0]);
        mask_surface.write_region(rect, &layer_coverage);
        layer.mask = Some(LayerMask { surface: mask_surface, enabled: true, linked: true, density: 1.0, feather: 0.0 });
        set_generative_info(
            &mut layer,
            &GenerativeInfo {
                command: command.to_string(),
                prompt: req.prompt.clone(),
                negative: req.negative.clone(),
                template: template_id.clone(),
                seed: req.seed,
                steps: req.steps,
                guidance: req.guidance,
                edge: match edge {
                    Edge::Soft => "soft".into(),
                    Edge::Hard => "hard".into(),
                },
                rect: [rect.x0, rect.y0, rect.width() as i32, rect.height() as i32],
                name: name.clone(),
                ..GenerativeInfo::default()
            },
        );
        layer.visible = i == 0;
        let nid = doc.insert_above(*active, layer);
        *active = Some(nid);
        made.push((nid, resp.seed, resp.run_id, resp.elapsed_ms, resp.timings));
    }
    ctx.progress(0.96, "Placing");
    // The visible result is the active layer.
    *active = made.first().map(|m| m.0);
    Ok((made, w, h, (rw, rh), template_id))
}

/// The JSON a fill or expand job answers with.
fn made_json(made: &Made, w: u32, h: u32, (rw, rh): (u32, u32), template_id: &str) -> Value {
    let first = made.first();
    json!({
        "layer": first.map(|m| m.0.0), "seed": first.map(|m| m.1), "runId": first.map(|m| m.2.clone()), "template": template_id,
        "layers": made.iter().map(|m| m.0.0).collect::<Vec<_>>(), "seeds": made.iter().map(|m| m.1).collect::<Vec<_>>(),
        "width": w, "height": h, "requestWidth": rw, "requestHeight": rh,
        "ms": made.iter().map(|m| m.3).sum::<u64>(),
        "timings": made.iter().map(|m| serde_json::to_value(m.4).unwrap_or(Value::Null)).collect::<Vec<_>>(),
    })
}

/// What empty canvas is painted with when there is no picture to continue: mid-grey.
const PREFILL: [u8; 4] = [128, 128, 128, 255];

/// Paint the pixels outside `inner` (in image coordinates) with the nearest pixel of `inner`, so
/// the canvas the VAE encodes continues the picture's edge colours instead of meeting a grey
/// wall. The model never sees this area as content (its reference is the cropped picture, see
/// `Outpaint`, and the noise mask is full over it), but the VAE's latents next to a wall of grey
/// carry its tone into the picture's edge band, which came back as a dark line along the old
/// edge. The replicated streaks are what the early padded-reference variant turned into
/// content; with the reference cropped they are only ever noised away.
fn prefill_outside(img: &mut Rgba8, inner: Rect) {
    let (w, h) = (img.width as i32, img.height as i32);
    let inner = inner.intersect(&Rect::new(0, 0, w, h));
    if inner.x0 == 0 && inner.y0 == 0 && inner.x1 == w && inner.y1 == h {
        return;
    }
    let empty = inner.x1 <= inner.x0 || inner.y1 <= inner.y0;
    let stride = img.width as usize * 4;
    for y in 0..h {
        let sy = if empty { 0 } else { y.clamp(inner.y0, inner.y1 - 1) };
        for x in 0..w {
            if x >= inner.x0 && x < inner.x1 && y >= inner.y0 && y < inner.y1 {
                continue;
            }
            let sx = if empty { 0 } else { x.clamp(inner.x0, inner.x1 - 1) };
            let src = sy as usize * stride + sx as usize * 4;
            let p = match img.data.get(src..src + 4) {
                Some(s) if !empty => [s[0], s[1], s[2], 255],
                _ => PREFILL,
            };
            let dst = y as usize * stride + x as usize * 4;
            if let Some(d) = img.data.get_mut(dst..dst + 4) {
                d.copy_from_slice(&p);
            }
        }
    }
}

fn run_fill(s: &mut Session, p: &Value) -> Result<Value> {
    let plan = plan_fill(s, p)?;
    let backend = backend(s)?;
    let FillPlan { template, prompt, negative, seed, steps, guidance, name, models, rect, variations, auto, edge } = plan;
    let label = name.clone();
    let job = FillJob {
        backend,
        server: server_key(s),
        command: FILL,
        template_id: template.meta.id.clone(),
        auto,
        auto_order: AUTO_FILL_ORDER,
        prompt,
        negative,
        seed,
        steps,
        guidance,
        name,
        models,
        variations,
        edge,
    };
    crate::jobs::edit_job(
        s,
        &label,
        move |doc, active, ctx| {
            let sel = doc.selection.as_ref().ok_or_else(|| EngineError::Other("the selection is gone".into()))?;
            let k = sel.channels().max(1);
            let coverage: Vec<f32> = sel.read_region(rect).chunks_exact(k).map(|c| c.first().copied().unwrap_or(0.0).clamp(0.0, 1.0)).collect();
            fill_region(doc, active, ctx, job, rect, coverage, None)
        },
        move |(made, w, h, request, template_id)| made_json(&made, w, h, request, &template_id),
    )
}

/// What `generate.expand` asks for: how much canvas to add on each side.
struct Pads {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

/// The prompt an expand uses when none is given (an instruction, so it takes the imperative
/// wrapper of the template).
const EXPAND_DEFAULT_PROMPT: &str = "extend the scene beyond its original edges, continuing it naturally";
const MAX_EXPAND_SIDE: u32 = 16_384;

fn plan_expand(s: &Session, p: &Value) -> Result<(FillPlan, Pads, (u32, u32))> {
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let (ow, oh) = (d.doc.size.width, d.doc.size.height);
    let side = |key: &str| -> Result<u32> {
        match opt_num(EXPAND, p, key, 0.0, f64::from(MAX_EXPAND_SIDE))? {
            None => Ok(0),
            Some(x) if x.fract() == 0.0 => Ok(x as u32),
            Some(_) => Err(bad(EXPAND, format!("`{key}` must be a whole number of pixels"))),
        }
    };
    let (width, height) = (side("width")?, side("height")?);
    let pads = if width > 0 || height > 0 {
        // A target size, the old picture placed by `anchor` (Canvas Size's names).
        let (nw, nh) = (if width > 0 { width } else { ow }, if height > 0 { height } else { oh });
        if nw < ow || nh < oh {
            return Err(bad(
                EXPAND,
                format!("`width`/`height` ({nw}×{nh}) must not be smaller than the canvas ({ow}×{oh}); Generative Expand only adds canvas"),
            ));
        }
        let anchor = opt_str(EXPAND, p, "anchor", 20)?.unwrap_or("center");
        let (ax, ay) = match anchor {
            "topLeft" => (0.0, 0.0),
            "top" => (0.5, 0.0),
            "topRight" => (1.0, 0.0),
            "left" => (0.0, 0.5),
            "center" => (0.5, 0.5),
            "right" => (1.0, 0.5),
            "bottomLeft" => (0.0, 1.0),
            "bottom" => (0.5, 1.0),
            "bottomRight" => (1.0, 1.0),
            other => {
                return Err(bad(
                    EXPAND,
                    format!("`anchor` must be one of topLeft, top, topRight, left, center, right, bottomLeft, bottom, bottomRight (got `{other}`)"),
                ));
            }
        };
        let (dw, dh) = (nw - ow, nh - oh);
        let left = (f64::from(dw) * ax).round() as u32;
        let top = (f64::from(dh) * ay).round() as u32;
        Pads { left, top, right: dw - left, bottom: dh - top }
    } else {
        Pads { left: side("left")?, top: side("top")?, right: side("right")?, bottom: side("bottom")? }
    };
    if pads.left + pads.right == 0 && pads.top + pads.bottom == 0 {
        return Err(bad(EXPAND, "nothing to add: give `left`/`top`/`right`/`bottom` in pixels, or a larger `width`/`height` with an `anchor`"));
    }
    let (nw, nh) = (ow.saturating_add(pads.left).saturating_add(pads.right), oh.saturating_add(pads.top).saturating_add(pads.bottom));
    if nw > MAX_EXPAND_SIDE || nh > MAX_EXPAND_SIDE {
        return Err(bad(EXPAND, format!("the expanded canvas would be {nw}×{nh}; at most {MAX_EXPAND_SIDE} px on a side")));
    }
    // The prompt is optional here: without one the model continues the scene.
    let mut q = p.clone();
    if let Some(o) = q.as_object_mut() {
        let blank = o.get("prompt").and_then(Value::as_str).is_none_or(|t| t.trim().is_empty());
        if blank {
            o.insert("prompt".into(), Value::String(EXPAND_DEFAULT_PROMPT.into()));
        }
        if o.get("name").is_none() {
            let shown = p.get("prompt").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
            o.insert("name".into(), Value::String(shown.map_or("Generative Expand".to_string(), |t| format!("Generative Expand: {}", short(t)))));
        }
    }
    // Expand has its own templates (task `expand`); the fill preference does not apply, so
    // without a `template` param it is `auto`.
    let default_model = s.prefs().integrations.default_edit_model.clone();
    let requested = opt_str(EXPAND, &q, "template", 200)?.map(str::to_string).unwrap_or_else(|| AUTO_TEMPLATE.to_string());
    let auto = requested == AUTO_TEMPLATE;
    if auto && let Some(o) = q.as_object_mut() {
        o.remove("template");
    }
    let Common { template, prompt, negative, seed, steps, guidance, models, name } =
        plan_common(s, EXPAND, &q, if auto { DEFAULT_EXPAND_TEMPLATE } else { &requested }, Task::Expand, &default_model)?;
    let variations = match opt_num(EXPAND, p, "variations", 1.0, f64::from(MAX_VARIATIONS))? {
        None => 1,
        Some(x) if x.fract() == 0.0 => x as u32,
        Some(_) => return Err(bad(EXPAND, format!("`variations` must be a whole number from 1 to {MAX_VARIATIONS}"))),
    };
    let margin = opt_num(EXPAND, p, "margin", 0.0, 1.0)?.unwrap_or(0.25);
    let edge = parse_edge(EXPAND, p)?;
    // The request: the bounding box of the added canvas (an L or a frame when more than one
    // side grows) plus `margin` of the picture next to it, on the new canvas.
    let canvas = Rect::new(0, 0, nw as i32, nh as i32);
    let old = Rect::new(pads.left as i32, pads.top as i32, (pads.left + ow) as i32, (pads.top + oh) as i32);
    let longer = i64::from(nw.max(nh));
    let m = (margin * longer as f64).round().clamp(0.0, 65_536.0) as i32;
    let (l, t, r, b) = (pads.left > 0, pads.top > 0, pads.right > 0, pads.bottom > 0);
    let added = Rect::new(
        if l || t || b { 0 } else { old.x1.saturating_sub(m) },
        if t || l || r { 0 } else { old.y1.saturating_sub(m) },
        if r || t || b { canvas.x1 } else { old.x0.saturating_add(m) },
        if b || l || r { canvas.y1 } else { old.y0.saturating_add(m) },
    );
    let rect = align_to_grid(added.intersect(&canvas), canvas);
    let pixels = u64::from(rect.width()) * u64::from(rect.height());
    if pixels > MAX_REQUEST_PIXELS {
        return Err(EngineError::Other(format!(
            "the expanded area with its margin is {}×{} pixels; Generative Expand handles up to 16 megapixels at a time: add less canvas or lower `margin`",
            rect.width(),
            rect.height()
        )));
    }
    let name = name.unwrap_or_else(|| "Generative Expand".to_string());
    Ok((FillPlan { template, prompt, negative, seed, steps, guidance, name, models, rect, variations, auto, edge }, pads, (nw, nh)))
}

fn expand_enabled(s: &Session) -> std::result::Result<(), String> {
    web_unavailable()?;
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

/// `generate.expand`: grow the canvas and have the model paint the new area, as one undo step.
fn run_expand(s: &mut Session, p: &Value) -> Result<Value> {
    let (plan, pads, (nw, nh)) = plan_expand(s, p)?;
    let backend = backend(s)?;
    let FillPlan { template, prompt, negative, seed, steps, guidance, name, models, rect, variations, auto, edge } = plan;
    let label = name.clone();
    let job = FillJob {
        backend,
        server: server_key(s),
        command: EXPAND,
        template_id: template.meta.id.clone(),
        auto,
        auto_order: AUTO_EXPAND_ORDER,
        prompt,
        negative,
        seed,
        steps,
        guidance,
        name,
        models,
        variations,
        edge,
    };
    crate::jobs::edit_job(
        s,
        &label,
        move |doc, active, ctx| {
            // Canvas Size with a transparent extension: the picture moves by the left/top pads.
            let (ow, oh) = (doc.size.width, doc.size.height);
            crate::image_cmds::translate_doc(doc, EXPAND, pads.left as i32, pads.top as i32)?;
            doc.size = Size::new(nw, nh);
            crate::canvas_geom::refresh(doc, crate::canvas_geom::Refresh::Shapes);
            let old = Rect::new(pads.left as i32, pads.top as i32, (pads.left + ow) as i32, (pads.top + oh) as i32);
            // Everything in the request that is not the old picture is to be painted.
            let (w, h) = (rect.width() as usize, rect.height() as usize);
            let mut coverage = vec![1.0f32; w * h];
            for y in 0..h {
                for x in 0..w {
                    let (cx, cy) = (rect.x0 + x as i32, rect.y0 + y as i32);
                    if cx >= old.x0
                        && cx < old.x1
                        && cy >= old.y0
                        && cy < old.y1
                        && let Some(c) = coverage.get_mut(y * w + x)
                    {
                        *c = 0.0;
                    }
                }
            }
            let (made, rw, rh, request, template_id) = fill_region(doc, active, ctx, job, rect, coverage, Some(Outpaint { picture: old.intersect(&rect) }))?;
            Ok((made, rw, rh, request, template_id, old))
        },
        move |(made, w, h, request, template_id, old)| {
            let mut v = made_json(&made, w, h, request, &template_id);
            if let Some(o) = v.as_object_mut() {
                o.insert("canvas".into(), json!([nw, nh]));
                o.insert("offset".into(), json!([old.x0, old.y0]));
            }
            v
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

/// `variations`: 1..=[`MAX_VARIATIONS`] whole results with consecutive seeds.
fn parse_variations(cmd: &str, p: &Value) -> Result<u32> {
    match opt_num(cmd, p, "variations", 1.0, f64::from(MAX_VARIATIONS))? {
        None => Ok(1),
        Some(x) if x.fract() == 0.0 => Ok(x as u32),
        Some(_) => Err(bad(cmd, format!("`variations` must be a whole number from 1 to {MAX_VARIATIONS}"))),
    }
}

fn edit_enabled(s: &Session) -> std::result::Result<(), String> {
    web_unavailable()?;
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

/// `generate.edit`: the whole picture goes to an edit model with an instruction; the result is
/// a layer over the canvas, masked to the selection when there is one.
fn plan_edit(s: &Session, p: &Value) -> Result<FillPlan> {
    let default_model = s.prefs().integrations.default_edit_model.clone();
    let requested = opt_str(EDIT, p, "template", 200)?.map(str::to_string).unwrap_or_else(|| AUTO_TEMPLATE.to_string());
    let auto = requested == AUTO_TEMPLATE;
    let mut q = p.clone();
    if auto && let Some(o) = q.as_object_mut() {
        o.remove("template");
    }
    let Common { template, prompt, negative, seed, steps, guidance, models, name } =
        plan_common(s, EDIT, &q, if auto { DEFAULT_EDIT_TEMPLATE } else { &requested }, Task::Edit, &default_model)?;
    let edge = parse_edge(EDIT, p)?;
    let variations = parse_variations(EDIT, p)?;
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let rect = d.doc.bounds();
    if rect.is_empty() {
        return Err(EngineError::Other("the document is empty".into()));
    }
    if u64::from(rect.width()) * u64::from(rect.height()) > MAX_REQUEST_PIXELS {
        return Err(EngineError::Other(format!(
            "the picture is {}×{} pixels; Generative Edit handles up to 16 megapixels at a time",
            rect.width(),
            rect.height()
        )));
    }
    let name = name.unwrap_or_else(|| format!("Generative Edit: {}", short(&prompt)));
    Ok(FillPlan { template, prompt, negative, seed, steps, guidance, name, models, rect, variations, auto, edge })
}

fn run_edit(s: &mut Session, p: &Value) -> Result<Value> {
    let plan = plan_edit(s, p)?;
    let backend = backend(s)?;
    let FillPlan { template, prompt, negative, seed, steps, guidance, name, models, rect, variations, auto, edge } = plan;
    let label = name.clone();
    let job = FillJob {
        backend,
        server: server_key(s),
        command: EDIT,
        template_id: template.meta.id.clone(),
        auto,
        auto_order: AUTO_EDIT_ORDER,
        prompt,
        negative,
        seed,
        steps,
        guidance,
        name,
        models,
        variations,
        edge,
    };
    crate::jobs::edit_job(
        s,
        &label,
        move |doc, active, ctx| {
            // The whole canvas is re-rendered; a selection, when there is one, confines what
            // of it shows (the edit template takes no mask, see `fill_region`).
            let coverage: Vec<f32> = match doc.selection.as_ref() {
                Some(sel) => {
                    let k = sel.channels().max(1);
                    sel.read_region(rect).chunks_exact(k).map(|c| c.first().copied().unwrap_or(0.0).clamp(0.0, 1.0)).collect()
                }
                None => vec![1.0; rect.width() as usize * rect.height() as usize],
            };
            fill_region(doc, active, ctx, job, rect, coverage, None)
        },
        move |(made, w, h, request, template_id)| made_json(&made, w, h, request, &template_id),
    )
}

fn doc_enabled(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

/// Everything `generate.removeBackground` needs, validated before any network work.
struct MattePlan {
    common: Common,
    layer: LayerId,
    /// Load the matte as the selection (combined by `mode`) instead of masking the layer.
    as_selection: bool,
    mode: SelectionMode,
    /// Send the composite rather than the layer alone.
    all_layers: bool,
    /// What to keep, as typed (empty = the model's own reading of the subject).
    subject: String,
    /// `template: auto`: the job may still fall back from the matte model to the detector.
    auto: bool,
    /// The planned template is a detector (`Task::Segment`), not a matte model.
    segment: bool,
}

/// What the detector is asked for when Remove Background has no subject named.
const SUBJECT_PROMPT: &str = "the main subject";

fn plan_remove_bg(s: &Session, p: &Value) -> Result<MattePlan> {
    let layer = layer_param(s, p)?;
    let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
    crate::cutout_cmds::check(doc, doc.layer(layer).ok_or(EngineError::NoLayer(layer))?).map_err(EngineError::Other)?;
    let subject = opt_str(REMOVE_BG, p, "prompt", MAX_PROMPT_CHARS)?.map(str::trim).unwrap_or("").to_string();
    // `auto`: the matte model (research licence) when the preference allows it, else the
    // permissive detector (SAM 3.1), whose mask is hard-edged but needs no opt-in.
    let research_ok = s.prefs().integrations.allow_research_models;
    let requested = opt_str(REMOVE_BG, p, "template", 200)?.unwrap_or(AUTO_TEMPLATE).to_string();
    let auto = requested == AUTO_TEMPLATE;
    let template_id: &str =
        if auto { if research_ok { DEFAULT_MATTE_TEMPLATE } else { crate::select_ml_cmds::DEFAULT_SEGMENT_TEMPLATE } } else { requested.as_str() };
    let task = match template::find(template_id).map(|t| t.meta.task) {
        Ok(Task::Matte) => Task::Matte,
        Ok(Task::Segment) => Task::Segment,
        Ok(other) => return Err(bad(REMOVE_BG, format!("template `{template_id}` is a {other:?} template, not a matte or a detector"))),
        Err(e) => return Err(bad(REMOVE_BG, e.to_string())),
    };
    let mut q = p.clone();
    if let Some(o) = q.as_object_mut() {
        // The prompt the job sends depends on the route; this one satisfies the plan.
        o.insert("prompt".into(), Value::String(if subject.is_empty() { MATTE_DEFAULT_PROMPT.into() } else { subject.clone() }));
        o.insert("template".into(), Value::String(template_id.to_string()));
    }
    let common = plan_common(s, REMOVE_BG, &q, template_id, task, "")?;
    let as_selection = opt_bool(REMOVE_BG, p, "asSelection", false)?;
    let mode = match opt_str(REMOVE_BG, p, "mode", 20)? {
        None | Some("replace") | Some("new") => SelectionMode::Replace,
        Some("add") => SelectionMode::Add,
        Some("subtract") => SelectionMode::Subtract,
        Some("intersect") => SelectionMode::Intersect,
        Some(other) => return Err(bad(REMOVE_BG, format!("`mode` must be replace, add, subtract or intersect (got `{other}`)"))),
    };
    let all_layers = opt_bool(REMOVE_BG, p, "sampleAllLayers", false)?;
    Ok(MattePlan { common, layer, as_selection, mode, all_layers, subject, auto, segment: task == Task::Segment })
}

fn remove_bg_enabled(s: &Session) -> std::result::Result<(), String> {
    web_unavailable()?;
    let d = s.active().ok_or("no document open")?;
    crate::cutout_cmds::check(&d.doc, crate::active_layer_of(s)?)
}

/// The size a matte request goes out at: within [`MAX_MATTE_REQUEST_PIXELS`], sides in
/// multiples of 32 (TextEncodeQwenImage21 rounds its references to that; sending them rounded
/// keeps the result the request's size).
pub(crate) fn matte_request_size(w: u32, h: u32) -> (u32, u32) {
    let (rw, rh) = fit_pixels(w, h, MAX_MATTE_REQUEST_PIXELS);
    ((rw / 32).max(1) * 32, (rh / 32).max(1) * 32)
}

/// `generate.removeBackground`: the model separates the subject and answers with an RGBA image;
/// its alpha becomes the layer's mask (or the selection). The pixels it came with are the
/// model's re-rendering and are never used: the layer keeps its own.
fn run_remove_bg(s: &mut Session, p: &Value) -> Result<Value> {
    let plan = plan_remove_bg(s, p)?;
    let backend = backend(s)?;
    let server = server_key(s);
    let MattePlan { common, layer: id, as_selection, mode, all_layers, subject, auto, segment } = plan;
    let Common { template, negative, seed, steps, guidance, models, .. } = common;
    let template_id = template.meta.id.clone();
    let label = if as_selection { "Select Subject (Generative)" } else { "Remove Background (Generative)" };
    crate::jobs::edit_job(
        s,
        label,
        move |doc, _active, ctx| {
            ctx.progress(0.0, "Rendering");
            // The route: the matte model when it is wanted and the server has it, else the
            // detector (its masks are hard-edged, its licence permissive).
            let (template_id, segment, models) = if auto && !segment {
                let t = resolve_auto(backend.as_ref(), &[DEFAULT_MATTE_TEMPLATE], crate::select_ml_cmds::DEFAULT_SEGMENT_TEMPLATE);
                let seg = t != DEFAULT_MATTE_TEMPLATE;
                (t, seg, if seg { Vec::new() } else { models })
            } else {
                (template_id, segment, models)
            };
            let prompt = match (segment, subject.is_empty()) {
                (true, true) => SUBJECT_PROMPT.to_string(),
                (false, true) => MATTE_DEFAULT_PROMPT.to_string(),
                (_, false) => subject.clone(),
            };
            let area = doc.bounds();
            let (w, h) = (area.width(), area.height());
            let n = w as usize * h as usize;
            let layer = if all_layers { None } else { doc.layer(id).and_then(|l| l.surface()) };
            // The layer's own alpha: transparent pixels go to the model over mid-grey (its
            // loader drops alpha, and black holes would read as content) and stay out of the
            // matte afterwards.
            let mut own_alpha: Option<Vec<u8>> = None;
            let rgba8: Vec<u8> = match layer {
                Some(surf) => {
                    let mut px = vec![[0u8; 4]; n];
                    surf.read_rgba8_into(area, &mut px);
                    own_alpha = Some(px.iter().map(|p| p[3]).collect());
                    px.into_iter()
                        .flat_map(|p| {
                            let a = u16::from(p[3]);
                            let over = |c: u8| ((u16::from(c) * a + 128 * (255 - a)) / 255) as u8;
                            [over(p[0]), over(p[1]), over(p[2]), 255]
                        })
                        .collect()
                }
                None => photocraft_compose::render(doc, area).to_rgba8().pixels,
            };
            let image = Rgba8::new(w, h, rgba8).map_err(gen_err)?;
            let (rw, rh) = if segment { fit_pixels(w, h, MAX_SEGMENT_REQUEST_PIXELS) } else { matte_request_size(w, h) };
            let image = if (rw, rh) == (w, h) { image } else { resize_rgba8(&image, rw, rh)? };
            let mut params = BTreeMap::new();
            if segment {
                params.insert("threshold".to_string(), json!(0.5));
            }
            let req =
                Request { template: template_id.clone(), prompt, negative, seed, steps, guidance, image: Some(image), mask: None, models, size: None, params };
            let resp = match run_switching(backend.as_ref(), &server, &req, &JobProgress::new(ctx)) {
                Ok(r) => r,
                Err(photocraft_genai::Error::NoOutput(_)) if segment => {
                    return Err(EngineError::Other("the detector found no subject: name it in the prompt".into()));
                }
                Err(e) => return Err(gen_err(e)),
            };
            ctx.check()?;
            ctx.progress(0.95, if as_selection { "Selecting" } else { "Masking" });
            // The matte model answers with one RGBA image whose alpha is the matte; the detector
            // with one mask image per instance (red = coverage), unioned.
            let mut cov: Vec<f32> = vec![0.0; n];
            let mut images = resp.images.into_iter().peekable();
            if images.peek().is_none() {
                return Err(EngineError::Other("the backend returned no image".into()));
            }
            for out in images {
                let channel = if segment { 0 } else { 3 };
                let plane = Gray8::new(out.width, out.height, out.data.as_chunks::<4>().0.iter().map(|p| p[channel]).collect()).map_err(gen_err)?;
                let plane = if (plane.width, plane.height) == (w, h) { plane } else { resize_gray8(&plane, w, h)? };
                for (c, v) in cov.iter_mut().zip(&plane.data) {
                    *c = c.max(f32::from(*v) / 255.0);
                }
                if !segment {
                    break;
                }
            }
            if let Some(own) = own_alpha {
                for (c, a) in cov.iter_mut().zip(own) {
                    *c *= f32::from(a) / 255.0;
                }
            }
            let (bounds, count) = crate::select_ml_cmds::coverage_bounds(&cov, area);
            if count == 0 {
                return Err(EngineError::Other("the model kept nothing: name the subject in the prompt".into()));
            }
            if as_selection {
                doc.selection = sel::combine(doc.selection.as_ref(), &cov, area, mode);
            } else {
                crate::extra_cmds::background_to_layer_for_mask(doc, id);
                let mut surface = Surface::with_default(PixelFormat::GRAY8, &[0.0]);
                surface.write_region(area, &cov);
                doc.layer_mut(id).ok_or(EngineError::NoLayer(id))?.mask = Some(LayerMask { surface, ..LayerMask::reveal_all() });
                doc.selection = None;
            }
            Ok((bounds, count, resp.seed, resp.run_id, resp.elapsed_ms, resp.timings, (rw, rh), (w, h), template_id))
        },
        move |(b, count, seed, run_id, ms, timings, (rw, rh), (w, h), tid)| {
            json!({
                "layer": id.0, "selection": as_selection, "bounds": [b.x0, b.y0, b.width(), b.height()], "pixels": count,
                "seed": seed, "template": tid, "runId": run_id, "width": w, "height": h, "requestWidth": rw, "requestHeight": rh,
                "ms": ms, "timings": [serde_json::to_value(timings).unwrap_or(Value::Null)],
            })
        },
    )
}

/// A detector request is sent at most this large (SAM 3.1 works at about 1 megapixel inside).
const MAX_SEGMENT_REQUEST_PIXELS: u64 = 2048 * 1024;

/// Edit › Purge › Generative Models: the server unloads its models and frees their memory. A
/// server that has had several model families loaded can end up streaming weights on every run
/// (2 GB of VRAM free, fills three times slower); this resets it, at the price of one reload.
fn run_free(s: &mut Session, _p: &Value) -> Result<Value> {
    backend(s)?.free().map_err(gen_err)?;
    Ok(json!({"freed": true}))
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
    // `async: true` probes on a worker (the picker's installed badges): a background job whose
    // result is this command's usual answer.
    if probe && opt_bool(MODELS, p, "async", false)? {
        let backend = backend(s)?;
        return crate::jobs::run(
            s,
            "Checking generative models",
            false,
            move |ctx| {
                ctx.progress(0.2, "Asking the server");
                Ok(models_json(Some(backend.as_ref()), allow_research))
            },
            move |_, v| Ok(v),
        );
    }
    let backend = if probe { backend(s).ok() } else { None };
    Ok(models_json(backend.as_deref(), allow_research))
}

/// `generate.models`' answer: every template with its model slots and, with a `backend`, what
/// the server has installed and what `auto` would pick.
fn models_json(backend: Option<&dyn GenerativeBackend>, allow_research: bool) -> Value {
    let health = backend.map(|b| b.health());
    let online = health.as_ref().is_some_and(|h| h.ok);
    let mut folders: BTreeMap<String, Option<Vec<String>>> = BTreeMap::new();
    let mut templates = Vec::new();
    for t in template::builtin() {
        let m = &t.meta;
        let mut slots = Vec::new();
        for slot in &m.models {
            let files = if online {
                folders.entry(slot.folder.clone()).or_insert_with(|| backend.and_then(|b| b.model_files(&slot.folder).ok())).clone()
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
    // What `auto` would pick right now (needs the server's model lists).
    let auto = |order, fallback| backend.filter(|_| online).map(|b| resolve_auto(b, order, fallback));
    // Remove Background: the matte model only with the research opt-in, else the detector.
    let auto_matte = backend.filter(|_| online).map(|b| {
        if allow_research {
            resolve_auto(b, &[DEFAULT_MATTE_TEMPLATE], crate::select_ml_cmds::DEFAULT_SEGMENT_TEMPLATE)
        } else {
            crate::select_ml_cmds::DEFAULT_SEGMENT_TEMPLATE.to_string()
        }
    });
    json!({
        "server": health, "templates": templates,
        "autoFill": auto(AUTO_FILL_ORDER, DEFAULT_FILL_TEMPLATE),
        "autoExpand": auto(AUTO_EXPAND_ORDER, DEFAULT_EXPAND_TEMPLATE),
        "autoEdit": auto(AUTO_EDIT_ORDER, DEFAULT_EDIT_TEMPLATE),
        "autoMatte": auto_matte,
    })
}

/// `generate.info`: what a layer remembers about the run that made it.
fn run_info(s: &mut Session, p: &Value) -> Result<Value> {
    let id = layer_param(s, p)?;
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let layer = d.doc.layer(id).ok_or(EngineError::NoLayer(id))?;
    Ok(json!({"layer": id.0, "generative": generative_info(layer)}))
}

fn similar_enabled(s: &Session) -> std::result::Result<(), String> {
    web_unavailable()?;
    let d = s.active().ok_or("no document open")?;
    let layer = crate::active_layer_of(s)?;
    let _ = d;
    if generative_info(layer).is_none() {
        return Err("the active layer was not made by a generative command".into());
    }
    Ok(())
}

/// `generate.similar`: run what made the layer again with a new seed, in the same place (the
/// layer's mask is the area), as a new layer above it.
fn run_similar(s: &mut Session, p: &Value) -> Result<Value> {
    let id = layer_param(s, p)?;
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let layer = d.doc.layer(id).ok_or(EngineError::NoLayer(id))?;
    let info = generative_info(layer).ok_or_else(|| EngineError::Other("the layer was not made by a generative command".into()))?;
    let seed = parse_seed(SIMILAR, p)?;
    let variations = parse_variations(SIMILAR, p)?;
    if info.command == IMAGE {
        // A generated image: the same request, as a layer above this one.
        let q = json!({
            "prompt": info.prompt, "negative": info.negative, "template": info.template, "seed": seed, "steps": info.steps,
            "guidance": info.guidance, "target": "layer", "width": info.width, "height": info.height, "name": info.name,
            "transparent": info.transparent,
        });
        s.edit("Select Layer", |_, active| {
            *active = Some(id);
            Ok(())
        })?;
        return run_image(s, &q);
    }
    let template = template::find(&info.template).map_err(|e| bad(SIMILAR, e.to_string()))?;
    if template.meta.license == License::Research && !s.prefs().integrations.allow_research_models {
        return Err(EngineError::Other(format!(
            "`{}` uses a research-only model; turn on Allow Research-Only Models in Preferences › AI Integrations to use it",
            template.meta.id
        )));
    }
    let canvas = d.doc.bounds();
    let rect = Rect::new(info.rect[0], info.rect[1], info.rect[0].saturating_add(info.rect[2]), info.rect[1].saturating_add(info.rect[3])).intersect(&canvas);
    if rect.is_empty() {
        return Err(EngineError::Other("the layer's area is no longer on the canvas".into()));
    }
    // An expand layer's area is regenerated as a fill (its canvas is already there); edits and
    // fills run as what they were.
    let (command, auto, auto_order, template_id) = match info.command.as_str() {
        c if c == EDIT => (EDIT, false, AUTO_EDIT_ORDER, info.template.clone()),
        c if c == FILL => (FILL, false, AUTO_FILL_ORDER, info.template.clone()),
        _ => (FILL, true, AUTO_FILL_ORDER, DEFAULT_FILL_TEMPLATE.to_string()),
    };
    let backend = backend(s)?;
    let job = FillJob {
        backend,
        server: server_key(s),
        command,
        template_id,
        auto,
        auto_order,
        prompt: info.prompt.clone(),
        negative: info.negative.clone(),
        seed,
        steps: info.steps,
        guidance: info.guidance,
        name: info.name.clone(),
        models: Vec::new(),
        variations,
        edge: if info.edge == "hard" { Edge::Hard } else { Edge::Soft },
    };
    let label = format!("Generate Similar: {}", short(&info.prompt));
    crate::jobs::edit_job(
        s,
        &label,
        move |doc, active, ctx| {
            let n = rect.width() as usize * rect.height() as usize;
            let coverage: Vec<f32> = match doc.layer(id).and_then(|l| l.mask.as_ref()) {
                Some(m) => {
                    let k = m.surface.channels().max(1);
                    m.surface.read_region(rect).chunks_exact(k).map(|c| c.first().copied().unwrap_or(0.0).clamp(0.0, 1.0)).collect()
                }
                None => vec![1.0; n],
            };
            // The result lands above the layer it is similar to.
            *active = Some(id);
            fill_region(doc, active, ctx, job, rect, coverage, None)
        },
        move |(made, w, h, request, template_id)| made_json(&made, w, h, request, &template_id),
    )
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
            params: r#"{"prompt":text,"negative":text,"steps":0..250=0,"guidance":0..30=0,"margin":0..1=0.25,"edge":"soft|hard","variations":int=1,"seed":{u64?=random},"template":{id?=Preferences › Default Fill Template; "auto" = the fastest permissive tier whose files the server has},"model":{file?=Preferences},"name":{str?}} → {"layer","layers":[id],"seed","seeds":[u64],"template","runId","width","height","requestWidth","requestHeight","ms","timings":[{"encodeMs","uploadMs","queueMs","runMs","downloadMs"}]} (steps and guidance 0 = the template's defaults; margin = context around the selection as a fraction of its larger side; edge soft = the result's layer mask fades across the band the model re-rendered around the selection (4 % of its size), dithered with grain so there is no visible line; hard = the mask is exactly the selection; variations 1..4 = results with consecutive seeds, each a layer, only the first visible; areas over 0.75 megapixels are sent downscaled and come back resampled; a background job: the result is a new layer above the active one; needs a ComfyUI server, see Preferences › AI Integrations)"#,
            enabled: fill_enabled,
            run: run_fill,
            journal: true,
        },
        CommandSpec {
            id: IMAGE,
            label: "Generate Image…",
            menu: &[],
            shortcut: None,
            params: r#"{"prompt":text,"negative":text,"target":"auto|layer|document","width":int=0,"height":int=0,"transparent":bool=false,"steps":0..250=0,"guidance":0..30=0,"seed":{u64?=random},"template":{id?=Preferences › Default Image Template},"model":{file?=Preferences},"name":{str?}} → {"layer"|"document","seed","template","runId","width","height","ms"} (target auto = a layer over the whole canvas when a document is open, else a new document; width/height 0 = the document's size or 1024, otherwise 64..4096 rounded down to multiples of 16; transparent = ask for the subject alone on a transparent background and keep the alpha the model returns (templates whose model can, such as qwen-2.1/image; others refuse); steps and guidance 0 = the template's defaults; a background job; needs a ComfyUI server, see Preferences › AI Integrations)"#,
            enabled: image_enabled,
            run: run_image,
            journal: true,
        },
        CommandSpec {
            id: EXPAND,
            label: "Generative Expand…",
            // Placed by the menu catalogue under Edit.
            menu: &[],
            shortcut: None,
            params: r#"{"prompt":text,"left":int=0,"top":int=0,"right":int=0,"bottom":int=0,"width":int=0,"height":int=0,"negative":text,"steps":0..250=0,"guidance":0..30=0,"edge":"soft|hard","variations":int=1,"margin":{0..1=0.25},"anchor":{str?=center},"seed":{u64?=random},"template":{id?="auto": the Lightning expand tier when its LoRA is installed, else the 40-step one},"model":{file?=Preferences},"name":{str?}} → {"layer","layers":[id],"seed","seeds":[u64],"template","canvas":[w,h],"offset":[x,y],"width","height","requestWidth","requestHeight","ms","timings"} (adds canvas (left/top/right/bottom in pixels, or a larger width/height placed by anchor: topLeft, top, topRight, left, center, right, bottomLeft, bottom, bottomRight) and has the model paint it, as one undo step: the picture moves by the left/top pads, the result is a new layer over the added area; edge soft = its mask fades across the re-rendered band just inside the old edge (8 % of the picture's longer side), dithered; hard = exactly the added area; an empty prompt continues the scene; otherwise as generate.fill; needs a ComfyUI server)"#,
            enabled: expand_enabled,
            run: run_expand,
            journal: true,
        },
        CommandSpec {
            id: EDIT,
            label: "Generative Edit…",
            // Placed by the menu catalogue under Edit, with the other generative items.
            menu: &[],
            shortcut: None,
            params: r#"{"prompt":text,"negative":text,"steps":0..250=0,"guidance":0..30=0,"edge":"soft|hard","variations":int=1,"seed":{u64?=random},"template":{id?="auto": the Lightning edit tier when its LoRA is installed, else the 40-step one},"model":{file?=Preferences},"name":{str?}} → {"layer","layers":[id],"seed","seeds":[u64],"template","runId","width","height","requestWidth","requestHeight","ms","timings"} (edits the whole picture by instruction: "make the sky stormy", "turn the boat blue", "remove the person"; a description such as "a stormy sky" is wrapped as "change this image so that it shows …"; the composite goes to the model and the result is a new layer above the active one; with a selection the layer is masked to it (edge soft = a dithered fade around it, hard = exactly) so only that part of the edit shows; variations 1..4 = results with consecutive seeds, each a layer, only the first visible; pictures over 0.75 megapixels are sent downscaled and come back resampled; a background job; needs a ComfyUI server)"#,
            enabled: edit_enabled,
            run: run_edit,
            journal: true,
        },
        CommandSpec {
            id: REMOVE_BG,
            label: "Remove Background (Generative)…",
            // Placed by the menu catalogue under Edit, with the other generative items.
            menu: &[],
            shortcut: None,
            params: r#"{"prompt":text,"asSelection":bool=false,"sampleAllLayers":bool=false,"mode":"replace|add|subtract|intersect","layer":{id?=active},"seed":{u64?=random},"steps":{0..250=0},"guidance":{0..30=0},"template":{id?="auto": qwen-2.1/matte (a soft matte from Qwen-Image-2.1's alpha output; research licence, Preferences › AI Integrations › Allow Research-Only Models) when allowed and installed, else sam3.1/segment (the permissive detector, a hard-edged mask)},"model":{file?}} → {"layer","selection","bounds":[x,y,w,h],"pixels","seed","template","runId","width","height","requestWidth","requestHeight","ms","timings"} (prompt = what to keep, e.g. "the lighthouse", empty = the model's own reading of the subject; the matte becomes the layer's mask (the Background becomes a normal layer; the pixels are never changed) or, with asSelection, the selection combined by mode; sampleAllLayers sends the composite instead of the layer alone; images over one megapixel are sent downscaled and the matte comes back resampled; a background job; needs a ComfyUI server; the classical Quick Action layer.removeBackground needs none)"#,
            enabled: remove_bg_enabled,
            run: run_remove_bg,
            journal: true,
        },
        CommandSpec {
            id: SIMILAR,
            label: "Generate Similar",
            // Placed by the menu catalogue under Edit, with the other generative items.
            menu: &[],
            shortcut: None,
            params: r#"{"layer":{id?=active},"seed":{u64?=random},"variations":int=1} → as the command that made the layer (runs it again with a new seed in the same place: the layer's mask is the area, the result is a new layer above it; a generated image is generated again as a layer; an expanded area is filled again with its prompt; needs a ComfyUI server)"#,
            enabled: similar_enabled,
            run: run_similar,
            journal: true,
        },
        CommandSpec {
            id: INFO,
            label: "Generative Layer Info",
            menu: &[],
            shortcut: None,
            params: r#"{"layer":{id?=active}} → {"layer","generative":{"command","prompt","negative","template","seed","steps","guidance","edge","rect":[x,y,w,h],"name","width","height","transparent"}|null} (what a layer remembers about the generative run that made it; kept with the layer through PSD round trips)"#,
            enabled: doc_enabled,
            run: run_info,
            journal: false,
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
            id: FREE,
            label: "Generative Models",
            // Placed by the menu catalogue under Edit › Purge.
            menu: &[],
            shortcut: None,
            params: r#"{} → {"freed"} (asks the ComfyUI server to unload its models and free their VRAM and RAM; the next generation reloads what it needs. Use it when fills have become several times slower than usual: a server that has loaded more than one model family may be streaming weights on every run)"#,
            enabled: |_| web_unavailable(),
            run: run_free,
            journal: false,
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

#[cfg(test)]
#[path = "generate_matte_tests.rs"]
mod matte_tests;

#[cfg(test)]
#[path = "generate_edit_tests.rs"]
mod edit_tests;

#[cfg(test)]
#[path = "generate_similar_tests.rs"]
mod similar_tests;
