//! The generative task bar (comfy-photocraft): Photoshop's contextual bar under a selection,
//! reduced to what the fork does today. A prompt, the template picker, the number of variations,
//! Generate; progress with Cancel while the job runs; the variation switcher once results exist.
//!
//! The bar is data in [`crate::state::UiState`] (`ui.inspect` shows it, `ui.set` drives it) and
//! it only ever runs engine commands (`generate.fill`, `generate.variation`), so the work, the
//! undo history and the journal are the ones a menu would produce. Edit › Generative Fill… and the
//! selection context menu open it (the generated dialog stays for a hidden bar); it also appears
//! by itself when a selection is made, unless Preferences › AI Integrations › Show generative bar
//! is off.

use egui::{Align, Layout, Rect, RichText, Sense, Stroke, pos2, vec2};
use photocraft_engine::jobs::{JobEvent, JobId, JobOutcome};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::theme::Tokens;

pub const COMMAND: &str = "generate.fill";
/// The bar's other mode: the whole picture by instruction, masked to the selection.
pub const EDIT_COMMAND: &str = "generate.edit";
const VARIATION_COMMAND: &str = "generate.variation";
const MODE_FILL: &str = "fill";
const MODE_EDIT: &str = "edit";
const AREA_ID: &str = "generative-bar";
/// Gap between the selection and the bar, and the bar's distance from the canvas edges.
const GAP: f32 = 12.0;
const EDGE: f32 = 8.0;

/// One template the picker offers (from `generate.models`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TemplateChoice {
    pub id: String,
    pub name: String,
    /// Research-licence templates are listed but disabled until the preference allows them.
    pub allowed: bool,
    /// Whether the server has every model file the template needs (None until the background
    /// probe answers, or when no server answers); the picker marks the missing ones.
    #[serde(default)]
    pub installed: Option<bool>,
    /// `fill` or `edit`: which mode of the bar lists it.
    #[serde(default)]
    pub task: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GenerativeBar {
    /// Shown under the selection (× hides it until the next selection is made).
    pub open: bool,
    /// `fill` (regenerate the selection from the prompt) or `edit` (change the whole picture
    /// by instruction, shown inside the selection).
    pub mode: String,
    pub prompt: String,
    /// Template id; empty = Preferences › Default Fill Template.
    pub template: String,
    /// Results per run, 1..=4.
    pub variations: u8,
    /// The `generate.fill` job the bar started and shows progress for.
    pub job: Option<u64>,
    /// Layers of the last run (one per variation) and which one is shown (0-based).
    pub results: Vec<u64>,
    pub shown: usize,
    /// Templates for the picker, fetched when the bar opens.
    pub templates: Vec<TemplateChoice>,
    /// The prompt field takes the keyboard focus on the next frame (opened from a menu).
    #[serde(skip)]
    pub focus: bool,
    /// The document (session index) the job runs on and the results belong to.
    #[serde(skip)]
    job_doc: Option<usize>,
    #[serde(skip)]
    results_doc: Option<usize>,
    /// Whether the active document had a selection last frame (the bar opens when one appears).
    #[serde(skip)]
    had_selection: bool,
    /// The job whose progress the bar drew this frame (the modal progress dialog stays away).
    #[serde(skip)]
    showing_job: Option<u64>,
    /// The background `generate.models` probe filling in [`TemplateChoice::installed`].
    #[serde(skip)]
    probe_job: Option<u64>,
}

impl Default for GenerativeBar {
    fn default() -> Self {
        Self {
            open: false,
            mode: MODE_FILL.to_string(),
            prompt: String::new(),
            template: String::new(),
            variations: 1,
            job: None,
            results: Vec::new(),
            shown: 0,
            templates: Vec::new(),
            focus: false,
            job_doc: None,
            results_doc: None,
            had_selection: false,
            showing_job: None,
            probe_job: None,
        }
    }
}

