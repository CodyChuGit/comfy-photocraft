//! UI for the retouching tools (healing, patch, clone, history brush, blur/sharpen/smudge,
//! dodge/burn/sponge, Mixer Brush) and the smart selection tools (Quick Selection, Object
//! Selection): gesture → engine command, options bars, and the clone-source marker.

use egui::{Color32, Stroke, vec2};
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::canvas::ViewXform;
use crate::state::Tool;
use crate::theme::Tokens;

/// The tool a stroke uses with ⌥ held: Dodge and Burn swap, as do Blur and Sharpen.
pub(crate) fn alt_flipped(tool: Tool, alt: bool) -> Tool {
    match (tool, alt) {
        (Tool::Dodge, true) => Tool::Burn,
        (Tool::Burn, true) => Tool::Dodge,
        (Tool::Blur, true) => Tool::Sharpen,
        (Tool::Sharpen, true) => Tool::Blur,
        _ => tool,
    }
}

/// Finish a stroke with a retouching tool. Returns false if `tool` isn't one.
pub fn finish_stroke(app: &mut PhotocraftApp, tool: Tool, points: &[[f64; 3]], mods: egui::Modifiers) -> bool {
    let o = app.ui.tool_options.clone();
    let pts = json!(points);
    let (cmd, mut p): (&str, Value) = match tool {
        Tool::SpotHealing => ("paint.spotHealing", json!({"type": o.spot_type, "sampleAllLayers": o.sample_all_layers})),
        Tool::MixerBrush => ("paint.mixerBrush", json!({})),
        Tool::Healing | Tool::CloneStamp => {
            let mut p = json!({"aligned": o.clone_aligned, "sampleLayer": o.clone_sample});
            // The Clone Source panel's active slot (set by ⌥-click) drives the stroke: the engine
            // keeps the aligned pairing and applies the slot's scale/rotation/flip.
            let slot = app.session.presets.clone.active().source.is_some();
            match (app.ui.clone_offset.filter(|_| o.clone_aligned && !slot), app.ui.clone_source.filter(|_| !slot)) {
                _ if slot => {}
                (Some(off), _) => p["offset"] = json!(off),
                (None, Some(src)) => p["source"] = json!(src),
                (None, None) => {
                    // Photoshop says Option-click on the Mac and Alt-click on Windows.
                    app.ui.status = if cfg!(target_os = "macos") {
                        tl!("Option-click to define a source point to clone from")
                    } else {
                        tl!("Alt-click to define a source point to clone from")
                    }
                    .into();
                    app.ui.status_error = true;
                    return true;
                }
            }
            (if tool == Tool::Healing { "paint.healingBrush" } else { "paint.cloneStamp" }, p)
        }
        Tool::HistoryBrush => ("paint.historyBrush", json!({})),
        Tool::Blur => ("paint.blur", json!({"strength": o.strength, "sampleAllLayers": o.sample_all_layers})),
        Tool::Sharpen => ("paint.sharpen", json!({"strength": o.strength, "protectDetail": o.protect_detail, "sampleAllLayers": o.sample_all_layers})),
        Tool::Smudge => ("paint.smudge", json!({"strength": o.strength, "fingerPainting": o.finger_painting, "sampleAllLayers": o.sample_all_layers})),
        Tool::Dodge | Tool::Burn => (
            if tool == Tool::Dodge { "paint.dodge" } else { "paint.burn" },
            json!({"range": o.tone_range, "exposure": o.exposure, "protectTones": o.protect_tones}),
        ),
        Tool::Sponge => ("paint.sponge", json!({"mode": o.sponge_mode, "vibrance": o.vibrance})),
        Tool::QuickSelection => {
            let size = app.session.tools.brush.size;
            let mode = if mods.alt { "subtract" } else { "add" };
            // comfy-photocraft: with a server that answers the stroke is SAM 3.1's prompt.
            if quick_select_detector(app, points, mode) {
                return true;
            }
            let xy: Vec<[f64; 2]> = points.iter().map(|q| [q[0], q[1]]).collect();
            let _ = app
                .run("select.quick", json!({"points": xy, "size": size, "mode": mode, "enhanceEdge": o.enhance_edge, "sampleAllLayers": o.sample_all_layers}));
            return true;
        }
        _ => return false,
    };
    p["points"] = pts;
    // The layer mask, an alpha channel or the Quick Mask when targeted, as the Brush paints.
    p["target"] = crate::canvas::paint_target(app);
    match app.run(cmd, p) {
        Ok(r) if matches!(tool, Tool::Healing | Tool::CloneStamp) => {
            // Aligned: keep the offset for later strokes; non-aligned: every stroke restarts at the source.
            app.ui.clone_offset = r.get("offset").and_then(|v| serde_json::from_value(v.clone()).ok()).filter(|_| o.clone_aligned);
        }
        Ok(_) => {}
        Err(e) => {
            app.ui.status = e;
            app.ui.status_error = true;
        }
    }
    true
}

/// comfy-photocraft: how many spots along a Quick Selection stroke the detector gets. SAM 3.1
/// reads several positive points as one object, but the mask degrades as they pile up (the fox
/// picture, 2026-10-16: 1–4 spots along a stroke give the whole fox, ~340 k pixels; 8 give
/// 292 k; 16 give a fragmented 164 k), so the stroke's ends and two spots between them say which
/// object.
pub(crate) const QUICK_STROKE_POINTS: usize = 4;

