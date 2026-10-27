//! `integrations.engine = native` routes the generative commands to the native worker's channel
//! (plan-native-engine §3.3/§3.4): a fake engine served in-process stands in for the worker.

use std::sync::Arc;

use serde_json::json;

use crate::Session;
use crate::generate_cmds::{engine_mode, set_native_backend};

/// The native worker slot is process-wide: these tests take turns.
static WORKER_SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn the_engine_preference_routes_to_the_native_worker() {
    let _turn = WORKER_SLOT.lock().unwrap_or_else(|e| e.into_inner());
    let (port, token) = photocraft_genai::native::serve_fake(
        vec![
            "krea2-turbo/image".into(),
            "qwen-edit-2511/edit".into(),
            "sam3.1/segment".into(),
            "qwen-2.1/image".into(),
            "qwen-2.1/edit".into(),
            "qwen-2.1/fill".into(),
            "qwen-2.1/matte".into(),
        ],
        1,
    )
    .unwrap();
    set_native_backend(Arc::new(photocraft_genai::native::NativeBackend::attached(port, &token).unwrap()));
    let mut s = Session::new();
    assert_eq!(engine_mode(&s), "auto");
    assert!(s.execute("prefs.set", json!({"path": "integrations.engine", "value": "warp"})).is_err(), "only the listed engines");
    s.execute("prefs.set", json!({"path": "integrations.engine", "value": "native"})).unwrap();
    assert_eq!(engine_mode(&s), "native");
    // Health names the worker's engine.
    let h = s.execute("generate.health", json!({})).unwrap();
    assert_eq!((h["ok"].as_bool(), h["backend"].as_str()), (Some(true), Some("native/fake")), "{h}");
    // Generate Image through the channel: a new document holding the fake's picture.
    let r = s
        .execute("generate.image", json!({"prompt": "a fox", "template": "krea2-turbo/image", "width": 128, "height": 64, "target": "document", "seed": 7}))
        .unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64(), r["template"].as_str()), (Some(128), Some(64), Some("krea2-turbo/image")), "{r}");
    let doc = &s.active().unwrap().doc;
    assert_eq!((doc.size.width, doc.size.height), (128, 64));
    // Qwen-Image-2.1 text to image goes through the channel too (a research-only model: opt in).
    s.execute("prefs.set", json!({"path": "integrations.allowResearchModels", "value": true})).unwrap();
    let r = s.execute("generate.image", json!({"prompt": "a fox", "template": "qwen-2.1/image", "width": 128, "height": 64, "target": "layer", "seed": 9})).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64(), r["template"].as_str()), (Some(128), Some(64), Some("qwen-2.1/image")), "{r}");
    // Generative Edit sends the picture across and gets one back the same size.
    let e = s.execute("generate.edit", json!({"prompt": "make it winter", "template": "qwen-edit-2511/edit", "seed": 3})).unwrap();
    assert_eq!((e["width"].as_u64(), e["height"].as_u64()), (Some(128), Some(64)), "{e}");
    // A template the worker lacks is a sentence, and the backend is still there.
    let err = s.execute("generate.edit", json!({"prompt": "x", "template": "qwen-edit-2511/edit-lightning-8"})).unwrap_err().to_string();
    assert!(err.contains("not installed"), "{err}");
    assert_eq!(s.execute("generate.health", json!({})).unwrap()["ok"], true);
    // Back to the ComfyUI route: health now reports that backend.
    s.execute("prefs.set", json!({"path": "integrations.engine", "value": "comfy"})).unwrap();
    assert_eq!(s.execute("generate.health", json!({})).unwrap()["backend"], "comfyui");
    // Under `auto`, a template the worker serves (SAM 3.1) goes native once the worker has its
    // folders, the rest stay on ComfyUI.
    s.execute("prefs.set", json!({"path": "integrations.engine", "value": "auto"})).unwrap();
    assert_eq!(s.execute("generate.health", json!({"template": "sam3.1/segment"})).unwrap()["backend"], "comfyui", "no folders: ComfyUI");
    s.execute("prefs.set", json!({"path": "integrations.runtimePath", "value": "runtime"})).unwrap();
    s.execute("prefs.set", json!({"path": "integrations.modelsPath", "value": "models"})).unwrap();
    assert_eq!(s.execute("generate.health", json!({})).unwrap()["backend"], "comfyui");
    assert_eq!(s.execute("generate.health", json!({"template": "sam3.1/segment"})).unwrap()["backend"], "native/fake");
    // Krea 2 text to image is in `NATIVE_AUTO_FAMILIES` too (2026-10-27); its style reference,
    // which this worker does not list, stays on ComfyUI.
    assert_eq!(s.execute("generate.health", json!({"template": "krea2-turbo/image"})).unwrap()["backend"], "native/fake");
    assert_eq!(s.execute("generate.health", json!({"template": "krea2-turbo/image-style"})).unwrap()["backend"], "comfyui");
    // Qwen-Image-2.1 text to image is in `NATIVE_AUTO_FAMILIES` and the worker lists it: native,
    // and `generate.image` itself goes there (no ComfyUI server is configured in this test).
    assert_eq!(s.execute("generate.health", json!({"template": "qwen-2.1/image"})).unwrap()["backend"], "native/fake");
    let r = s.execute("generate.image", json!({"prompt": "a fox", "template": "qwen-2.1/image", "width": 128, "height": 64, "target": "layer", "seed": 5})).unwrap();
    assert_eq!(r["template"].as_str(), Some("qwen-2.1/image"), "{r}");
    // Its edit and fill templates are served by the worker too: Generative Edit and Fill with
    // them go native under `auto` (the family is listed and the worker names the templates),
    // while a template the worker does not list stays on ComfyUI.
    assert_eq!(s.execute("generate.health", json!({"template": "qwen-2.1/edit"})).unwrap()["backend"], "native/fake");
    assert_eq!(s.execute("generate.health", json!({"template": "qwen-2.1/fill"})).unwrap()["backend"], "native/fake");
    assert_eq!(s.execute("generate.health", json!({"template": "qwen-2.1/matte"})).unwrap()["backend"], "native/fake");
    assert_eq!(s.execute("generate.health", json!({"template": "qwen-edit-2511/fill"})).unwrap()["backend"], "comfyui");
    let e = s.execute("generate.edit", json!({"prompt": "make it winter", "template": "qwen-2.1/edit", "seed": 3})).unwrap();
    assert_eq!((e["width"].as_u64(), e["height"].as_u64(), e["template"].as_str()), (Some(128), Some(64), Some("qwen-2.1/edit")), "{e}");
    s.execute("select.rect", json!({"x": 10, "y": 10, "width": 40, "height": 30})).unwrap();
    let f = s.execute("generate.fill", json!({"prompt": "a hat", "template": "qwen-2.1/fill", "seed": 4})).unwrap();
    assert_eq!(f["template"].as_str(), Some("qwen-2.1/fill"), "{f}");
    s.execute("select.deselect", json!({})).unwrap();
    // Remove Background under `auto` with research models allowed: the matte template, served
    // natively (`resolve_auto` sees the worker's template list), its alpha as the layer mask.
    let m = s.execute("generate.removeBackground", json!({"seed": 2})).unwrap();
    assert_eq!(m["template"].as_str(), Some("qwen-2.1/matte"), "{m}");
    assert!(m["pixels"].as_u64().unwrap_or(0) > 0 && !m["selection"].as_bool().unwrap_or(true), "{m}");
    let sel = s.execute("select.byText", json!({"prompt": "fox"})).unwrap();
    assert_eq!((sel["count"].as_u64(), sel["selected"].as_bool()), (Some(1), Some(true)), "{sel}");
    // The fake's coverage is a centred disc: the selection's bounds sit inside the 128×64 picture.
    let b = &sel["instances"][0]["bounds"];
    assert!(b[0].as_i64().unwrap_or(-1) >= 40 && b[2].as_i64().unwrap_or(0) <= 48, "{sel}");
    // A removal on the native 2.1 fill still asks the rewriter, which the worker does not
    // serve: the question goes to the ComfyUI route (`thinker_for`), the fill stays native.
    // Measured 2026-10-26: with both on the worker the rewriter's name was lost and the
    // detector fell back to the point prompt.
    let comfy = photocraft_genai::fake::FakeComfy::start().unwrap();
    s.edit_prefs(|p| {
        p.integrations.comfy_server = comfy.url.clone();
        p.integrations.default_fill_template = "qwen-2.1/fill".into();
    });
    s.execute("select.rect", json!({"x": 10, "y": 10, "width": 40, "height": 30})).unwrap();
    let r = s.execute("edit.contentAwareFill", json!({"generative": true, "variations": 1})).unwrap();
    assert_eq!(r["template"].as_str(), Some("qwen-2.1/fill"), "{r}");
    let st = comfy.state();
    assert_eq!(st.prompts.len(), 1, "the rewriter's ask alone reached ComfyUI: {}", st.prompts.len());
    assert_eq!(st.prompts[0].1["3"]["class_type"], "TextGenerate", "{}", st.prompts[0].1);
    drop(st);
    // LoRAs ride along as a request parameter for the native engine (validated here, applied
    // by the worker); the ComfyUI route chains a LoraLoaderModelOnly per file after the model
    // loader, and a malformed list is refused.
    s.execute("select.rect", json!({"x": 10, "y": 10, "width": 40, "height": 30})).unwrap();
    let f = s.execute("generate.fill", json!({"prompt": "a hat", "template": "qwen-2.1/fill", "seed": 4, "loras": [{"file": "qwen-image-2.1-consistency.safetensors", "strength": 0.5}]})).unwrap();
    assert_eq!(f["template"].as_str(), Some("qwen-2.1/fill"), "{f}");
    let f = s.execute("generate.fill", json!({"prompt": "a hat", "template": "qwen-edit-2511/fill", "seed": 4, "loras": [{"file": "x.safetensors"}]})).unwrap();
    assert_eq!(f["template"].as_str(), Some("qwen-edit-2511/fill"), "{f}");
    let st = comfy.state();
    let g = &st.prompts.last().unwrap().1;
    let lora = g.as_object().unwrap().values().find(|n| n["class_type"] == "LoraLoaderModelOnly" && n["inputs"]["lora_name"] == "x.safetensors");
    assert!(lora.is_some_and(|n| n["inputs"]["strength_model"] == 1.0), "{g}");
    drop(st);
    for bad in [json!("x"), json!([{"strength": 1}]), json!([{"file": "../x.safetensors"}]), json!([{"file": "x.safetensors", "strength": 9}])] {
        let e = s.execute("generate.fill", json!({"prompt": "a hat", "template": "qwen-2.1/fill", "loras": bad})).unwrap_err();
        assert!(matches!(e, crate::EngineError::BadParams { .. }), "{e}");
    }
    let t = crate::generate_cmds::Tuning::from_params("x", &json!({"loras": [{"file": "a.safetensors"}, {"file": "b.safetensors", "strength": -1}]})).unwrap();
    assert_eq!(t.loras_param(), Some(json!([{"file": "a.safetensors", "strength": 1.0}, {"file": "b.safetensors", "strength": -1.0}])));
}