/// Is the bar drawing job `id`'s progress (so the modal progress dialog is not needed)?
pub fn shows_job(app: &PhotocraftApp, id: JobId) -> bool {
    app.ui.generative_bar.showing_job == Some(id.0)
}

/// Edit › Generative Fill… (and the selection context menu) with no params: open the bar instead
/// of the generated dialog. With params, or with the bar turned off in Preferences, the usual
/// path runs (the dialog from a menu, the engine from automation).
pub fn menu(app: &mut PhotocraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if id != COMMAND || !params.as_object().is_none_or(|o| o.is_empty()) || !app.session.prefs().integrations.show_generative_bar {
        return None;
    }
    Some(open(app))
}

/// Open the bar under the current selection with the keyboard in its prompt field.
pub fn open(app: &mut PhotocraftApp) -> Result<Value, String> {
    if let Some(Err(why)) = photocraft_engine::commands::find(COMMAND).map(|c| (c.enabled)(&app.session)) {
        return Err(why);
    }
    load_templates(app);
    let bar = &mut app.ui.generative_bar;
    bar.open = true;
    bar.focus = true;
    bar.had_selection = true;
    Ok(json!({"generativeBar": true}))
}

/// The fill templates, without contacting the server (this runs on the UI thread).
fn load_templates(app: &mut PhotocraftApp) {
    let Ok(v) = app.session.execute("generate.models", json!({"probe": false})) else { return };
    let templates = v["templates"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|t| t["task"] == MODE_FILL || t["task"] == MODE_EDIT)
                .filter_map(|t| {
                    Some(TemplateChoice {
                        id: t["id"].as_str()?.to_string(),
                        name: t["name"].as_str().unwrap_or_default().to_string(),
                        allowed: t["allowed"].as_bool().unwrap_or(false),
                        installed: None,
                        task: t["task"].as_str().unwrap_or_default().to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    app.ui.generative_bar.templates = templates;
    // Which of them the server has: asked on a worker (the desktop app) so the bar never waits
    // for the server; inline (tests, web) the answer lands at once. No server configured: the
    // badges stay unknown.
    if app.ui.generative_bar.probe_job.is_none() && !app.session.prefs().integrations.comfy_server.trim().is_empty() {
        match app.run("generate.models", json!({"probe": true, "async": true})) {
            Ok(v) if v.get("pending").and_then(Value::as_bool) == Some(true) => app.ui.generative_bar.probe_job = v.get("job").and_then(Value::as_u64),
            Ok(v) => apply_installed(app, &v),
            Err(_) => {}
        }
    }
}

/// Mark each picker template installed or not from a `generate.models` answer (a template is
/// installed when every one of its model files is; unknown stays unknown).
fn apply_installed(app: &mut PhotocraftApp, v: &Value) {
    let Some(list) = v["templates"].as_array() else { return };
    for t in list {
        let Some(id) = t["id"].as_str() else { continue };
        let slots: Vec<Option<bool>> = t["models"].as_array().map(|a| a.iter().map(|m| m["installed"].as_bool()).collect()).unwrap_or_default();
        let installed = if slots.iter().any(Option::is_none) { None } else { Some(slots.iter().all(|s| *s == Some(true))) };
        if let Some(c) = app.ui.generative_bar.templates.iter_mut().find(|c| c.id == id) {
            c.installed = installed;
        }
    }
}

/// The template id the next run uses: the bar's choice or the preference's default (`auto` =
/// the engine picks the fastest permissive tier the server has).
fn current_template(app: &PhotocraftApp) -> String {
    let bar = &app.ui.generative_bar;
    let chosen = bar.template.trim();
    // A chosen template counts only for the mode it belongs to.
    if !chosen.is_empty() && bar.templates.iter().any(|c| c.id == chosen && c.task == bar.mode) {
        return chosen.to_string();
    }
    if bar.mode == MODE_EDIT {
        return photocraft_engine::generate_cmds::AUTO_TEMPLATE.to_string();
    }
    let pref = app.session.prefs().integrations.default_fill_template.trim();
    if pref.is_empty() { photocraft_engine::generate_cmds::AUTO_TEMPLATE.to_string() } else { pref.to_string() }
}

/// The command the bar's mode runs.
fn mode_command(app: &PhotocraftApp) -> &'static str {
    if app.ui.generative_bar.mode == MODE_EDIT { EDIT_COMMAND } else { COMMAND }
}

/// Does a Fill prompt read like an instruction to change what is already there ("make this
/// hyper realistic", "turn it into a painting")? Fill regenerates the selection from scratch,
/// so such prompts belong to Edit; the bar says so.
pub fn looks_like_an_edit(prompt: &str) -> bool {
    let lower = prompt.trim().to_ascii_lowercase();
    let mut words = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty());
    let Some(first) = words.next() else { return false };
    let verbs = ["make", "turn", "convert", "transform", "change", "redraw", "repaint", "restyle", "stylize", "stylise", "render"];
    verbs.contains(&first) && words.any(|w| w == "this" || w == "it" || w == "these" || w == "them")
}

/// Run the bar's command (`generate.fill`, or `generate.edit` in Edit mode) with its prompt,
/// template and variations. In the desktop app it starts a background job the bar then
/// follows; inline (tests, web) the results land at once.
pub fn generate(app: &mut PhotocraftApp) -> Result<Value, String> {
    let prompt = app.ui.generative_bar.prompt.trim().to_string();
    if prompt.is_empty() {
        return Err("type what to generate first".into());
    }
    let variations = app.ui.generative_bar.variations.clamp(1, 4);
    let template = current_template(app);
    let command = mode_command(app);
    let params = json!({"prompt": prompt, "variations": variations, "template": template});
    let doc = app.session.active_index();
    match app.run(command, params) {
        Ok(v) => {
            if v.get("pending").and_then(Value::as_bool) == Some(true) {
                let bar = &mut app.ui.generative_bar;
                bar.job = v.get("job").and_then(Value::as_u64);
                bar.job_doc = doc;
                bar.results.clear();
                bar.shown = 0;
            } else {
                apply_result(app, &v, doc);
            }
            Ok(v)
        }
        Err(e) => {
            crate::notices::error(app, format!("{}: {e}", if command == EDIT_COMMAND { "Generative Edit" } else { "Generative Fill" }));
            Err(e)
        }
    }
}

fn apply_result(app: &mut PhotocraftApp, v: &Value, doc: Option<usize>) {
    let bar = &mut app.ui.generative_bar;
    bar.results = v["layers"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    bar.shown = 0;
    bar.results_doc = doc;
}

/// A background job ended: a fill records its result layers in the bar, whether the bar started
/// it or a script / the dialog did (while the bar has no job of its own).
pub fn on_event(app: &mut PhotocraftApp, e: &JobEvent) {
    if app.ui.generative_bar.probe_job == Some(e.id.0) {
        app.ui.generative_bar.probe_job = None;
        if let JobOutcome::Done(v) = &e.outcome {
            apply_installed(app, v);
        }
        return;
    }
    let active = app.session.active_index();
    let bar = &mut app.ui.generative_bar;
    let own = bar.job == Some(e.id.0);
    if !own && ((e.command != COMMAND && e.command != EDIT_COMMAND) || bar.job.is_some()) {
        return;
    }
    let doc = if own { bar.job_doc.take() } else { active };
    bar.job = None;
    if let JobOutcome::Done(v) = &e.outcome {
        apply_result(app, v, doc);
    }
}

/// The result layers, when they all still exist in the active document.
fn valid_results(app: &PhotocraftApp) -> Vec<u64> {
    let bar = &app.ui.generative_bar;
    if bar.results.len() < 2 || bar.results_doc != app.session.active_index() {
        return Vec::new();
    }
    let Some(d) = app.session.active() else { return Vec::new() };
    if bar.results.iter().all(|id| d.doc.layer(photocraft_doc::LayerId(*id)).is_some()) { bar.results.clone() } else { Vec::new() }
}

/// Show variation `index` (0-based) of the last run: one undo step through `generate.variation`.
pub fn show_variation(app: &mut PhotocraftApp, index: usize) -> Result<Value, String> {
    let layers = valid_results(app);
    if layers.is_empty() {
        return Err("no variations to switch between".into());
    }
    let index = index.min(layers.len() - 1);
    let r = app.run(VARIATION_COMMAND, json!({"layers": layers, "index": index + 1}))?;
    app.ui.generative_bar.shown = index;
    Ok(r)
}

/// `ui.set` fields: `generativeBar` (open), `generativeMode` (`fill` | `edit`),
/// `generativePrompt`, `generativeTemplate`, `generativeVariations` (1..=4).
pub fn set(app: &mut PhotocraftApp, p: &Value) -> Result<(), String> {
    if let Some(v) = p.get("generativeMode") {
        match v.as_str() {
            Some(m) if m == MODE_FILL || m == MODE_EDIT => app.ui.generative_bar.mode = m.to_string(),
            _ => return Err("generativeMode must be \"fill\" or \"edit\"".into()),
        }
    }
    if let Some(v) = p.get("generativeVariations") {
        match v.as_u64() {
            Some(n) if (1..=4).contains(&n) => app.ui.generative_bar.variations = n as u8,
            _ => return Err("generativeVariations must be 1 to 4".into()),
        }
    }
    if let Some(v) = p.get("generativePrompt") {
        let s = v.as_str().ok_or("generativePrompt must be a string")?;
        if s.chars().count() > 4000 {
            return Err("generativePrompt is longer than 4000 characters".into());
        }
        app.ui.generative_bar.prompt = s.to_string();
    }
    if let Some(v) = p.get("generativeTemplate") {
        let s = v.as_str().ok_or("generativeTemplate must be a template id (see generate.models) or empty for the default")?;
        if s.len() > 200 {
            return Err("generativeTemplate is too long".into());
        }
        app.ui.generative_bar.template = s.to_string();
    }
    if let Some(v) = p.get("generativeBar") {
        match v.as_bool() {
            Some(true) => {
                open(app)?;
            }
            Some(false) => app.ui.generative_bar.open = false,
            None => return Err("generativeBar must be true or false".into()),
        }
    }
    Ok(())
}

/// Per frame: open the bar when a selection appears, close it when the selection goes, draw it.
pub fn show(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if app.session.active_index().is_none() {
        app.ui.generative_bar.had_selection = false;
        return;
    }
    let sel_bounds = app.session.active().and_then(|d| d.doc.selection.as_ref()).map(|s| s.content_bounds()).filter(|b| !b.is_empty());
    let has_selection = sel_bounds.is_some();
    let wanted = app.session.prefs().integrations.show_generative_bar;
    app.ui.generative_bar.showing_job = None;
    {
        let bar = &mut app.ui.generative_bar;
        if has_selection && !bar.had_selection && wanted {
            bar.open = true;
        }
        if !has_selection {
            bar.open = false;
            bar.focus = false;
        }
        bar.had_selection = has_selection;
    }
    let Some(bounds) = sel_bounds else { return };
    let busy = app.ui.transform.is_some() || app.ui.text_edit.is_some() || app.jobs.focus.is_some() || !app.ui.dialogs.is_empty();
    if !app.ui.generative_bar.open || busy {
        return;
    }
    // Opened by the selection itself: the picker still needs its template list (offline, cheap).
    if app.ui.generative_bar.templates.is_empty() {
        load_templates(app);
    }
    let Some(xf) = crate::canvas::ViewXform::active(app) else { return };
    let canvas = crate::rulers::content_rect(app, app.last_canvas_rect);
    if canvas.width() < 200.0 || canvas.height() < 80.0 {
        return;
    }
    let sel = xf.doc_rect(bounds);
    let id = egui::Id::new(AREA_ID);
    let size = ctx.memory(|m| m.area_rect(id)).map_or(vec2(560.0, 44.0), |r| r.size());
    let pos = place(sel, size, canvas);
    let t = Tokens::get(ctx);
    let mut action: Option<Action> = None;
    // The bar's own job, or any fill running on this document (started by a script or the dialog).
    let job = app
        .ui
        .generative_bar
        .job
        .map(JobId)
        .and_then(|j| app.session.job(j))
        .or_else(|| app.session.active_job().filter(|j| j.command == COMMAND || j.command == EDIT_COMMAND));
    app.ui.generative_bar.showing_job = job.as_ref().map(|j| j.id.0);
    let results = valid_results(app);
    let default_template = current_template(app);
    let prefs_research = app.session.prefs().integrations.allow_research_models;
    egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        egui::Frame::NONE
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.card_border))
            .corner_radius(t.radius_lg)
            .shadow(ui.style().visuals.popup_shadow)
            .inner_margin(egui::Margin::symmetric(10, 7))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.horizontal(|ui| {
                    let (ir, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                    crate::icons::paint(ui, ir, "sparkles", 16.0, t.accent);
                    match job {
                        Some(j) => running(ui, &j, &t, &mut action),
                        None => idle(ui, app, &results, &default_template, prefs_research, &t, &mut action),
                    }
                    let (xr, xresp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                    if xresp.hovered() {
                        ui.painter().rect_filled(xr, t.radius_sm, t.hover);
                    }
                    crate::icons::paint(ui, xr, "x", 11.0, if xresp.hovered() { t.text } else { t.text_dim });
                    if xresp.on_hover_text(tl!("Close")).clicked() {
                        action = Some(Action::Close);
                    }
                });
            });
    });
    match action {
        Some(Action::Generate) => {
            let _ = generate(app);
        }
        Some(Action::Cancel(j)) => crate::jobs_ui::cancel(app, j),
        Some(Action::Variation(i)) => {
            if let Err(e) = show_variation(app, i) {
                crate::notices::error(app, e);
            }
        }
        Some(Action::Close) => app.ui.generative_bar.open = false,
        None => {}
    }
}

