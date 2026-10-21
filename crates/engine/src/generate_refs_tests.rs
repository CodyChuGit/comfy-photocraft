//! Reference images on `generate.edit` / `generate.image`, image variations and the time
//! estimate, against the fake server.

use photocraft_doc::LayerId;
use photocraft_genai::fake::FakeComfy;
use serde_json::{Value, json};

use super::*;
use crate::Session;
use crate::generate_cmds::{EDIT, FILL, IMAGE, SIMILAR, estimate_seconds, generative_info};

/// A 64×48 document with a textured layer, a second small layer (the reference) and a
/// selection, talking to `url`.
fn session(url: &str) -> (Session, LayerId, LayerId) {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 64, "height": 48})).unwrap();
    let paint = |s: &mut Session, rect: photocraft_geom::Rect, rgba: [f32; 4]| {
        s.edit("paint", |doc, active| {
            let surf = doc.layer_mut(active.unwrap()).unwrap().surface_mut().unwrap();
            let n = surf.channels();
            let mut tmp = [0.0f32; 8];
            let used = photocraft_raster::from_rgba_into(&surf.format(), rgba, &mut tmp).min(n);
            let px: Vec<f32> = (0..rect.width() * rect.height()).flat_map(|_| tmp[..used].to_vec()).collect();
            surf.write_region(rect, &px);
            Ok(())
        })
        .unwrap();
    };
    s.execute("layer.new.layer", json!({})).unwrap();
    paint(&mut s, photocraft_geom::Rect::new(0, 0, 64, 48), [0.2, 0.6, 0.9, 1.0]);
    let base = s.active().unwrap().active_layer.unwrap();
    s.execute("layer.new.layer", json!({"name": "Hat"})).unwrap();
    paint(&mut s, photocraft_geom::Rect::new(10, 10, 30, 22), [0.9, 0.1, 0.1, 1.0]);
    let hat = s.active().unwrap().active_layer.unwrap();
    s.execute("layer.select", json!({"layer": base.0})).unwrap();
    s.execute("select.rect", json!({"x": 10, "y": 8, "width": 20, "height": 16})).unwrap();
    s.edit_prefs(|p| p.integrations.comfy_server = url.to_string());
    (s, base, hat)
}

fn uploads(fake: &FakeComfy) -> Vec<(String, (u32, u32))> {
    fake.state().uploads.iter().map(|(n, b)| (n.clone(), photocraft_genai::png::decode_rgba8(b).map(|i| (i.width, i.height)).unwrap_or((0, 0)))).collect()
}

#[test]
fn an_edit_sends_its_layer_references_and_similar_sends_them_again() {
    let fake = FakeComfy::start().unwrap();
    let (mut s, base, hat) = session(&fake.url);
    let r = s.execute(EDIT, json!({"prompt": "put the hat from image 2 on the sky", "seed": 3, "references": [{"layer": hat.0}, {"layer": base.0}]})).unwrap();
    let up = uploads(&fake);
    assert_eq!(up.len(), 3, "the picture and two references: {up:?}");
    assert_eq!(up[1].1, (20, 12), "the hat layer over its content bounds");
    {
        let st = fake.state();
        let g = &st.prompts[0].1;
        assert_eq!(g["20"]["inputs"]["image"], st.uploads[1].0);
        assert_eq!(g["21"]["inputs"]["image"], st.uploads[2].0);
        assert_eq!(g["6"]["inputs"]["image2"], json!(["20", 0]));
    }
    let layer = LayerId(r["layer"].as_u64().unwrap());
    let info = generative_info(s.active().unwrap().doc.layer(layer).unwrap()).unwrap();
    assert_eq!(info.references, vec![json!({"layer": hat.0}), json!({"layer": base.0})]);
    // Similar resolves them again.
    s.execute(SIMILAR, json!({"layer": layer.0, "seed": 4})).unwrap();
    let st = fake.state();
    assert_eq!(st.prompts.len(), 2);
    assert!(st.prompts[1].1.get("21").is_some(), "both references again");
}

