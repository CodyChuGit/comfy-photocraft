use photocraft_genai::fake::{FakeComfy, Options};
use photocraft_geom::Rect;
use serde_json::json;

use super::*;

/// A 64×48 document with one layer, talking to `url`.
fn session(url: &str) -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 64, "height": 48})).unwrap();
    s.execute("layer.new.layer", json!({})).unwrap();
    s.edit_prefs(|p| p.integrations.comfy_server = url.to_string());
    s
}

fn selection_bounds(s: &Session) -> Option<Rect> {
    s.active().unwrap().doc.selection.as_ref().map(|m| m.content_bounds())
}

#[test]
fn by_text_selects_what_the_model_found() {
    // The fake "finds" the centre quarter: x 16..48, y 12..36 on 64×48.
    let fake = FakeComfy::start().unwrap();
    let mut s = session(&fake.url);
    let steps = s.active().unwrap().history.past_len();
    let r = s.execute(BY_TEXT, json!({"prompt": "the lighthouse"})).unwrap();
    assert_eq!(r["selected"], true);
    assert_eq!(r["count"], 1);
    assert_eq!(r["instances"][0]["bounds"], json!([16, 12, 32, 24]));
    assert_eq!(r["instances"][0]["pixels"], 32 * 24);
    assert_eq!(selection_bounds(&s), Some(Rect::new(16, 12, 48, 36)));
    assert_eq!(s.active().unwrap().history.past_len(), steps + 1, "one undo step");

    let st = fake.state();
    assert_eq!(st.uploads.len(), 1, "the composite, no mask");
    let sent = photocraft_genai::png::decode_rgba8(&st.uploads[0].1).unwrap();
    assert_eq!((sent.width, sent.height), (64, 48));
    let g = &st.prompts[0].1;
    assert_eq!(g["3"]["inputs"]["text"], "the lighthouse");
    assert_eq!(g["4"]["class_type"], "SAM3_Detect");
    assert_eq!(g["4"]["inputs"]["threshold"], 0.5);
    assert_eq!(g["4"]["inputs"]["individual_masks"], true);
    assert_eq!(g["1"]["inputs"]["ckpt_name"], "sam3.1_multiplex_fp16.safetensors");
    drop(st);

    assert!(s.undo());
    assert_eq!(selection_bounds(&s), None);
}

#[test]
fn modes_combine_with_the_current_selection() {
    let fake = FakeComfy::start().unwrap();
    let mut s = session(&fake.url);
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 20, "height": 20})).unwrap();
    s.execute(BY_TEXT, json!({"prompt": "x", "mode": "add"})).unwrap();
    assert_eq!(selection_bounds(&s), Some(Rect::new(0, 0, 48, 36)), "union");
    s.execute(BY_TEXT, json!({"prompt": "x", "mode": "intersect"})).unwrap();
    assert_eq!(selection_bounds(&s), Some(Rect::new(16, 12, 48, 36)));
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 64, "height": 48})).unwrap();
    s.execute(BY_TEXT, json!({"prompt": "x", "mode": "subtract"})).unwrap();
    let m = s.active().unwrap().doc.selection.clone().unwrap();
    assert_eq!(m.read_region(Rect::new(30, 20, 31, 21)), vec![0.0], "hole where the object was");
    assert_eq!(m.read_region(Rect::new(2, 2, 3, 3)), vec![1.0]);
    s.execute(BY_TEXT, json!({"prompt": "x", "mode": "replace", "threshold": 0.3})).unwrap();
    assert_eq!(selection_bounds(&s), Some(Rect::new(16, 12, 48, 36)));
    assert_eq!(fake.state().prompts.last().unwrap().1["4"]["inputs"]["threshold"], 0.3);
}

