//! The generative engine's state in the status bar (plan-native-engine §5.4): a dot coloured by
//! `generate.engine.status` (grey stopped, amber starting, green ready, blue running outside
//! the app, red failed, hollow when no ComfyUI folder is set), its tooltip, and a menu with
//! Start / Stop / Restart / Show log. The commands do the work; this module polls the status
//! every [`POLL_EVERY`] seconds and draws.
//!
//! Start at launch and stop on quit follow Preferences › AI Integrations
//! (`generate_engine_cmds::start_on_launch` / `stop_on_quit`).

use egui::{Color32, Sense, vec2};
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::theme::Tokens;

/// Seconds between status polls (one quick local request each).
pub const POLL_EVERY: f64 = 2.0;

/// What the status bar knows: the last `generate.engine.status` and when it was asked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EngineUi {
    pub status: Option<Value>,
    polled: f64,
}

/// The engine's state name (`unconfigured|stopped|starting|ready|external|failed`), `stopped`
/// before the first poll.
pub fn state(app: &PhotocraftApp) -> String {
    app.ui.engine.status.as_ref().and_then(|v| v["state"].as_str()).unwrap_or("stopped").to_string()
}

/// The tooltip's first line for a state.
pub fn label(state: &str) -> String {
    match state {
        "unconfigured" => tl!("Engine: not configured"),
        "starting" => tl!("Engine: starting…"),
        "ready" => tl!("Engine: ready"),
        "external" => tl!("Engine: running outside the app"),
        "failed" => tl!("Engine: failed"),
        _ => tl!("Engine: stopped"),
    }
    .to_string()
}

/// The dot's colour for a state (`None` = hollow: nothing configured).
fn colour(state: &str, t: &Tokens) -> Option<Color32> {
    Some(match state {
        "unconfigured" => return None,
        "starting" => t.warning,
        "ready" => Color32::from_rgb(0x4c, 0xaf, 0x50),
        "external" => t.accent,
        "failed" => t.danger,
        _ => t.text_faint,
    })
}

/// Poll the status every [`POLL_EVERY`] seconds (faster while starting, so the dot turns green
/// without a pointer move).
pub fn tick(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    let now = ctx.input(|i| i.time);
    let every = if state(app) == "starting" { 1.0 } else { POLL_EVERY };
    if app.ui.engine.status.is_some() && now - app.ui.engine.polled < every {
        return;
    }
    app.ui.engine.polled = now;
    // Straight to the engine: a poll must not touch the status line or the journal.
    app.ui.engine.status = app.session.execute("generate.engine.status", json!({})).ok();
    if state(app) == "starting" {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
}

/// The dot and its menu, in a status bar.
pub fn dot(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let st = state(app);
    let detail = app.ui.engine.status.as_ref().and_then(|v| v["detail"].as_str()).unwrap_or("").to_string();
    let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, t.radius_sm, t.hover);
    }
    match colour(&st, &t) {
        Some(c) => {
            ui.painter().circle_filled(r.center(), 4.0, c);
        }
        None => {
            ui.painter().circle_stroke(r.center(), 4.0, egui::Stroke::new(1.0, t.text_faint));
        }
    }
    let mut tip = label(&st);
    if st == "unconfigured" {
        tip.push('\n');
        tip.push_str(tl!("Set the ComfyUI folder in Preferences › AI Integrations to let the app start it"));
    } else if !detail.is_empty() {
        tip.push('\n');
        tip.push_str(&detail);
    }
    let resp = resp.on_hover_text(tip);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(180.0);
        let configured = st != "unconfigured";
        let running = matches!(st.as_str(), "starting" | "ready");
        let mut cmd = None;
        if ui.add_enabled(configured && !running && st != "external", egui::Button::new(tl!("Start engine"))).clicked() {
            cmd = Some("generate.engine.start");
        }
        if ui.add_enabled(running, egui::Button::new(tl!("Stop engine"))).clicked() {
            cmd = Some("generate.engine.stop");
        }
        if ui.add_enabled(configured && st != "external", egui::Button::new(tl!("Restart engine"))).clicked() {
            cmd = Some("generate.engine.restart");
        }
        ui.separator();
        if ui.button(tl!("Show log")).clicked() {
            show_log(app);
            ui.close();
        }
        if let Some(c) = cmd {
            match app.run(c, json!({})) {
                Ok(v) => app.ui.engine.status = Some(v),
                Err(e) => crate::notices::error(app, e),
            }
            ui.close();
        }
    });
}

/// Reveal the engine's log in the file manager.
fn show_log(app: &mut PhotocraftApp) {
    let path = app.ui.engine.status.as_ref().and_then(|v| v["log"].as_str()).map(str::to_string).unwrap_or_default();
    if path.is_empty() {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (program, args) = photocraft_engine::layer_menu_cmds::reveal_command(&path);
        if let Err(e) = std::process::Command::new(program).args(&args).spawn() {
            crate::notices::error(app, format!("{program}: {e}"));
        }
    }
}

/// At launch: start the engine when the preference says so.
pub fn on_launch(app: &mut PhotocraftApp) {
    if photocraft_engine::generate_engine_cmds::start_on_launch(&app.session) {
        app.ui.engine.status = app.run("generate.engine.start", json!({})).ok();
    }
}

/// At quit: stop the engine the app started when the preference says so (a server started by
/// hand is left alone by the command).
pub fn on_quit(app: &mut PhotocraftApp) {
    if photocraft_engine::generate_engine_cmds::stop_on_quit(&app.session) {
        let _ = app.run("generate.engine.stop", json!({}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_colours_cover_every_state() {
        let t = Tokens::get(&egui::Context::default());
        for s in ["unconfigured", "stopped", "starting", "ready", "external", "failed"] {
            assert!(label(s).starts_with("Engine"), "{s}");
        }
        assert!(colour("unconfigured", &t).is_none());
        assert_eq!(colour("failed", &t), Some(t.danger));
        assert_eq!(colour("ready", &t), Some(Color32::from_rgb(0x4c, 0xaf, 0x50)));
    }

    /// The poll stores `generate.engine.status` (unconfigured by default) and start-at-launch is
    /// a no-op without a folder.
    #[test]
    fn the_status_bar_polls_the_engine() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        let ctx = egui::Context::default();
        on_launch(&mut app);
        assert!(app.ui.engine.status.is_none());
        tick(&mut app, &ctx);
        assert_eq!(state(&app), "unconfigured");
        on_quit(&mut app);
    }
}
