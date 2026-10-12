use egui::vec2;
use egui_kittest::Harness;
use photocraft_genai::fake::FakeComfy;
use serde_json::json;

use super::*;
use crate::control::{ControlRequest, Outcome, handle};

/// A 64×48 document with a textured layer and a small "Hat" layer, talking to `url`; no
/// background jobs (inline runs).
fn app(url: &str) -> (PhotocraftApp, u64, u64) {
    let mut s = photocraft_engine::Session::new();
    s.execute("file.new", json!({"width": 64, "height": 48})).unwrap();
    s.execute("layer.new.layer", json!({})).unwrap();
    s.execute("edit.fill", json!({"color": "#3366aa"})).unwrap();
    let base = s.active().unwrap().active_layer.unwrap().0;
    s.execute("layer.new.layer", json!({"name": "Hat"})).unwrap();
    s.execute("select.rect", json!({"x": 10, "y": 10, "width": 20, "height": 12})).unwrap();
    s.execute("edit.fill", json!({"color": "#cc2222"})).unwrap();
    s.execute("select.deselect", json!({})).unwrap();
    let hat = s.active().unwrap().active_layer.unwrap().0;
    s.execute("layer.select", json!({"layer": base})).unwrap();
    s.edit_prefs(|p| p.integrations.comfy_server = url.to_string());
    let mut app = PhotocraftApp::new(s, crate::Services::default());
    app.sync_views();
    load_catalog(&mut app);
    (app, base, hat)
}

fn control(app: &mut PhotocraftApp, ctx: &egui::Context, method: &str, params: Value) -> Value {
    let (req, _rx) = ControlRequest::new(method, params);
    match handle(app, ctx, &req) {
        Outcome::Done(v) => v,
        _ => panic!("{method}: the reply was deferred"),
    }
}

#[test]
fn image_mode_with_layer_references_runs_the_reference_template_into_a_new_document() {
    let fake = FakeComfy::start().unwrap();
    let (mut app, base, hat) = app(&fake.url);
    app.ui.generate.prompt = "a portrait wearing the hat from image 1".into();
    add_reference(&mut app, json!({"layer": hat}));
    add_reference(&mut app, json!({"layer": base}));
    let (cmd, q) = request(&app).unwrap();
    assert_eq!(cmd, "generate.image");
    assert_eq!(q["target"], "document");
    assert_eq!(q["references"], json!([{"layer": hat}, {"layer": base}]));
    assert!(q.get("template").is_none(), "Auto: the engine switches to the reference template");
    assert_eq!(effective_template(&app, "image"), photocraft_engine::generate_cmds::DEFAULT_IMAGE_REF_TEMPLATE);
    let docs = app.session.documents().len();
    let r = generate(&mut app).unwrap();
    assert_eq!(r["template"], "qwen-edit-2511/image-ref");
    assert_eq!(app.session.documents().len(), docs + 1, "a new document");
    let st = fake.state();
    assert_eq!(st.uploads.len(), 2, "the two references (no picture in image mode)");
    let g = &st.prompts[0].1;
    assert_eq!(g["4"]["inputs"]["image"], st.uploads[0].0);
    assert_eq!(g["20"]["inputs"]["image"], st.uploads[1].0);
    assert!(g.get("21").is_none(), "the third slot pruned");
    assert_eq!((g["10"]["inputs"]["width"].as_u64(), g["10"]["inputs"]["height"].as_u64()), (Some(1024), Some(1024)), "1:1 at 1K");
}

#[test]
fn the_mode_decides_the_command_and_the_deck_follows_the_template() {
    let fake = FakeComfy::start().unwrap();
    let (mut app, _, hat) = app(&fake.url);
    app.ui.generate.prompt = "make it night".into();
    app.ui.generate.mode = "fill".into();
    assert!(request(&app).is_err(), "Fill needs a selection");
    assert_eq!(refs_limit(&app, "fill"), 0, "a fill takes no references: no deck");
    app.run("select.rect", json!({"x": 4, "y": 4, "width": 20, "height": 20})).unwrap();
    assert_eq!(request(&app).unwrap().0, "generate.fill");
    app.ui.generate.mode = "edit".into();
    assert_eq!(refs_limit(&app, "edit"), 2);
    add_reference(&mut app, json!({"layer": hat}));
    let (cmd, q) = request(&app).unwrap();
    assert_eq!((cmd, q["references"].as_array().map(Vec::len)), ("generate.edit", Some(1)));
    generate(&mut app).unwrap();
    assert_eq!(fake.state().uploads.len(), 2, "the picture and the reference");
    // The limit: a third and fourth are refused with a status message.
    add_reference(&mut app, json!({"layer": 1}));
    add_reference(&mut app, json!({"layer": 2}));
    assert_eq!(app.ui.generate.references.len(), 2);
    assert!(app.ui.status.contains("at most 2"), "{}", app.ui.status);
    app.ui.generate.mode = "expand".into();
    app.ui.generate.expand = [0, 0, 32, 0];
    let (cmd, q) = request(&app).unwrap();
    assert_eq!(cmd, "generate.expand");
    assert_eq!((q["right"].as_u64(), q.get("references")), (Some(32), None));
    app.ui.generate.expand = [0; 4];
    assert!(request(&app).is_err(), "nothing to expand");
    app.ui.generate.mode = "image".into();
    app.ui.generate.into = "layer".into();
    app.ui.generate.references.clear();
    assert_eq!(request(&app).unwrap().1["target"], "layer");
}