/// The spots along a Quick Selection stroke that go to the detector: at most
/// [`QUICK_STROKE_POINTS`], evenly spaced along the stroke's samples, whole pixels inside the
/// `canvas` (width, height), no repeats.
pub(crate) fn stroke_prompt(points: &[[f64; 3]], canvas: [f64; 2]) -> Vec<[f64; 2]> {
    let inside: Vec<[f64; 2]> = points
        .iter()
        .map(|p| [p[0].round(), p[1].round()])
        .filter(|p| p[0].is_finite() && p[1].is_finite() && p[0] >= 0.0 && p[1] >= 0.0 && p[0] < canvas[0] && p[1] < canvas[1])
        .collect();
    let n = inside.len();
    let k = n.min(QUICK_STROKE_POINTS);
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(k);
    for i in 0..k {
        let idx = if k < 2 { 0 } else { i * (n - 1) / (k - 1) };
        if let Some(p) = inside.get(idx).copied()
            && !out.contains(&p)
        {
            out.push(p);
        }
    }
    out
}

/// comfy-photocraft: a Quick Selection stroke through SAM 3.1 when the server answers. The spots
/// along the stroke are one point prompt (`select.byPoint`), so painting over an object selects
/// the object rather than the similar-coloured region; ⌥ subtracts the object under the stroke;
/// Enhance Edge is the detector's edge refinement. False, the classical `select.quick`, without
/// a server, with one that is down, or for a stroke entirely off the canvas.
fn quick_select_detector(app: &mut PhotocraftApp, points: &[[f64; 3]], mode: &str) -> bool {
    let now = app.last_frame_time;
    if !crate::object_select::online(app, now) {
        return false;
    }
    let Some(canvas) = app.session.active().map(|d| [f64::from(d.doc.size.width), f64::from(d.doc.size.height)]) else { return false };
    let pts = stroke_prompt(points, canvas);
    if pts.is_empty() {
        return false;
    }
    let o = &app.ui.tool_options;
    let q = json!({"points": pts, "mode": mode, "sampleAllLayers": o.sample_all_layers, "refine": o.enhance_edge});
    if let Err(e) = app.run("select.byPoint", q) {
        crate::notices::error(app, e);
    }
    true
}

/// Patch Tool and Content-Aware Move Tool: a drag that starts inside the selection (without ⇧ or
/// ⌥) drags the selection; any other drag draws a lasso selection.
pub fn patch_drags_selection(app: &PhotocraftApp, at: [f64; 2], mods: egui::Modifiers) -> bool {
    if mods.shift || mods.alt {
        return false;
    }
    let Some(sel) = app.session.active().and_then(|st| st.doc.selection.as_ref()) else { return false };
    sel.sample_channel(at[0].floor() as i32, at[1].floor() as i32, 0) > 0.0
}

/// Whole-pixel offset for a drag from `start` to `end`, limited so the dragged outline stays on the
/// canvas (the engine rejects a patch or move that leaves it).
pub fn patch_offset(app: &mut PhotocraftApp, start: [f64; 2], end: [f64; 2]) -> [i32; 2] {
    let (dx, dy) = ((end[0] - start[0]).round(), (end[1] - start[1]).round());
    let (dx, dy) = if dx.is_finite() && dy.is_finite() { (dx.clamp(-1e7, 1e7) as i32, dy.clamp(-1e7, 1e7) as i32) } else { (0, 0) };
    let Some((canvas, sel)) = app.session.active().and_then(|st| Some((st.doc.bounds(), st.doc.selection.clone()?))) else { return [dx, dy] };
    let key = u64::MAX - app.session.active().map_or(0, |st| st.doc.id.0);
    let b = app.cached_bounds(key, &sel).intersect(&canvas);
    if b.is_empty() {
        return [dx, dy];
    }
    let clamp = |d: i32, lo: i32, hi: i32| if lo > hi { 0 } else { d.clamp(lo, hi) };
    [clamp(dx, canvas.x0 - b.x0, canvas.x1 - b.x1), clamp(dy, canvas.y0 - b.y0, canvas.y1 - b.y1)]
}

/// Patch Tool: the patch was dragged from `start` to `end`.
pub fn finish_patch(app: &mut PhotocraftApp, start: [f64; 2], end: [f64; 2]) {
    let off = patch_offset(app, start, end);
    if off == [0, 0] {
        return;
    }
    let preview = crate::patch_preview::take(app);
    let p = json!({"offset": off, "mode": app.ui.tool_options.patch_mode, "target": crate::canvas::paint_target(app)});
    match app.run("paint.patch", p) {
        Ok(_) => crate::patch_preview::committed(app, preview, off),
        Err(e) => {
            app.ui.status = e;
            app.ui.status_error = true;
        }
    }
}

/// Content-Aware Move Tool: the selection was dragged from `start` to `end`. The engine runs it as
/// a background job (progress dialog, Esc cancels); the selection follows the content.
pub fn finish_content_aware_move(app: &mut PhotocraftApp, start: [f64; 2], end: [f64; 2]) {
    let off = patch_offset(app, start, end);
    if off == [0, 0] {
        return;
    }
    let o = &app.ui.tool_options;
    let p = json!({
        "offset": off,
        "mode": o.cam_mode,
        "structure": o.cam_structure.round().clamp(1.0, 7.0),
        "color": o.cam_color.round().clamp(0.0, 10.0),
        "sampleAllLayers": o.sample_all_layers,
        "target": crate::canvas::paint_target(app),
    });
    if let Err(e) = app.run("paint.contentAwareMove", p) {
        app.ui.status = e;
        app.ui.status_error = true;
    }
}

/// Object Selection: a click, a box or a Ctrl-click refinement through SAM 3.1 when a server
/// answers (`object_select`), the classical rectangle finder otherwise.
pub fn finish_object_selection(app: &mut PhotocraftApp, start: [f64; 2], end: [f64; 2], mods: egui::Modifiers) {
    crate::object_select::finish(app, start, end, mods);
}