enum Action {
    Generate,
    Cancel(JobId),
    Variation(usize),
    Close,
}

/// Where the bar goes: centred under the selection, above it when there is no room below, and
/// always inside the canvas area.
fn place(sel: Rect, size: egui::Vec2, canvas: Rect) -> egui::Pos2 {
    let x = (sel.center().x - size.x / 2.0).clamp(canvas.left() + EDGE, (canvas.right() - size.x - EDGE).max(canvas.left() + EDGE));
    let below = sel.bottom() + GAP;
    let above = sel.top() - GAP - size.y;
    let y = if below + size.y <= canvas.bottom() - EDGE {
        below.max(canvas.top() + EDGE)
    } else if above >= canvas.top() + EDGE {
        above
    } else {
        canvas.bottom() - EDGE - size.y
    };
    pos2(x.round(), y.round())
}

fn running(ui: &mut egui::Ui, j: &photocraft_engine::jobs::JobInfo, t: &Tokens, action: &mut Option<Action>) {
    let msg = if j.message.is_empty() || j.message == j.label { tl!("Generating…").to_string() } else { j.message.clone() };
    ui.label(RichText::new(msg).color(t.text_dim));
    let (br, _) = ui.allocate_exact_size(vec2(180.0, 6.0), Sense::hover());
    crate::jobs_ui::bar(ui, br, (j.progress > 0.0).then_some(j.progress), t);
    let pct = if j.progress > 0.0 { format!("{:.0}%", j.progress * 100.0) } else { String::new() };
    ui.label(RichText::new(pct).color(t.text_faint).size(11.5).monospace());
    if crate::widgets::secondary_button(ui, tl!("Cancel"), 72.0).clicked() {
        *action = Some(Action::Cancel(j.id));
    }
}