#[test]
fn reference_counts_and_specs_are_checked_before_anything_runs() {
    let fake = FakeComfy::start().unwrap();
    let (mut s, base, hat) = session(&fake.url);
    let three = json!([{"layer": hat.0}, {"layer": base.0}, {"layer": hat.0}]);
    let e = s.execute(EDIT, json!({"prompt": "x", "references": three})).unwrap_err();
    assert!(matches!(&e, EngineError::BadParams { msg, .. } if msg.contains("at most 2")), "{e}");
    // A fill with references switches to the reference template, which takes two; a plain
    // fill template named outright takes none.
    let e = s.execute(FILL, json!({"prompt": "x", "references": three})).unwrap_err();
    assert!(matches!(&e, EngineError::BadParams { msg, .. } if msg.contains("at most 2")), "{e}");
    let e = s.execute(FILL, json!({"prompt": "x", "template": "qwen-edit-2511/fill", "references": [{"layer": hat.0}]})).unwrap_err();
    assert!(e.to_string().contains("takes no reference images"), "{e}");
    for bad in [json!("x"), json!([5]), json!([{}]), json!([{"layer": 1, "path": "a"}]), json!([{"job": "x"}]), json!([{"path": ""}])] {
        assert!(matches!(s.execute(EDIT, json!({"prompt": "x", "references": bad.clone()})), Err(EngineError::BadParams { .. })), "{bad}");
    }
    let e = s.execute(EDIT, json!({"prompt": "x", "references": [{"layer": 99_999}]})).unwrap_err();
    assert!(e.to_string().contains("reference 1"), "{e}");
    let e = s.execute(EDIT, json!({"prompt": "x", "references": [{"job": 42}]})).unwrap_err();
    assert!(e.to_string().contains("no stored result"), "{e}");
    let e = s.execute(IMAGE, json!({"prompt": "x", "template": "krea2-turbo/image", "references": [{"layer": hat.0}]})).unwrap_err();
    assert!(e.to_string().contains("takes no reference images"), "{e}");
    let e = s.execute(IMAGE, json!({"prompt": "x", "template": "qwen-edit-2511/image-ref"})).unwrap_err();
    assert!(e.to_string().contains("at least 1"), "{e}");
    assert!(fake.state().prompts.is_empty(), "nothing queued");
}

/// A fill given a reference image (no template named) runs the fill-from-reference template:
/// the selection goes out flattened to grey, the reference on image 2, the prompt names the hole;
/// the layer remembers it all and Generate Similar sends the reference again.
#[test]
fn a_fill_with_a_reference_erases_the_selection_and_runs_the_reference_template() {
    let fake = FakeComfy::start().unwrap();
    let (mut s, _, hat) = session(&fake.url);
    let want = crate::generate_cmds::fill_ref_template(crate::generate_cmds::quality(&s));
    let r = s.execute(FILL, json!({"prompt": "the red hat", "seed": 3, "edge": "hard", "references": [{"layer": hat.0}]})).unwrap();
    assert_eq!(r["template"], want);
    let up = uploads(&fake);
    assert_eq!(up.len(), 3, "the picture, its mask and the reference: {up:?}");
    assert_eq!(up[2].1, (20, 12), "the hat layer over its content bounds");
    {
        let st = fake.state();
        let g = &st.prompts[0].1;
        assert_eq!(g["20"]["inputs"]["image"], st.uploads[2].0, "the reference on image 2");
        assert_eq!(g["6"]["inputs"]["image2"], json!(["20", 0]));
        assert!(g.get("21").is_none(), "the third slot pruned");
        let prompt = g["6"]["inputs"]["prompt"].as_str().unwrap_or_default();
        assert!(prompt.contains("flat grey area") && prompt.contains("the red hat"), "{prompt}");
        // Selection 20×16 at (10, 8) with its margin: a 32×32 request from (5, 3), sent at
        // 512²; document (x, y) is request ((x − 5) × 16, (y − 3) × 16).
        let sent = photocraft_genai::png::decode_rgba8(&st.uploads[0].1).unwrap();
        assert_eq!(sent.get(200, 200), Some([128, 128, 128, 255]), "inside the selection: flat grey");
        assert_ne!(sent.get(8, 8).map(|p| [p[0], p[1], p[2]]), Some([128, 128, 128]), "the margin keeps its pixels");
    }
    let layer = LayerId(r["layer"].as_u64().unwrap());
    let info = generative_info(s.active().unwrap().doc.layer(layer).unwrap()).unwrap();
    assert_eq!((info.template.as_str(), info.erase, info.grow), (want, true, 0));
    assert_eq!(info.references, vec![json!({"layer": hat.0})]);
    s.execute(SIMILAR, json!({"layer": layer.0, "seed": 4})).unwrap();
    let st = fake.state();
    assert_eq!(st.prompts.len(), 2);
    assert!(st.prompts[1].1.get("20").is_some(), "the reference again");
    let sent = photocraft_genai::png::decode_rgba8(&st.uploads[3].1).unwrap();
    assert_eq!(sent.get(200, 200), Some([128, 128, 128, 255]), "erased again");
}

