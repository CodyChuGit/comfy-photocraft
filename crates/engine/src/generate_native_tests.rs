//! `integrations.engine = native` routes the generative commands to the native worker's channel
//! (plan-native-engine §3.3/§3.4): a fake engine served in-process stands in for the worker.

use std::sync::Arc;

use serde_json::json;

use crate::Session;
use crate::generate_cmds::{engine_mode, set_native_backend};

#[test]
fn the_engine_preference_routes_to_the_native_worker() {
    let (port, token) = photocraft_genai::native::serve_fake(vec!["krea2-turbo/image".into(), "qwen-edit-2511/edit".into(), "sam3.1/segment".into()], 1).unwrap();
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
    assert_eq!(s.execute("generate.health", json!({"template": "krea2-turbo/image"})).unwrap()["backend"], "comfyui");
    let sel = s.execute("select.byText", json!({"prompt": "fox"})).unwrap();
    assert_eq!((sel["count"].as_u64(), sel["selected"].as_bool()), (Some(1), Some(true)), "{sel}");
    // The fake's coverage is a centred disc: the selection's bounds sit inside the 128×64 picture.
    let b = &sel["instances"][0]["bounds"];
    assert!(b[0].as_i64().unwrap_or(-1) >= 40 && b[2].as_i64().unwrap_or(0) <= 48, "{sel}");
}