/// A worker that serves the enhancer template (its 2.1 text encoder answering as Qwen3-VL)
/// answers a native job's questions itself: nothing reaches the ComfyUI server, and the
/// worker is not told to unload after answering.
#[test]
fn a_worker_that_serves_the_enhancer_answers_its_own_jobs_questions() {
    let _turn = WORKER_SLOT.lock().unwrap_or_else(|e| e.into_inner());
    let (port, token) =
        photocraft_genai::native::serve_fake(vec!["qwen-2.1/fill".into(), "enhance/edit".into(), "enhance/image".into(), "sam3.1/segment".into()], 1).unwrap();
    let worker = Arc::new(photocraft_genai::native::NativeBackend::attached(port, &token).unwrap());
    set_native_backend(Arc::clone(&worker));
    let comfy = photocraft_genai::fake::FakeComfy::start().unwrap();
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 128, "height": 64})).unwrap();
    s.execute("layer.new.layer", json!({})).unwrap();
    s.edit_prefs(|p| {
        p.integrations.engine = "auto".into();
        p.integrations.allow_research_models = true;
        p.integrations.runtime_path = "runtime".into();
        p.integrations.models_path = "models".into();
        p.integrations.comfy_server = comfy.url.clone();
        p.integrations.default_fill_template = "qwen-2.1/fill".into();
    });
    s.execute("select.rect", json!({"x": 10, "y": 10, "width": 40, "height": 30})).unwrap();
    let r = s.execute("edit.contentAwareFill", json!({"generative": true, "variations": 2})).unwrap();
    assert_eq!(r["template"].as_str(), Some("qwen-2.1/fill"), "{r}");
    assert!(comfy.state().prompts.is_empty(), "the rewriter and the judge stayed on the worker");
    assert!(comfy.state().requests.iter().all(|q| q != "POST /free"), "nothing asked ComfyUI to unload");
    // The fill ran after the questions on a worker that was never freed: still healthy, still resident.
    assert_eq!(s.execute("generate.health", json!({"template": "qwen-2.1/fill"})).unwrap()["backend"], "native/fake");
}