/// `grow` widens the selection before the request (and the result's mask with it); `erase`
/// flattens it on any fill template, and off keeps the pixels on the reference template.
#[test]
fn grow_widens_the_selection_and_erase_is_a_choice() {
    let fake = FakeComfy::start().unwrap();
    let (mut s, _, hat) = session(&fake.url);
    let r = s.execute(FILL, json!({"prompt": "x", "seed": 1, "edge": "hard", "grow": 4, "erase": true})).unwrap();
    assert_eq!(r["template"], crate::generate_cmds::likely_auto(FILL, crate::generate_cmds::quality(&s)), "a plain fill still");
    let layer = LayerId(r["layer"].as_u64().unwrap());
    let d = s.active().unwrap();
    let mask = &d.doc.layer(layer).unwrap().mask.as_ref().unwrap().surface;
    assert_eq!(mask.read_region(photocraft_geom::Rect::new(7, 8, 8, 9)), vec![1.0], "3 px left of the selection: grown into");
    assert_eq!(mask.read_region(photocraft_geom::Rect::new(10, 2, 11, 3)), vec![0.0], "6 px above: not");
    let info = generative_info(d.doc.layer(layer).unwrap()).unwrap();
    assert_eq!((info.grow, info.erase), (4, true));
    {
        let st = fake.state();
        let sent = photocraft_genai::png::decode_rgba8(&st.uploads[0].1).unwrap();
        assert_eq!(sent.get(72, 120), Some([128, 128, 128, 255]), "document (9, 10), left of the selection: grown and erased");
    }
    // The reference template with `erase: false`: the pixels stay.
    s.execute(FILL, json!({"prompt": "x", "seed": 1, "erase": false, "references": [{"layer": hat.0}]})).unwrap();
    let st = fake.state();
    let sent = photocraft_genai::png::decode_rgba8(&st.uploads[3].1).unwrap();
    assert_ne!(sent.get(200, 200).map(|p| [p[0], p[1], p[2]]), Some([128, 128, 128]));
    assert!(matches!(s.execute(FILL, json!({"prompt": "x", "grow": 1.5})), Err(EngineError::BadParams { .. })));
    assert!(matches!(s.execute(FILL, json!({"prompt": "x", "grow": 9999})), Err(EngineError::BadParams { .. })));
}