#[test]
fn the_button_shows_the_formula_estimate() {
    let fake = FakeComfy::start().unwrap();
    let (mut app, _, _) = app(&fake.url);
    app.ui.generate.mode = "image".into();
    app.ui.generate.template.insert("image".into(), "krea2-turbo/image".into());
    app.ui.generate.aspect = "3:2".into();
    app.ui.generate.size = "1.5K".into();
    app.ui.generate.count = 2;
    assert_eq!(image_size(&app), (1536, 1024));
    let want = photocraft_engine::generate_cmds::estimate_seconds("krea2-turbo/image", 1536, 1024, 2, 0).unwrap();
    assert!((estimate(&app, "image").unwrap() - want).abs() < 1e-3);
    assert_eq!(eta_label(want), format!("~{:.0} s", want));
    assert_eq!(eta_label(150.0), "~3 min");
    assert_eq!(eta_label(0.0), "");
    // A custom size snaps to the template's grid and limits.
    app.ui.generate.aspect = "custom".into();
    app.ui.generate.width = 1000;
    app.ui.generate.height = 3000;
    assert_eq!(image_size(&app), (992, 2048));
    // Edit counts the picture at the size it is sent.
    app.ui.generate.mode = "edit".into();
    let edit = estimate(&app, "edit").unwrap();
    let e = photocraft_engine::generate_cmds::estimate_seconds("qwen-edit-2511/edit-lightning-8", 64, 48, 2, 0).unwrap();
    assert!((edit - e).abs() < 1e-3);
}

#[test]
fn ui_set_and_inspect_drive_the_panel_and_share_the_prompt_with_the_bar() {
    let fake = FakeComfy::start().unwrap();
    let (mut app, _, hat) = app(&fake.url);
    let ctx = egui::Context::default();
    let ok = control(
        &mut app,
        &ctx,
        "ui.set",
        json!({"generatePanel": {"mode": "image", "prompt": "a cat", "aspect": "16:9", "size": "1K", "count": 3, "references": [{"layer": hat}], "into": "document"}}),
    );
    assert_eq!(ok["ok"], true, "{ok}");
    let v = control(&mut app, &ctx, "ui.inspect", json!({}));
    let p = &v["result"]["generatePanel"];
    assert_eq!(p["command"], "generate.image");
    assert_eq!(p["imageSize"], json!([1024, 576]));
    assert_eq!(p["params"]["variations"], 3);
    assert_eq!(p["effectiveTemplate"], "qwen-edit-2511/image-ref");
    assert_eq!(p["referencesMax"], 3);
    assert!(p["etaLabel"].as_str().is_some_and(|s| s.starts_with('~')), "{p}");
    // The bar's alias writes the same prompt.
    assert_eq!(control(&mut app, &ctx, "ui.set", json!({"generativePrompt": "a dog"}))["ok"], true);
    assert_eq!(control(&mut app, &ctx, "ui.inspect", json!({}))["result"]["generatePanel"]["prompt"], "a dog");
    // Validation happens before anything changes.
    for bad in [
        json!({"generatePanel": {"mode": "video"}}),
        json!({"generatePanel": {"count": 9}}),
        json!({"generatePanel": {"aspect": "wide"}}),
        json!({"generatePanel": {"template": "sam3.1/segment"}}),
        json!({"generatePanel": {"references": [{"pixels": 1}]}}),
        json!({"generatePanel": {"expand": [1, 2]}}),
        json!({"generatePanel": {"bogus": 1}}),
        json!({"generatePanel": 5}),
    ] {
        assert_eq!(control(&mut app, &ctx, "ui.set", bad.clone())["ok"], false, "{bad}");
    }
    assert_eq!(app.ui.generate.count, 3, "unchanged by the refused calls");
    // Switching the panel to Edit moves the bar too.
    assert_eq!(control(&mut app, &ctx, "ui.set", json!({"generatePanel": {"mode": "edit"}}))["ok"], true);
    assert_eq!(app.ui.generative_bar.mode, "edit");
}

/// Draw the panel alone at `height`.
fn harness(app: PhotocraftApp, height: f32) -> Harness<'static, PhotocraftApp> {
    let mut h = Harness::builder().with_size(vec2(320.0, height)).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            if ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                panel(app, ui);
            }
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, Default::default());
    h.run_steps(4);
    h
}

#[test]
fn enter_generates_only_when_the_preference_says_so_and_ctrl_enter_always() {
    let fake = FakeComfy::start().unwrap();
    let (mut a, _, _) = app(&fake.url);
    a.ui.generate.mode = "image".into();
    a.ui.generate.into = "layer".into();
    a.ui.generate.template.insert("image".into(), "krea2-turbo/image".into());
    a.ui.generate.prompt = "a lighthouse".into();
    a.ui.generate.focus = true;
    let mut h = harness(a, 420.0);
    assert!(!h.state().ui.generate.compact);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(fake.state().prompts.is_empty(), "Enter is a new line by default");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(fake.state().prompts.len(), 1, "Ctrl+Enter generates");
    h.state_mut().session.edit_prefs(|p| p.integrations.enter_to_generate = true);
    h.state_mut().ui.generate.prompt = "a boat".into();
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(fake.state().prompts.len(), 2, "Enter generates with the preference on");
}

#[test]
fn a_short_group_shows_one_row_and_drops_land_in_the_deck() {
    let fake = FakeComfy::start().unwrap();
    let (a, _, _) = app(&fake.url);
    let h = harness(a, 60.0);
    assert!(h.state().ui.generate.compact, "one row");
    let rect = h.state().ui.generate.drop_rect.expect("the panel's rect");
    let target = h.state().drop_target(&egui::Context::default(), Some(rect.center()));
    assert_eq!(target, crate::file_open::DropTarget::References);
    // The bar steps aside while the panel is on screen (auto), and shows when it is hidden.
    let app = h.state();
    assert!(visible(app));
}