/// Is a generative server configured (Preferences › AI Integrations)? The options bar shows the
/// SAM 3.1 controls of the selection tools only then (the default preference names the local
/// server, so this is true out of the box). Under `integrations.engine = native` the question
/// is whether the worker has its runtime and model folders.
pub fn server_configured(app: &PhotocraftApp) -> bool {
    let i = &app.session.prefs().integrations;
    let native = !i.runtime_path.trim().is_empty() && !i.models_path.trim().is_empty();
    match i.engine.trim() {
        "native" => native,
        "comfy" => !i.comfy_server.trim().is_empty(),
        // `auto`: either route can answer a SAM 3.1 request.
        _ => native || !i.comfy_server.trim().is_empty(),
    }
}

/// Could SAM 3.1 requests go to the native worker (`integrations.engine` `native`, or `auto`
/// with the worker's folders set)? Its SAM 3.1 keeps a picture's embedding, so the object finder
/// can ask sooner after the pointer rests.
pub fn native_engine(app: &PhotocraftApp) -> bool {
    let i = &app.session.prefs().integrations;
    match i.engine.trim() {
        "native" => true,
        "comfy" => false,
        _ => !i.runtime_path.trim().is_empty() && !i.models_path.trim().is_empty(),
    }
}

/// The template the selection tools' health check asks about: under `auto` its backend is the
/// native worker when that serves SAM 3.1, the ComfyUI server otherwise.
const HEALTH_TEMPLATE: &str = "sam3.1/segment-point";

/// Is the backend SAM 3.1 would use answering right now (`generate.health`, one quick request; a
/// closed local port answers at once)? A click or Select Subject goes to SAM 3.1 only then, so a
/// machine without ComfyUI or the native graphs keeps the classical behaviour instead of
/// collecting errors.
pub fn server_online(app: &mut PhotocraftApp) -> bool {
    server_configured(app) && app.run("generate.health", json!({"template": HEALTH_TEMPLATE})).is_ok_and(|h| h["ok"].as_bool() == Some(true))
}

/// The Object Selection options bar's "Select by text" field: the phrase through SAM 3.1
/// (`select.byText`, a background job), replacing the selection.
pub fn select_by_text(app: &mut PhotocraftApp) -> Result<Value, String> {
    let phrase = app.ui.tool_options.select_text.trim().to_string();
    if phrase.is_empty() {
        return Err("type what to select first".into());
    }
    let r = app.run("select.byText", json!({"prompt": phrase, "mode": "replace", "sampleAllLayers": app.ui.tool_options.sample_all_layers}));
    if let Err(e) = &r {
        crate::notices::error(app, format!("Select by Text: {e}"));
    }
    r
}

/// Select › Subject and the selection tools' Select Subject button: SAM 3.1's reading of the
/// main subject when the server answers (`select.subjectML`, with the edge refinement the
/// Object Selection tool uses), the classical `select.subject` otherwise.
pub fn select_subject(app: &mut PhotocraftApp) -> Result<Value, String> {
    let refine = app.ui.tool_options.refine_edge;
    if server_online(app)
        && let Ok(v) = app.run("select.subjectML", json!({"refine": refine}))
    {
        return Ok(v);
    }
    app.run("select.subject", json!({}))
}

/// ⌥-click with Clone Stamp / Healing Brush sets the source.
pub fn set_source(app: &mut PhotocraftApp, x: f64, y: f64) {
    app.ui.clone_source = Some([x.round(), y.round()]);
    app.ui.clone_offset = None;
    let _ = app.run("cloneSource.set", json!({"source": [x.round(), y.round()]}));
    app.ui.status = format!("Clone source set at {:.0}, {:.0}", x, y);
    app.ui.status_error = false;
}

/// Where the clone source is sampled from, shown only while a Clone Stamp or Healing Brush stroke
/// is painted, as in Photoshop: setting the source (⌥-click) leaves nothing on the canvas (#668).
pub fn source_marker_point(app: &PhotocraftApp) -> Option<[f64; 2]> {
    if !matches!(app.ui.tool, Tool::CloneStamp | Tool::Healing) {
        return None;
    }
    let at = app.drag.as_ref().filter(|d| matches!(d.tool, Tool::CloneStamp | Tool::Healing)).and_then(|d| d.points.last().map(|p| [p[0], p[1]]))?;
    match (crate::preset_panels::clone_sample_point(app, Some(at)), app.ui.clone_offset, app.ui.clone_source) {
        (Some(p), ..) => Some(p),
        (None, Some(off), _) => Some([at[0] + off[0], at[1] + off[1]]),
        (None, None, s) => s,
    }
}

/// Crosshair at [`source_marker_point`].
pub fn draw_source_marker(app: &PhotocraftApp, painter: &egui::Painter, xf: &ViewXform) {
    let Some(s) = source_marker_point(app) else { return };
    let c = xf.to_screen(s[0] as f32, s[1] as f32);
    for (w, col) in [(3.0, Color32::from_black_alpha(160)), (1.0, Color32::WHITE)] {
        painter.line_segment([c - vec2(7.0, 0.0), c + vec2(7.0, 0.0)], Stroke::new(w, col));
        painter.line_segment([c - vec2(0.0, 7.0), c + vec2(0.0, 7.0)], Stroke::new(w, col));
    }
}

fn opt(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(tl!(&text)).color(t.text_dim).size(12.0));
}

fn pct(ui: &mut egui::Ui, label: &str, v: &mut f32) {
    opt(ui, label);
    crate::widgets::value_field(ui, v, 1.0..=100.0, "%", 58.0);
}