/// Content-Aware Fill, the Remove tool and Spot Healing go through the fill model when asked
/// (`generative`), with the removal instruction and the hole flattened to grey, each as a masked
/// layer; without a backend configured they fall back to their classical fills, in place.
#[test]
fn the_classical_tools_go_through_the_model_when_asked_and_fall_back_without_a_server() {
    let fake = FakeComfy::start().unwrap();
    let (mut s, _, _) = session(&fake.url);
    let layers = s.active().unwrap().doc.layers.len();
    let r = s.execute("edit.contentAwareFill", json!({"generative": true})).unwrap();
    assert!(r["layer"].is_number(), "a generative layer: {r}");
    assert_eq!(s.active().unwrap().doc.layers.len(), layers + 1);
    {
        let st = fake.state();
        let prompt = st.prompts[0].1["6"]["inputs"]["prompt"].as_str().unwrap_or_default();
        assert!(prompt.starts_with("Remove the object in the selected area") && prompt.contains("change nothing else"), "{prompt}");
        // Selection 20×16 at (10, 8): the 32×32 request from (5, 3) at 512²; (200, 200) is inside
        // and keeps the picture's pixels (a grey hole is painted back as a panel: measured).
        let sent = photocraft_genai::png::decode_rgba8(&st.uploads[0].1).unwrap();
        assert_ne!(sent.get(200, 200).map(|p| [p[0], p[1], p[2]]), Some([128, 128, 128]), "the pixels stay");
    }
    let made = LayerId(r["layer"].as_u64().unwrap());
    let info = generative_info(s.active().unwrap().doc.layer(made).unwrap()).unwrap();
    assert_eq!((info.command.as_str(), info.erase, info.name.as_str()), (FILL, false, "Content-Aware Fill"));
    // The Remove tool and Spot Healing: a stroke each, a layer each.
    s.execute("paint.remove", json!({"points": [[20, 16], [26, 16]], "size": 6, "generative": true})).unwrap();
    s.execute("paint.spotHealing", json!({"points": [[22, 18]], "size": 6, "type": "generative"})).unwrap();
    assert_eq!(s.active().unwrap().doc.layers.len(), layers + 3);
    assert_eq!(fake.state().prompts.len(), 3);
    // No backend configured: the classical fills, in place, no layer.
    s.edit_prefs(|p| {
        p.integrations.comfy_server.clear();
        p.integrations.runtime_path.clear();
        p.integrations.models_path.clear();
    });
    let n = s.active().unwrap().doc.layers.len();
    let r = s.execute("edit.contentAwareFill", json!({"generative": true})).unwrap();
    assert!(r.get("filled").is_some() && r.get("runId").is_none(), "the classical fill: {r}");
    s.execute("paint.remove", json!({"points": [[20, 16], [26, 16]], "size": 6, "generative": true})).unwrap();
    s.execute("paint.spotHealing", json!({"points": [[22, 18]], "size": 6, "type": "generative"})).unwrap();
    assert_eq!(s.active().unwrap().doc.layers.len(), n);
    assert_eq!(fake.state().prompts.len(), 3, "nothing more was asked of the server");
}

#[test]
fn an_image_from_a_reference_switches_template_and_variations_fill_one_document() {
    let fake = FakeComfy::start().unwrap();
    let (mut s, _, hat) = session(&fake.url);
    let r = s
        .execute(IMAGE, json!({"prompt": "a portrait wearing the hat from image 1", "target": "document", "width": 96, "height": 64, "variations": 2, "seed": 9, "references": [{"layer": hat.0}]}))
        .unwrap();
    assert_eq!(r["template"], "qwen-edit-2511/image-ref", "the default image template takes no references");
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(96), Some(64)), "the requested size, not the reference's");
    assert_eq!(r["seeds"], json!([9, 10]));
    let d = s.active().unwrap();
    assert_eq!(d.doc.layers.len(), 2);
    assert!(d.doc.layers[0].visible && !d.doc.layers[1].visible, "only the first visible");
    assert_eq!(d.doc.layers[1].name, "Generated (2/2)");
    assert_eq!(r["layers"].as_array().map(Vec::len), Some(2));
    let info = generative_info(&d.doc.layers[0]).unwrap();
    assert_eq!(info.template, "qwen-edit-2511/image-ref");
    assert_eq!(info.references, vec![json!({"layer": hat.0})]);
    let st = fake.state();
    assert_eq!(st.prompts.len(), 2);
    let g = &st.prompts[0].1;
    assert!(g.get("20").is_none() && g.get("21").is_none(), "unused slots pruned");
    assert_eq!(g["10"]["inputs"]["width"], 96);
}