fn idle(ui: &mut egui::Ui, app: &mut PhotocraftApp, results: &[u64], default_template: &str, research: bool, t: &Tokens, action: &mut Option<Action>) {
    let bar = &mut app.ui.generative_bar;
    // Fill (regenerate the selection) or Edit (change the picture by instruction, shown
    // inside the selection).
    let mut mode = if bar.mode == MODE_EDIT { MODE_EDIT.to_string() } else { MODE_FILL.to_string() };
    if crate::widgets::dropdown(ui, "generative-mode", &mut mode, &[(MODE_FILL.to_string(), "Fill"), (MODE_EDIT.to_string(), "Edit")], 64.0) {
        bar.mode = mode.clone();
    }
    // The prompt; Enter generates.
    let hint = if mode == MODE_EDIT { tl!("Describe the change…") } else { tl!("Describe what to generate…") };
    let field = egui::TextEdit::singleline(&mut bar.prompt).hint_text(hint).desired_width(240.0).id_salt("generative-prompt");
    let resp = ui.add(field);
    if bar.focus {
        resp.request_focus();
        bar.focus = false;
    }
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if mode == MODE_FILL && looks_like_an_edit(&bar.prompt) {
        ui.label(RichText::new(tl!("Changing what is there? Switch to Edit")).color(t.text_dim).size(11.5));
    }
    // Template picker: Auto (the fastest permissive tier the server has), then every template
    // of the mode, research ones only when the preference allows.
    let mut current = default_template.to_string();
    // Templates the server lacks files for stay listed (the engine says what is missing) but
    // say so.
    let labels: Vec<(String, String)> = bar
        .templates
        .iter()
        .filter(|c| c.task == mode && (c.allowed || research || c.id == current))
        .map(|c| (c.id.clone(), if c.installed == Some(false) { format!("{} ({})", c.name, tl!("not installed")) } else { c.name.clone() }))
        .collect();
    let mut options: Vec<(String, &str)> = vec![(photocraft_engine::generate_cmds::AUTO_TEMPLATE.to_string(), "Auto")];
    options.extend(labels.iter().map(|(id, l)| (id.clone(), l.as_str())));
    if crate::widgets::dropdown(ui, "generative-template", &mut current, &options, 150.0) {
        bar.template = current;
    }
    // Variations.
    ui.label(RichText::new(tl!("Variations")).color(t.text_dim));
    let mut n = bar.variations.clamp(1, 4);
    crate::widgets::dropdown(ui, "generative-variations", &mut n, &[(1u8, "1"), (2, "2"), (3, "3"), (4, "4")], 44.0);
    bar.variations = n;
    let can_run = !bar.prompt.trim().is_empty();
    if ui.add_enabled_ui(can_run, |ui| crate::widgets::primary_button(ui, tl!("Generate"), 88.0)).inner.clicked() || (enter && can_run) {
        *action = Some(Action::Generate);
    }
    // The variation switcher, once a run made more than one.
    if results.len() > 1 {
        let shown = bar.shown.min(results.len() - 1);
        ui.add_space(4.0);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            if arrow(ui, "chevrons-left", shown > 0, t).on_hover_text(tl!("Previous variation")).clicked() {
                *action = Some(Action::Variation(shown.saturating_sub(1)));
            }
            ui.label(RichText::new(format!("{}/{}", shown + 1, results.len())).color(t.text).monospace());
            if arrow(ui, "chevrons-right", shown + 1 < results.len(), t).on_hover_text(tl!("Next variation")).clicked() {
                *action = Some(Action::Variation(shown + 1));
            }
        });
    }
}

fn arrow(ui: &mut egui::Ui, icon: &str, enabled: bool, t: &Tokens) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), if enabled { Sense::click() } else { Sense::hover() });
    if enabled && resp.hovered() {
        ui.painter().rect_filled(r, t.radius_sm, t.hover);
    }
    crate::icons::paint(ui, r, icon, 12.0, if enabled { t.text } else { t.text_faint });
    resp
}

#[cfg(test)]
#[path = "generative_bar_tests.rs"]
mod tests;
