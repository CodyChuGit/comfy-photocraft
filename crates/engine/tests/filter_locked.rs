//! Filters refuse a pixel-locked (or fully locked) layer and leave it unchanged, as painting
//! does (#1102).

use photocraft_engine::Session;
use serde_json::json;

fn layered() -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 40, "height": 30})).unwrap();
    s.execute("layer.new.layer", json!({})).unwrap();
    s.execute("paint.stroke", json!({"points": [[5, 10], [20, 10]], "size": 8, "hardness": 1.0, "color": "#ff0000"})).unwrap();
    s
}

fn px(s: &Session) -> [f32; 4] {
    let st = s.active().unwrap();
    st.active_layer.and_then(|id| st.doc.layer(id)).unwrap().surface().unwrap().rgba(12, 6)
}

#[test]
fn filters_refuse_a_locked_layer() {
    for lock in [json!({"pixels": true}), json!({"all": true})] {
        let mut s = layered();
        s.execute("layer.lockLayers", lock.clone()).unwrap();
        let before = px(&s);
        let r = s.execute("filter.blur.gaussianBlur", json!({"radius": 4}));
        assert!(r.is_err(), "blurred a layer locked with {lock}: {r:?}");
        assert_eq!(px(&s), before);
    }
}

#[test]
fn filters_still_run_on_an_unlocked_layer() {
    let mut s = layered();
    let before = px(&s);
    s.execute("filter.blur.gaussianBlur", json!({"radius": 4})).unwrap();
    assert_ne!(px(&s), before);
}