/// Options bar for the retouching and smart-selection tools. Returns false for other tools.
pub fn options_bar(app: &mut PhotocraftApp, ui: &mut egui::Ui, tool: Tool) -> bool {
    if !tool.is_brushlike() && !matches!(tool, Tool::QuickSelection | Tool::ObjectSelection | Tool::Patch | Tool::ContentAwareMove)
        || matches!(tool, Tool::Brush | Tool::Pencil | Tool::MixerBrush | Tool::Eraser)
    {
        return false;
    }
    let server = server_configured(app);
    let o = &mut app.ui.tool_options;
    match tool {
        Tool::SpotHealing => {
            opt(ui, tl!("Type:"));
            for (k, l) in [("contentAware", tl!("Content-Aware")), ("createTexture", tl!("Create Texture")), ("proximityMatch", tl!("Proximity Match"))] {
                let mut on = o.spot_type == k;
                if crate::widgets::checkbox(ui, &mut on, l).clicked() {
                    o.spot_type = k.into();
                }
            }
            crate::widgets::vline(ui, 22.0);
            crate::widgets::checkbox(ui, &mut o.sample_all_layers, tl!("Sample All Layers"));
        }
        Tool::Patch => {
            opt(ui, tl!("Patch:"));
            for (k, l) in [("source", tl!("Source")), ("destination", tl!("Destination"))] {
                let mut on = o.patch_mode == k;
                if crate::widgets::checkbox(ui, &mut on, l).clicked() {
                    o.patch_mode = k.into();
                }
            }
            crate::widgets::vline(ui, 22.0);
            opt(ui, tl!("Lasso around an area, then drag the selection"));
        }
        Tool::ContentAwareMove => {
            opt(ui, tl!("Mode:"));
            let modes = [("move".to_string(), tl!("Move")), ("extend".to_string(), tl!("Extend"))];
            crate::widgets::dropdown(ui, "cam-mode", &mut o.cam_mode, &modes, 90.0);
            opt(ui, tl!("Structure:"));
            crate::widgets::value_field(ui, &mut o.cam_structure, 1.0..=7.0, "", 40.0);
            opt(ui, tl!("Color:"));
            crate::widgets::value_field(ui, &mut o.cam_color, 0.0..=10.0, "", 40.0);
            crate::widgets::checkbox(ui, &mut o.sample_all_layers, tl!("Sample All Layers"));
            crate::widgets::vline(ui, 22.0);
            opt(ui, tl!("Lasso around an area, then drag the selection"));
        }
        Tool::Healing | Tool::CloneStamp => {
            crate::widgets::checkbox(ui, &mut o.clone_aligned, tl!("Aligned"));
            opt(ui, tl!("Sample:"));
            let opts = [
                ("current".to_string(), tl!("Current Layer")),
                ("currentAndBelow".to_string(), tl!("Current & Below")),
                ("all".to_string(), tl!("All Layers")),
            ];
            crate::widgets::dropdown(ui, "clone-sample", &mut o.clone_sample, &opts, 130.0);
            if app.ui.clone_source.is_none() {
                crate::widgets::vline(ui, 22.0);
                opt(ui, &crate::i18n::fmt(tl!("{key}-click to set the source"), &[("key", &crate::shortcuts::pretty("Alt"))]));
            }
        }
        Tool::Dodge | Tool::Burn => {
            opt(ui, tl!("Range:"));
            let opts = [("shadows".to_string(), tl!("Shadows")), ("midtones".to_string(), tl!("Midtones")), ("highlights".to_string(), tl!("Highlights"))];
            crate::widgets::dropdown(ui, "tone-range", &mut o.tone_range, &opts, 100.0);
            pct(ui, tl!("Exposure:"), &mut o.exposure);
            crate::widgets::checkbox(ui, &mut o.protect_tones, tl!("Protect Tones"));
        }
        Tool::Sponge => {
            opt(ui, tl!("Mode:"));
            let opts = [("desaturate".to_string(), tl!("Desaturate")), ("saturate".to_string(), tl!("Saturate"))];
            crate::widgets::dropdown(ui, "sponge-mode", &mut o.sponge_mode, &opts, 110.0);
            crate::widgets::checkbox(ui, &mut o.vibrance, tl!("Vibrance"));
        }
        Tool::Blur | Tool::Sharpen | Tool::Smudge => {
            pct(ui, tl!("Strength:"), &mut o.strength);
            crate::widgets::checkbox(ui, &mut o.sample_all_layers, tl!("Sample All Layers"));
            if tool == Tool::Sharpen {
                crate::widgets::checkbox(ui, &mut o.protect_detail, tl!("Protect Detail"));
            }
            if tool == Tool::Smudge {
                crate::widgets::checkbox(ui, &mut o.finger_painting, tl!("Finger Painting"));
            }
        }
        Tool::HistoryBrush => opt(ui, "Paints from the document's opening state"),
        Tool::QuickSelection => {
            crate::widgets::checkbox(ui, &mut o.sample_all_layers, tl!("Sample All Layers"));
            crate::widgets::checkbox(ui, &mut o.enhance_edge, tl!("Enhance Edge"));
            // With a generative server the stroke goes to SAM 3.1 (`quick_select_detector`).
            if server {
                opt(ui, tl!("Paint over an object to select it"));
            }
            opt(ui, &crate::i18n::fmt(tl!("{key} to subtract"), &[("key", &crate::shortcuts::pretty("Alt"))]));
            crate::widgets::vline(ui, 22.0);
            if crate::widgets::secondary_button(ui, tl!("Select Subject"), 0.0).clicked() {
                let _ = select_subject(app);
            }
        }
        Tool::ObjectSelection => {
            crate::widgets::checkbox(ui, &mut o.sample_all_layers, tl!("Sample All Layers"));
            // With a generative server: the detector's object finder under the pointer, a click
            // or a box selects, Ctrl-click refines, a phrase selects by text (Enter). Without
            // one, the classical rectangle only.
            let mut by_text = false;
            if server {
                // New: click or box an object. Refine: add spots to the last one (⌥ excludes).
                for (on, label, tip) in [
                    (false, tl!("New"), tl!("Click or drag a box around an object")),
                    (true, tl!("Refine"), tl!("Click a spot the object must include; Alt-click one it must not")),
                ] {
                    if crate::widgets::pill_tab(ui, label, o.object_refine == on).on_hover_text(tip).clicked() {
                        o.object_refine = on;
                    }
                }
                crate::widgets::vline(ui, 22.0);
                crate::widgets::checkbox(ui, &mut o.object_finder, tl!("Object finder"))
                    .on_hover_text(tl!("Show what a click would select while the pointer rests"));
                crate::widgets::checkbox(ui, &mut o.refine_edge, tl!("Refine edge"))
                    .on_hover_text(tl!("Soften the detector's hard edge from the pixels (hair, fur)"));
                let field = egui::TextEdit::singleline(&mut o.select_text).hint_text(tl!("Select by text…")).desired_width(150.0).id_salt("object-select-text");
                let resp = ui.add(field);
                by_text = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !o.select_text.trim().is_empty();
                opt(
                    ui,
                    if o.object_refine { tl!("Click to include a spot, Alt-click to exclude one") } else { tl!("Click an object, or drag a box around it") },
                );
            } else {
                opt(ui, tl!("Drag a rectangle around the object"));
            }
            crate::widgets::vline(ui, 22.0);
            let subject = crate::widgets::secondary_button(ui, tl!("Select Subject"), 0.0).clicked();
            if by_text {
                let _ = select_by_text(app);
            } else if subject {
                let _ = select_subject(app);
            }
        }
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::{ToolEvent, tool_event};

    fn app() -> PhotocraftApp {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 100, "height": 60})).unwrap();
        app.run("tools.setBrush", json!({"brush": {"size": 16, "hardness": 1.0}})).unwrap();
        app
    }

    fn drag(app: &mut PhotocraftApp, tool: Tool) {
        app.ui.tool = tool;
        let m = egui::Modifiers::NONE;
        tool_event(app, ToolEvent::Down { x: 10.0, y: 30.0, pressure: 1.0 }, m);
        tool_event(app, ToolEvent::Move { x: 50.0, y: 30.0, pressure: 1.0 }, m);
        tool_event(app, ToolEvent::Up { x: 50.0, y: 30.0 }, m);
        assert!(!app.ui.status_error, "{tool:?}: {}", app.ui.status);
    }

    fn active(app: &PhotocraftApp) -> &photocraft_doc::Layer {
        let st = app.session.active().unwrap();
        st.active_layer.and_then(|id| st.doc.layer(id)).unwrap()
    }

    fn stripes(app: &mut PhotocraftApp, step: usize, target: &str) {
        for x in (0..100).step_by(step) {
            app.run("paint.pencil", json!({"points": [[x, 0], [x, 60]], "size": 2, "color": "#606060", "target": target})).unwrap();
        }
    }

    /// comfy-photocraft: with a generative server configured the Object Selection tool asks
    /// SAM 3.1 for the object under a click (`select.byPoint`), its options bar selects by text
    /// (`select.byText`) and Select Subject goes through the detector; a drag stays the classical
    /// rectangle and without a server nothing changes.
    #[test]
    fn object_selection_click_asks_the_detector_and_the_bar_selects_by_text() {
        let fake = photocraft_genai::fake::FakeComfy::start().unwrap();
        let mut app = app();
        app.ui.tool = Tool::ObjectSelection;
        let none = egui::Modifiers::NONE;
        // No server (the default preference names the local one, which may be running on the
        // machine the tests run on): a click does nothing, quietly.
        app.session.edit_prefs(|p| p.integrations.comfy_server.clear());
        tool_event(&mut app, ToolEvent::Down { x: 40.0, y: 30.0, pressure: 1.0 }, none);
        tool_event(&mut app, ToolEvent::Up { x: 40.0, y: 30.0 }, none);
        assert!(fake.state().prompts.is_empty());
        assert!(!app.ui.status_error, "{}", app.ui.status);
        assert!(app.session.active().unwrap().doc.selection.is_none());
        // A server: the click goes to the detector's point prompt and its mask becomes the selection.
        app.session.edit_prefs(|p| p.integrations.comfy_server = fake.url.clone());
        tool_event(&mut app, ToolEvent::Down { x: 40.0, y: 30.0, pressure: 1.0 }, none);
        tool_event(&mut app, ToolEvent::Up { x: 40.0, y: 30.0 }, none);
        {
            let st = fake.state();
            assert_eq!(st.prompts.len(), 1, "{}", app.ui.status);
            let g = &st.prompts[0].1;
            assert_eq!(g["4"]["class_type"], "SAM3_Detect");
            let coords = g["4"]["inputs"]["positive_coords"].as_str().unwrap_or_default();
            assert!(coords.contains("\"x\":40") && coords.contains("\"y\":30"), "the clicked point as the prompt: {coords}");
        }
        assert!(app.session.active().unwrap().doc.selection.is_some());
        assert_eq!(app.session.journal.last().map(|(id, _)| id.as_str()), Some("select.byPoint"));
        // ⇧-click adds through the same path.
        tool_event(&mut app, ToolEvent::Down { x: 70.0, y: 30.0, pressure: 1.0 }, egui::Modifiers::SHIFT);
        tool_event(&mut app, ToolEvent::Up { x: 70.0, y: 30.0 }, egui::Modifiers::SHIFT);
        assert_eq!(fake.state().prompts.len(), 2);
        assert_eq!(app.session.journal.last().map(|(_, p)| p["mode"].as_str()), Some(Some("add")));
        // A drag is a box prompt to the detector (`object_select` has the rest).
        tool_event(&mut app, ToolEvent::Down { x: 10.0, y: 10.0, pressure: 1.0 }, none);
        tool_event(&mut app, ToolEvent::Move { x: 60.0, y: 50.0, pressure: 1.0 }, none);
        tool_event(&mut app, ToolEvent::Up { x: 60.0, y: 50.0 }, none);
        assert_eq!(fake.state().prompts.len(), 3);
        assert_eq!(app.session.journal.last().map(|(id, p)| (id.as_str(), p["box"].is_array())), Some(("select.byPoint", true)));
        // The options bar's phrase.
        app.ui.tool_options.select_text = "   ".into();
        assert!(select_by_text(&mut app).is_err());
        app.ui.tool_options.select_text = "the dog".into();
        select_by_text(&mut app).unwrap();
        {
            let st = fake.state();
            assert_eq!(st.prompts.len(), 4);
            assert_eq!(st.prompts[3].1["3"]["inputs"]["text"], "the dog");
        }
        // Select Subject: the detector with a server that answers, the classical path otherwise
        // (none configured, or one that is down).
        select_subject(&mut app).unwrap();
        assert_eq!(fake.state().prompts.len(), 5);
        assert_eq!(app.session.journal.last().map(|(id, _)| id.as_str()), Some("select.subjectML"));
        app.session.edit_prefs(|p| p.integrations.comfy_server.clear());
        select_subject(&mut app).unwrap();
        assert_eq!(fake.state().prompts.len(), 5);
        assert_eq!(app.session.journal.last().map(|(id, _)| id.as_str()), Some("select.subject"));
        app.session.edit_prefs(|p| p.integrations.comfy_server = "http://127.0.0.1:9".into());
        tool_event(&mut app, ToolEvent::Down { x: 40.0, y: 30.0, pressure: 1.0 }, none);
        tool_event(&mut app, ToolEvent::Up { x: 40.0, y: 30.0 }, none);
        assert_ne!(app.session.journal.last().map(|(id, _)| id.as_str()), Some("select.byPoint"), "a server that is down: no point prompt");
        assert!(!app.ui.status_error, "{}", app.ui.status);
    }

    /// comfy-photocraft: a Quick Selection stroke is SAM 3.1's point prompt when a server answers
    /// (spots along the stroke, ⌥ subtracts, Enhance Edge is the refinement); without one, or with
    /// one that is down, it stays the classical `select.quick`.
    #[test]
    fn quick_selection_strokes_ask_the_detector_when_a_server_answers() {
        let fake = photocraft_genai::fake::FakeComfy::start().unwrap();
        let mut app = app();
        app.ui.tool = Tool::QuickSelection;
        app.ui.tool_options.enhance_edge = false;
        let none = egui::Modifiers::NONE;
        let stroke = |app: &mut PhotocraftApp, mods: egui::Modifiers| {
            tool_event(app, ToolEvent::Down { x: 10.0, y: 30.0, pressure: 1.0 }, mods);
            for x in (12..=50).step_by(2) {
                tool_event(app, ToolEvent::Move { x: f64::from(x), y: 30.0, pressure: 1.0 }, mods);
            }
            tool_event(app, ToolEvent::Up { x: 50.0, y: 30.0 }, mods);
            assert!(!app.ui.status_error, "{}", app.ui.status);
        };
        // No server: the classical quick selection.
        app.session.edit_prefs(|p| p.integrations.comfy_server.clear());
        stroke(&mut app, none);
        assert_eq!(app.session.journal.last().map(|(id, _)| id.as_str()), Some("select.quick"));
        assert!(fake.state().prompts.is_empty());
        // A server: the stroke's spots become the detector's positive points, mode add.
        app.session.edit_prefs(|p| p.integrations.comfy_server = fake.url.clone());
        app.ui.tool_options.enhance_edge = true;
        stroke(&mut app, none);
        let (id, p) = app.session.journal.last().map(|(id, p)| (id.clone(), p.clone())).unwrap();
        assert_eq!(id, "select.byPoint");
        assert_eq!((p["mode"].as_str(), p["refine"].as_bool()), (Some("add"), Some(true)));
        let pts = p["points"].as_array().unwrap();
        assert!(pts.len() > 1 && pts.len() <= QUICK_STROKE_POINTS, "{} spots", pts.len());
        assert_eq!((pts.first().cloned(), pts.last().cloned()), (Some(json!([10.0, 30.0])), Some(json!([50.0, 30.0]))), "the stroke's ends are in");
        {
            let st = fake.state();
            assert_eq!(st.prompts.len(), 1);
            let coords = st.prompts[0].1["4"]["inputs"]["positive_coords"].as_str().unwrap_or_default();
            assert!(coords.contains("\"x\":10") && coords.contains("\"x\":50"), "{coords}");
        }
        assert!(app.session.active().unwrap().doc.selection.is_some());
        // ⌥: the object under the stroke is subtracted.
        stroke(&mut app, egui::Modifiers::ALT);
        assert_eq!(app.session.journal.last().map(|(_, p)| p["mode"].as_str()), Some(Some("subtract")));
        assert_eq!(fake.state().prompts.len(), 2);
        // A server that is down: the classical path again, quietly.
        app.session.edit_prefs(|p| p.integrations.comfy_server = "http://127.0.0.1:9".into());
        stroke(&mut app, none);
        assert_eq!(app.session.journal.last().map(|(id, _)| id.as_str()), Some("select.quick"));
    }

    #[test]
    fn stroke_prompt_spreads_a_few_whole_pixel_spots_inside_the_canvas() {
        let canvas = [100.0, 60.0];
        // A long stroke is thinned to the cap, ends kept, in order.
        let long: Vec<[f64; 3]> = (0..200).map(|i| [f64::from(i) * 0.4, 20.4, 1.0]).collect();
        let p = stroke_prompt(&long, canvas);
        assert_eq!(p.len(), QUICK_STROKE_POINTS);
        assert_eq!((p[0], p[p.len() - 1]), ([0.0, 20.0], [80.0, 20.0]));
        assert!(p.windows(2).all(|w| w[0][0] < w[1][0]));
        // Points off the canvas are dropped, repeats merged, a dot is one spot.
        assert_eq!(stroke_prompt(&[[-3.0, 5.0, 1.0], [5.2, 5.2, 1.0], [4.8, 4.9, 1.0], [120.0, 5.0, 1.0]], canvas), vec![[5.0, 5.0]]);
        assert!(stroke_prompt(&[[-1.0, -1.0, 1.0], [f64::NAN, 2.0, 1.0]], canvas).is_empty());
        assert!(stroke_prompt(&[], canvas).is_empty());
    }

    #[test]
    fn alt_flips_dodge_burn_and_blur_sharpen() {
        for (tool, flipped) in [(Tool::Dodge, Tool::Burn), (Tool::Burn, Tool::Dodge), (Tool::Blur, Tool::Sharpen), (Tool::Sharpen, Tool::Blur)] {
            assert_eq!(alt_flipped(tool, true), flipped);
            assert_eq!(alt_flipped(tool, false), tool);
        }
        for tool in [Tool::Sponge, Tool::Smudge, Tool::Brush] {
            assert_eq!(alt_flipped(tool, true), tool, "{tool:?} has no ⌥ counterpart");
        }
    }

    #[test]
    fn alt_stroke_runs_the_opposite_tool() {
        // ⌥ held as the stroke starts: Dodge burns, Burn dodges, Blur sharpens, Sharpen blurs.
        for (tool, cmd) in [(Tool::Dodge, "paint.burn"), (Tool::Burn, "paint.dodge"), (Tool::Blur, "paint.sharpen"), (Tool::Sharpen, "paint.blur")] {
            let mut app = app();
            app.ui.tool = tool;
            let m = egui::Modifiers::ALT;
            tool_event(&mut app, ToolEvent::Down { x: 10.0, y: 30.0, pressure: 1.0 }, m);
            tool_event(&mut app, ToolEvent::Move { x: 50.0, y: 30.0, pressure: 1.0 }, m);
            tool_event(&mut app, ToolEvent::Up { x: 50.0, y: 30.0 }, m);
            assert_eq!(app.session.journal.last().map(|(id, _)| id.as_str()), Some(cmd), "{tool:?} with ⌥");
            assert_eq!(app.ui.tool, tool, "the selected tool stays {tool:?}");
        }
    }

    #[test]
    fn clone_without_a_source_names_the_platform_modifier() {
        // Option-click on the Mac, Alt-click elsewhere (#251).
        for tool in [Tool::CloneStamp, Tool::Healing] {
            let mut app = app();
            app.ui.tool = tool;
            assert!(finish_stroke(&mut app, tool, &[[10.0, 30.0, 1.0], [50.0, 30.0, 1.0]], egui::Modifiers::NONE));
            assert!(app.ui.status_error, "{tool:?}");
            let want = if cfg!(target_os = "macos") { "Option-click" } else { "Alt-click" };
            assert!(app.ui.status.starts_with(want), "{tool:?}: {}", app.ui.status);
        }
    }

    #[test]
    fn retouch_strokes_paint_the_targeted_mask() {
        // #207: with the mask targeted, retouching changes the mask, never the pixels.
        for tool in [Tool::Blur, Tool::Sharpen, Tool::Smudge, Tool::Dodge, Tool::Burn] {
            let mut app = app();
            app.run("layer.new.layer", json!({})).unwrap();
            stripes(&mut app, 4, "pixels");
            app.run("layer.layerMask.revealAll", json!({})).unwrap();
            stripes(&mut app, 6, "mask");
            app.ui.mask_target = true;
            app.ui.tool_options.protect_detail = false;
            let b = app.session.active().unwrap().doc.bounds();
            let px0 = active(&app).surface().unwrap().read_region(b);
            let mask0 = active(&app).mask.as_ref().unwrap().surface.read_region(b);
            drag(&mut app, tool);
            assert_eq!(active(&app).surface().unwrap().read_region(b), px0, "{tool:?}: pixels untouched");
            assert_ne!(active(&app).mask.as_ref().unwrap().surface.read_region(b), mask0, "{tool:?}: mask changed");
        }
    }

    #[test]
    fn sample_all_layers_reaches_the_command() {
        // #207, #731: the options-bar checkbox reaches the command.
        for tool in [Tool::SpotHealing, Tool::Blur, Tool::Sharpen, Tool::Smudge] {
            for all in [false, true] {
                let mut app = app();
                stripes(&mut app, 4, "pixels");
                app.run("layer.new.layer", json!({})).unwrap();
                app.ui.tool_options.sample_all_layers = all;
                drag(&mut app, tool);
                let a = active(&app).surface().unwrap().rgba(30, 30)[3];
                assert_eq!(a > 0.0, all, "{tool:?} sampleAllLayers={all}: alpha {a}");
            }
        }
    }

    #[test]
    fn mixer_brush_is_selectable_and_paints_only_inside_the_selection_via_control() {
        use crate::control::{ControlRequest, Outcome, handle};

        let mut app = app();
        app.run("paint.pencil", json!({"points": [[50, 30]], "size": 200, "color": "#204080"})).unwrap();
        app.run("tools.setColors", json!({"foreground": "#f02010"})).unwrap();
        app.run(
            "tools.setBrush",
            json!({
                "pressureSize": false,
                "size": 12,
                "mixer": {"wet": 0.0, "load": 1.0, "mix": 0.0, "flow": 1.0}
            }),
        )
        .unwrap();
        app.run("select.rect", json!({"x": 35, "y": 20, "width": 30, "height": 20})).unwrap();
        let before = {
            let surface = active(&app).surface().unwrap();
            [surface.rgba(20, 30), surface.rgba(50, 30), surface.rgba(80, 30)]
        };

        let ctx = egui::Context::default();
        let (req, _rx) = ControlRequest::new(
            "ui.pointer",
            json!({
                "tool": "mixerBrush",
                "events": [
                    {"kind": "down", "x": 8, "y": 30},
                    {"kind": "move", "x": 92, "y": 30},
                    {"kind": "up", "x": 92, "y": 30}
                ]
            }),
        );
        assert!(matches!(handle(&mut app, &ctx, &req), Outcome::Done(_)));
        assert_eq!(app.ui.tool, Tool::MixerBrush);
        assert_eq!(app.session.journal.last().map(|(id, _)| id.as_str()), Some("paint.mixerBrush"));

        let surface = active(&app).surface().unwrap();
        assert_ne!(surface.rgba(50, 30), before[1], "the selected pixels are mixed");
        assert_eq!(surface.rgba(20, 30), before[0], "outside the selection is unchanged");
        assert_eq!(surface.rgba(80, 30), before[2], "the far side outside the selection is unchanged");
    }

    #[test]
    fn patch_tool_lassoes_then_drags_the_patch() {
        let mut app = app();
        app.run("paint.pencil", json!({"points": [[68, 30], [74, 30]], "size": 6, "color": "#ff0000"})).unwrap();
        let red = |app: &PhotocraftApp| active(app).surface().unwrap().rgba(71, 30)[1] < 0.5;
        assert!(red(&app));
        app.ui.tool = Tool::Patch;
        let m = egui::Modifiers::NONE;
        // Outside any selection the drag is a lasso.
        tool_event(&mut app, ToolEvent::Down { x: 60.0, y: 20.0, pressure: 1.0 }, m);
        for [x, y] in [[84.0, 20.0], [84.0, 40.0], [60.0, 40.0]] {
            tool_event(&mut app, ToolEvent::Move { x, y, pressure: 1.0 }, m);
        }
        tool_event(&mut app, ToolEvent::Up { x: 60.0, y: 40.0 }, m);
        assert!(app.session.active().unwrap().doc.selection.is_some(), "lasso made a selection");
        assert!(red(&app), "the lasso alone changes no pixels");
        // ⇧-drag inside the selection adds to it rather than patching.
        assert!(!patch_drags_selection(&app, [70.0, 30.0], egui::Modifiers::SHIFT));
        // Dragging inside it patches from where it is dropped; the offset is limited to the canvas.
        assert!(patch_drags_selection(&app, [70.0, 30.0], m));
        assert_eq!(patch_offset(&mut app, [70.0, 30.0], [-200.0, 30.0]), [-60, 0]);
        tool_event(&mut app, ToolEvent::Down { x: 70.0, y: 30.0, pressure: 1.0 }, m);
        tool_event(&mut app, ToolEvent::Move { x: 50.0, y: 31.0, pressure: 1.0 }, m);
        tool_event(&mut app, ToolEvent::Up { x: 30.0, y: 30.0 }, m);
        assert!(!app.ui.status_error, "{}", app.ui.status);
        let px = active(&app).surface().unwrap().rgba(71, 30);
        assert!(px[0] > 0.95 && px[1] > 0.95 && px[2] > 0.95, "blemish patched with the white background: {px:?}");
    }

    #[test]
    fn content_aware_move_tool_lassoes_then_moves_or_extends() {
        for (mode, keeps) in [("move", false), ("extend", true)] {
            let mut app = app();
            app.run("paint.pencil", json!({"points": [[20, 30], [24, 30]], "size": 6, "color": "#0000ff"})).unwrap();
            let blue = |app: &PhotocraftApp, x: i32| active(app).surface().unwrap().rgba(x, 30)[0] < 0.5;
            app.ui.tool = Tool::ContentAwareMove;
            app.ui.tool_options.cam_mode = mode.into();
            app.ui.tool_options.cam_structure = 7.0;
            let m = egui::Modifiers::NONE;
            tool_event(&mut app, ToolEvent::Down { x: 12.0, y: 20.0, pressure: 1.0 }, m);
            for [x, y] in [[32.0, 20.0], [32.0, 40.0], [12.0, 40.0]] {
                tool_event(&mut app, ToolEvent::Move { x, y, pressure: 1.0 }, m);
            }
            tool_event(&mut app, ToolEvent::Up { x: 12.0, y: 40.0 }, m);
            assert!(app.session.active().unwrap().doc.selection.is_some(), "lasso made a selection");
            assert!(blue(&app, 22), "the lasso alone changes no pixels");
            tool_event(&mut app, ToolEvent::Down { x: 22.0, y: 30.0, pressure: 1.0 }, m);
            tool_event(&mut app, ToolEvent::Move { x: 50.0, y: 31.0, pressure: 1.0 }, m);
            tool_event(&mut app, ToolEvent::Up { x: 72.0, y: 30.0 }, m);
            assert!(!app.ui.status_error, "{mode}: {}", app.ui.status);
            assert!(blue(&app, 72), "{mode}: the content lands where it was dropped");
            assert_eq!(blue(&app, 22), keeps, "{mode}: the original place");
            assert_eq!(app.session.journal.last().map(|(id, p)| (id.as_str(), p["mode"].as_str())), Some(("paint.contentAwareMove", Some(mode))));
            let sel = app.session.active().unwrap().doc.selection.as_ref().unwrap();
            assert!(sel.sample_channel(72, 30, 0) > 0.0 && sel.sample_channel(22, 30, 0) == 0.0, "{mode}: the selection follows");
        }
    }
}