#[test]
fn instances_can_be_picked_one_at_a_time() {
    let fake = FakeComfy::start_with(Options { segments: vec![[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 1.0, 1.0]], ..Options::default() }).unwrap();
    let mut s = session(&fake.url);
    let r = s.execute(BY_TEXT, json!({"prompt": "window"})).unwrap();
    assert_eq!(r["count"], 2);
    assert_eq!(selection_bounds(&s), Some(Rect::new(0, 0, 64, 48)), "all instances by default");
    s.execute(BY_TEXT, json!({"prompt": "window", "instance": 2})).unwrap();
    assert_eq!(selection_bounds(&s), Some(Rect::new(32, 0, 64, 48)));
    let e = s.execute(BY_TEXT, json!({"prompt": "window", "instance": 3})).unwrap_err();
    assert!(e.to_string().contains("only 2 instance"), "{e}");
    assert_eq!(selection_bounds(&s), Some(Rect::new(32, 0, 64, 48)), "a failed pick changes nothing");
}

#[test]
fn nothing_found_is_a_clear_error_that_changes_nothing() {
    let fake = FakeComfy::start_with(Options { segments: vec![], ..Options::default() }).unwrap();
    let mut s = session(&fake.url);
    s.execute("select.rect", json!({"x": 1, "y": 1, "width": 5, "height": 5})).unwrap();
    let steps = s.active().unwrap().history.past_len();
    let e = s.execute(BY_TEXT, json!({"prompt": "unicorn"})).unwrap_err();
    assert!(e.to_string().contains("nothing matching \"unicorn\""), "{e}");
    assert_eq!(selection_bounds(&s), Some(Rect::new(1, 1, 6, 6)));
    assert_eq!(s.active().unwrap().history.past_len(), steps);
}

#[test]
fn select_subject_ml_uses_a_fixed_phrase() {
    let fake = FakeComfy::start().unwrap();
    let mut s = session(&fake.url);
    s.execute(SUBJECT_ML, json!({})).unwrap();
    s.execute(SUBJECT_ML, json!({"what": "sky", "mode": "add"})).unwrap();
    let st = fake.state();
    assert_eq!(st.prompts[0].1["3"]["inputs"]["text"], "the main subject");
    assert_eq!(st.prompts[1].1["3"]["inputs"]["text"], "sky");
    drop(st);
    assert!(matches!(s.execute(SUBJECT_ML, json!({"what": "banana"})), Err(EngineError::BadParams { .. })));
}

#[test]
fn the_active_layer_can_be_sampled_instead_of_the_composite() {
    let fake = FakeComfy::start().unwrap();
    let mut s = session(&fake.url);
    s.execute(BY_TEXT, json!({"prompt": "x", "sampleAllLayers": false})).unwrap();
    let sent = photocraft_genai::png::decode_rgba8(&fake.state().uploads[0].1).unwrap();
    assert_eq!((sent.width, sent.height), (64, 48));
    assert_eq!(sent.get(5, 5), Some([0, 0, 0, 0]), "the empty active layer is transparent, the white Background is not sampled");
}

#[test]
fn parameters_are_validated_and_the_command_needs_a_document() {
    let fake = FakeComfy::start().unwrap();
    let mut s = session(&fake.url);
    for p in [
        json!({}),
        json!({"prompt": ""}),
        json!({"prompt": "x", "mode": "weird"}),
        json!({"prompt": "x", "threshold": 2}),
        json!({"prompt": "x", "instance": 0}),
        json!({"prompt": "x", "instance": 1.5}),
        json!({"prompt": "x", "sampleAllLayers": "yes"}),
        json!({"prompt": "x", "template": "krea2-turbo/image"}),
    ] {
        let e = s.execute(BY_TEXT, p.clone()).unwrap_err();
        assert!(matches!(e, EngineError::BadParams { .. }), "{p}: {e}");
    }
    assert!(fake.state().requests.is_empty());
    let mut empty = Session::new();
    assert!(matches!(empty.execute(BY_TEXT, json!({"prompt": "x"})), Err(EngineError::Disabled(..))));
    let m = s.execute("generate.models", json!({})).unwrap();
    let sam = m["templates"].as_array().unwrap().iter().find(|t| t["id"] == "sam3.1/segment").unwrap();
    assert_eq!(sam["task"], "segment");
    assert_eq!(sam["license"], "community");
}