#[test]
fn image_variations_on_a_layer_and_references_from_files_and_generations() {
    let fake = FakeComfy::start().unwrap();
    let (mut s, _, hat) = session(&fake.url);
    s.execute("select.deselect", json!({})).unwrap();
    let r = s.execute(IMAGE, json!({"prompt": "a lighthouse", "target": "layer", "variations": 3, "seed": 1})).unwrap();
    let ids: Vec<u64> = r["layers"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    assert_eq!(ids.len(), 3);
    let d = s.active().unwrap();
    let vis: Vec<bool> = ids.iter().map(|i| d.doc.layer(LayerId(*i)).unwrap().visible).collect();
    assert_eq!(vis, [true, false, false]);
    assert_eq!(d.active_layer, Some(LayerId(ids[0])), "the visible result is active");
    // A file reference and a stored generation (the library in a temp folder).
    let dir = std::env::temp_dir().join(format!("photocraft-refs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    s.set_generations_dir(dir.join("generations"));
    let png = photocraft_genai::png::encode_rgba8(&photocraft_genai::Rgba8::solid(30, 20, [9, 9, 9, 255]).unwrap()).unwrap();
    let file = dir.join("ref.png");
    std::fs::write(&file, png).unwrap();
    s.execute(IMAGE, json!({"prompt": "a boat", "target": "layer", "references": [{"layer": hat.0}]})).unwrap();
    let job = s.generations.records().last().unwrap().id;
    let before = fake.state().uploads.len();
    s.execute(EDIT, json!({"prompt": "x", "references": [{"path": file.to_string_lossy()}, {"job": job}]})).unwrap();
    let up = uploads(&fake);
    assert_eq!(up.len(), before + 3);
    assert_eq!(up[before + 1].1, (30, 20), "the file");
    assert_eq!(up[before + 2].1, (64, 48), "the stored result (the layer image over the canvas)");
    let e = s.execute(EDIT, json!({"prompt": "x", "references": [{"path": dir.join("nope.png").to_string_lossy()}]})).unwrap_err();
    assert!(e.to_string().contains("nope.png"), "{e}");
    s.flush_generations();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_time_estimate_is_one_formula_everywhere() {
    // Lightning-8 fill: 7.5 s + 5.5 s per megapixel, at the size it is sent (≤ 0.75 MP).
    let est = estimate_seconds("qwen-edit-2511/fill-lightning-8", 1000, 1000, 2, 0).unwrap();
    let (w, h) = crate::generate_cmds::request_size(1000, 1000);
    let mp = f64::from(w) * f64::from(h) / 1e6;
    assert!((f64::from(est) - (7.5 + 5.5 * mp) * 2.0).abs() < 0.01, "{est}");
    // References: 9 s each on the Lightning edit.
    let two = estimate_seconds("qwen-edit-2511/edit-lightning-8", 1000, 1000, 1, 2).unwrap();
    assert!((f64::from(two) - (7.5 + 5.5 * mp + 18.0)).abs() < 0.01, "{two}");
    // An image template counts the size asked for.
    assert!((estimate_seconds("krea2-turbo/image", 1000, 1000, 1, 0).unwrap() - 7.0).abs() < 0.01);
    assert!(estimate_seconds("nope/x", 1, 1, 1, 0).is_none());
    // generate.models reports it per template for an area and count.
    let mut s = Session::new();
    let v = s.execute("generate.models", json!({"probe": false, "width": 1000, "height": 1000, "count": 1})).unwrap();
    let krea = v["templates"].as_array().unwrap().iter().find(|t| t["id"] == "krea2-turbo/image").unwrap().clone();
    assert!((krea["eta"].as_f64().unwrap() - 7.0).abs() < 0.01);
    assert_eq!(krea["pages"], json!(["image"]));
    assert_eq!(krea["creator"], "Krea");
    let edit = v["templates"].as_array().unwrap().iter().find(|t| t["id"] == "qwen-edit-2511/edit-lightning-8").unwrap().clone();
    assert_eq!(edit["references"]["max"], 2);
    let v = s.execute("generate.models", json!({"probe": false})).unwrap();
    assert!(v["templates"][0]["eta"].is_null(), "no area, no estimate");
    assert!(matches!(s.execute("generate.models", json!({"probe": false, "width": 10})), Err(EngineError::BadParams { .. })));
}
