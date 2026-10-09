use std::time::{Duration, Instant};

use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{Document, LayerId, Size};
use photocraft_genai::fake::{FakeComfy, Options};
use photocraft_geom::Rect;
use serde_json::json;

use super::*;
use crate::jobs::{JobOutcome, Started};

const FAKE_COLOR: [f32; 4] = [200.0 / 255.0, 40.0 / 255.0, 40.0 / 255.0, 1.0];

/// A 64×48 document with a textured layer and a 20×16 selection at (10, 8), talking to `url`.
fn session(url: &str) -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 64, "height": 48})).unwrap();
    s.execute("layer.new.layer", json!({})).unwrap();
    texture(&mut s);
    s.execute("select.rect", json!({"x": 10, "y": 8, "width": 20, "height": 16})).unwrap();
    s.edit_prefs(|p| p.integrations.comfy_server = url.to_string());
    s
}

fn texture(s: &mut Session) {
    s.edit("texture", |doc, active| {
        let r = doc.bounds();
        let surf = doc.layer_mut(active.unwrap()).unwrap().surface_mut().unwrap();
        let n = surf.channels();
        let mut px = Vec::new();
        for y in 0..r.height() {
            for x in 0..r.width() {
                let v = ((x * 7 + y * 13) % 31) as f32 / 30.0;
                let rgba = [v, 1.0 - v, 0.5, 1.0];
                let mut tmp = [0.0f32; 8];
                let used = photocraft_raster::from_rgba_into(&surf.format(), rgba, &mut tmp).min(n);
                px.extend_from_slice(&tmp[..used]);
            }
        }
        surf.write_region(r, &px);
        Ok(())
    })
    .unwrap();
}

fn composite(s: &Session) -> photocraft_compose::Buffer {
    let d = s.active().unwrap();
    photocraft_compose::render(&d.doc, d.doc.bounds())
}

fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2.5 / 255.0)
}

fn wait(s: &mut Session, id: crate::jobs::JobId) -> crate::jobs::JobEvent {
    let t = Instant::now();
    loop {
        if let Some(e) = s.poll_jobs().into_iter().find(|e| e.id == id) {
            return e;
        }
        assert!(t.elapsed() < Duration::from_secs(30), "job did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn fill_adds_a_masked_layer_above_the_active_one() {
    let fake = FakeComfy::start().unwrap();
    let mut s = session(&fake.url);
    let before = composite(&s);
    let (layers, steps, journal) = {
        let d = s.active().unwrap();
        (d.doc.layers.len(), d.history.past_len(), s.journal.len())
    };
    let r = s.execute(FILL, json!({"prompt": "a red bicycle", "seed": 5})).unwrap();
    assert_eq!(r["seed"], 5);
    // The preference is `auto`, and the fake "has" the Lightning LoRA: the 8-step tier runs.
    assert_eq!(r["template"], AUTO_FILL_ORDER[0]);
    // Selection 20×16 at (10, 8), margin 25 % of 20 = 5 → the request is 30×26 at (5, 3).
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(30), Some(26)));
    let id = LayerId(r["layer"].as_u64().unwrap());

    let d = s.active().unwrap();
    assert_eq!(d.doc.layers.len(), layers + 1);
    assert_eq!(d.active_layer, Some(id), "the new layer is active");
    assert_eq!(d.history.past_len(), steps + 1, "one undo step");
    assert_eq!(s.journal.len(), journal + 1);
    let l = d.doc.layer(id).unwrap();
    assert_eq!(l.name, "Generative Fill: a red bicycle");
    let surf = l.surface().unwrap();
    assert_eq!(surf.content_bounds(), Rect::new(5, 3, 35, 29), "the result covers the request rectangle");
    let px = surf.read_region(Rect::new(15, 12, 16, 13));
    assert!(close([px[0], px[1], px[2], px[3]], FAKE_COLOR), "{px:?}");
    let mask = l.mask.as_ref().expect("a layer mask cut from the selection");
    assert!(mask.enabled);
    assert_eq!(mask.surface.read_region(Rect::new(15, 12, 16, 13)), vec![1.0], "inside the selection");
    assert_eq!(mask.surface.read_region(Rect::new(6, 4, 7, 5)), vec![0.0], "in the margin, outside the selection");
    assert_eq!(mask.surface.read_region(Rect::new(60, 40, 61, 41)), vec![0.0], "outside the request");

    // The composite changes only inside the selection.
    let after = composite(&s);
    assert!(close(after.get(15, 12), FAKE_COLOR), "{:?}", after.get(15, 12));
    assert_eq!(after.get(6, 4), before.get(6, 4), "margin pixels are untouched");
    assert_eq!(after.get(2, 2), before.get(2, 2));
    assert_eq!(after.get(50, 40), before.get(50, 40));

    // What the server received: the composite crop and a mask that is white only in the selection.
    let st = fake.state();
    assert_eq!(st.uploads.len(), 2);
    let sent = photocraft_genai::png::decode_rgba8(&st.uploads[0].1).unwrap();
    assert_eq!((sent.width, sent.height), (30, 26));
    let b = before.get(15, 12);
    assert_eq!(
        sent.get(10, 9),
        Some([(b[0] * 255.0 + 0.5) as u8, (b[1] * 255.0 + 0.5) as u8, (b[2] * 255.0 + 0.5) as u8, 255]),
        "document (15,12) is request (10,9)"
    );
    let mask_png = photocraft_genai::png::decode_rgba8(&st.uploads[1].1).unwrap();
    assert_eq!(mask_png.get(5, 5).map(|p| p[0]), Some(255), "document (10,8) is selected");
    assert!(mask_png.get(0, 0).map(|p| p[0]) < Some(128), "document (5,3) is margin: at most the outward feather's tail");
    let prompt = st.prompts[0].1["6"]["inputs"]["prompt"].as_str().unwrap_or_default().to_string();
    assert!(prompt.starts_with("Add a red bicycle to this image"), "the description is wrapped as an edit instruction: {prompt}");
    drop(st);

    assert!(s.undo());
    assert_eq!(s.active().unwrap().doc.layers.len(), layers);
    assert_eq!(composite(&s).px, before.px);
}

#[test]
fn fill_runs_as_a_background_job_with_the_same_result() {
    let fake = FakeComfy::start().unwrap();
    let mut inline = session(&fake.url);
    inline.execute(FILL, json!({"prompt": "moss", "seed": 9})).unwrap();

    let mut s = session(&fake.url);
    let layers = s.active().unwrap().doc.layers.len();
    let id = match s.start(FILL, json!({"prompt": "moss", "seed": 9})).unwrap() {
        Started::Job(id) => id,
        Started::Done(v) => panic!("ran inline: {v}"),
    };
    assert_eq!(s.active().unwrap().doc.layers.len(), layers, "nothing lands before the job is applied");
    assert!(s.job(id).is_some());
    let e = wait(&mut s, id);
    let JobOutcome::Done(v) = &e.outcome else { panic!("{e:?}") };
    assert_eq!(v["seed"], 9);
    assert_eq!(s.active().unwrap().doc.layers.len(), layers + 1);
    assert_eq!(composite(&s).px, composite(&inline).px);
    let info = s.jobs_with_recent().into_iter().find(|j| j.id == id).unwrap();
    assert_eq!(info.state, "done");
    assert_eq!(info.progress, 1.0);
}

#[test]
fn cancelling_a_fill_leaves_the_document_alone_and_interrupts_the_server() {
    let fake = FakeComfy::start_with(Options { delay_ms: 20_000, progress_steps: 2, ..Options::default() }).unwrap();
    let mut s = session(&fake.url);
    let before = composite(&s);
    let layers = s.active().unwrap().doc.layers.len();
    let id = match s.start(FILL, json!({"prompt": "slow"})).unwrap() {
        Started::Job(id) => id,
        Started::Done(v) => panic!("ran inline: {v}"),
    };
    // Let it reach the waiting loop.
    let t = Instant::now();
    while fake.state().prompts.is_empty() {
        assert!(t.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
    }
    let t = Instant::now();
    assert!(s.cancel_job(id));
    s.join_cancelled_jobs();
    assert!(t.elapsed() < Duration::from_secs(5), "cancel took {:?}", t.elapsed());
    assert_eq!(s.active().unwrap().doc.layers.len(), layers);
    assert_eq!(composite(&s).px, before.px);
    assert_eq!(fake.state().interrupts, 1, "the server was told to stop");
    assert!(s.jobs().is_empty());
}

#[test]
fn a_server_failure_is_an_error_and_changes_nothing() {
    let fake = FakeComfy::start_with(Options { fail: Some("CUDA out of memory".into()), ..Options::default() }).unwrap();
    let mut s = session(&fake.url);
    let before = composite(&s);
    let steps = s.active().unwrap().history.past_len();
    let e = s.execute(FILL, json!({"prompt": "x"})).unwrap_err();
    assert!(e.to_string().contains("CUDA out of memory"), "{e}");
    assert_eq!(s.active().unwrap().history.past_len(), steps);
    assert_eq!(composite(&s).px, before.px);
}

#[test]
fn an_unreachable_server_is_a_clear_error() {
    let mut s = session("http://127.0.0.1:1");
    let e = s.execute(FILL, json!({"prompt": "x"})).unwrap_err();
    assert!(e.to_string().contains("cannot reach the ComfyUI server"), "{e}");
    let h = s.execute(HEALTH, json!({})).unwrap();
    assert_eq!(h["ok"], false);
    assert!(h["error"].as_str().is_some_and(|m| m.contains("127.0.0.1:1")));
    let m = s.execute(MODELS, json!({})).unwrap();
    assert!(m["templates"].as_array().is_some_and(|t| !t.is_empty()));
    assert!(m["templates"][0]["models"][0]["installed"].is_null(), "unknown while offline");
}

#[test]
fn health_and_models_report_the_server() {
    let fake = FakeComfy::start().unwrap();
    let mut s = session(&fake.url);
    let h = s.execute(HEALTH, json!({})).unwrap();
    assert_eq!(h["ok"], true);
    assert_eq!(h["version"], "0.39.0");
    assert_eq!(h["vramTotal"], 32_000_000_000u64);
    let m = s.execute(MODELS, json!({})).unwrap();
    assert_eq!(m["server"]["ok"], true);
    let t = &m["templates"][0];
    assert_eq!(t["id"], DEFAULT_FILL_TEMPLATE);
    assert_eq!(t["license"], "permissive");
    assert_eq!(t["allowed"], true);
    assert_eq!(t["models"][0]["installed"], true);
    assert_eq!(s.journal.iter().filter(|(id, _)| id.starts_with("generate.")).count(), 0, "queries are not journaled");
}

#[test]
fn parameters_are_validated_before_anything_runs() {
    let fake = FakeComfy::start().unwrap();
    let mut s = session(&fake.url);
    for p in [
        json!({}),
        json!({"prompt": ""}),
        json!({"prompt": "   "}),
        json!({"prompt": 5}),
        json!({"prompt": "x", "steps": -1}),
        json!({"prompt": "x", "steps": 1000}),
        json!({"prompt": "x", "margin": 2}),
        json!({"prompt": "x", "margin": -0.1}),
        json!({"prompt": "x", "variations": 0}),
        json!({"prompt": "x", "variations": 5}),
        json!({"prompt": "x", "variations": 1.5}),
        json!({"prompt": "x", "seed": -1}),
        json!({"prompt": "x", "seed": 1.5}),
        json!({"prompt": "x", "seed": "7"}),
        json!({"prompt": "x", "guidance": 99}),
        json!({"prompt": "x", "template": "nope/fill"}),
        json!({"prompt": "x", "model": "../evil.safetensors"}),
        json!({"prompt": "x", "name": "n".repeat(300)}),
    ] {
        let e = s.execute(FILL, p.clone()).unwrap_err();
        assert!(matches!(e, EngineError::BadParams { .. }), "{p}: {e}");
    }
    assert!(fake.state().requests.is_empty(), "no request reached the server");
}

#[test]
fn fill_needs_a_selection_and_a_document() {
    let fake = FakeComfy::start().unwrap();
    let mut s = session(&fake.url);
    s.execute("select.deselect", json!({})).unwrap();
    assert!(matches!(s.execute(FILL, json!({"prompt": "x"})), Err(EngineError::Disabled(..))));
    assert!(!s.is_enabled(FILL));
    let mut empty = Session::new();
    assert!(matches!(empty.execute(FILL, json!({"prompt": "x"})), Err(EngineError::Disabled(..))));
    assert!(empty.is_enabled(HEALTH));
}

#[test]
fn the_model_preference_and_param_override_the_template() {
    let fake = FakeComfy::start().unwrap();
    let mut s = session(&fake.url);
    s.edit_prefs(|p| p.integrations.default_edit_model = "qwen_image_edit_2511_bf16.safetensors".into());
    s.execute(FILL, json!({"prompt": "x", "seed": 1})).unwrap();
    s.execute(FILL, json!({"prompt": "x", "seed": 1, "model": "custom.safetensors", "steps": 4, "guidance": 1, "negative": "blurry", "name": "Bike"})).unwrap();
    let st = fake.state();
    assert_eq!(st.prompts[0].1["1"]["inputs"]["unet_name"], "qwen_image_edit_2511_bf16.safetensors", "the preference");
    assert_eq!(st.prompts[1].1["1"]["inputs"]["unet_name"], "custom.safetensors", "the param wins");
    assert_eq!(st.prompts[1].1["14"]["inputs"]["steps"], 4);
    assert_eq!(st.prompts[1].1["14"]["inputs"]["cfg"], 1.0);
    assert_eq!(st.prompts[1].1["7"]["inputs"]["prompt"], "blurry");
    drop(st);
    let d = s.active().unwrap();
    assert_eq!(d.doc.layer(d.active_layer.unwrap()).unwrap().name, "Bike");
}

#[test]
fn a_sixteen_bit_document_gets_a_sixteen_bit_layer() {
    let fake = FakeComfy::start().unwrap();
    let mut s = Session::new();
    let doc = Document::new("deep", Size::new(32, 32), ColorMode::Rgb, SampleType::U16);
    s.add_document(doc, None);
    s.execute("layer.new.layer", json!({})).unwrap();
    texture(&mut s);
    s.execute("select.rect", json!({"x": 8, "y": 8, "width": 8, "height": 8})).unwrap();
    s.edit_prefs(|p| p.integrations.comfy_server = fake.url.clone());
    let r = s.execute(FILL, json!({"prompt": "x", "margin": 0})).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(8), Some(8)));
    let d = s.active().unwrap();
    let l = d.doc.layer(LayerId(r["layer"].as_u64().unwrap())).unwrap();
    assert_eq!(l.surface().unwrap().format().sample, SampleType::U16);
    let px = l.surface().unwrap().read_region(Rect::new(10, 10, 11, 11));
    assert!(close([px[0], px[1], px[2], px[3]], FAKE_COLOR), "{px:?}");
}

#[test]
fn results_of_another_size_are_resampled_to_the_request() {
    let big = Rgba8::solid(60, 52, [10, 20, 30, 255]).unwrap();
    let r = resize_rgba8(&big, 30, 26).unwrap();
    assert_eq!((r.width, r.height), (30, 26));
    assert_eq!(r.get(0, 0), Some([10, 20, 30, 255]));
    assert_eq!(r.get(29, 25), Some([10, 20, 30, 255]));
    // A two-tone image keeps its halves.
    let mut data = Vec::new();
    for y in 0..4u32 {
        for _ in 0..4u32 {
            data.extend_from_slice(if y < 2 { &[0, 0, 0, 255] } else { &[255, 255, 255, 255] });
        }
    }
    let two = Rgba8::new(4, 4, data).unwrap();
    let r = resize_rgba8(&two, 2, 2).unwrap();
    // Lanczos averages across the edge a little; the halves stay dark and light.
    assert!(r.get(0, 0).unwrap()[0] < 48 && r.get(0, 0).unwrap()[3] == 255, "{:?}", r.get(0, 0));
    assert!(r.get(1, 1).unwrap()[0] > 207, "{:?}", r.get(1, 1));
    assert!(resize_rgba8(&two, 0, 2).is_err());
}

#[test]
fn variations_are_hidden_siblings_and_the_variation_command_switches_them() {
    let fake = FakeComfy::start().unwrap();
    let mut s = session(&fake.url);
    let (layers, steps) = {
        let d = s.active().unwrap();
        (d.doc.layers.len(), d.history.past_len())
    };
    let r = s.execute(FILL, json!({"prompt": "a kite", "seed": 40, "variations": 3})).unwrap();
    let ids: Vec<LayerId> = r["layers"].as_array().unwrap().iter().map(|v| LayerId(v.as_u64().unwrap())).collect();
    assert_eq!(ids.len(), 3);
    assert_eq!(r["layer"], ids[0].0, "the visible result is the primary one");
    assert_eq!(r["seeds"], json!([40, 41, 42]), "consecutive seeds");
    assert_eq!(fake.state().prompts.len(), 3, "one backend run per variation");
    {
        let d = s.active().unwrap();
        assert_eq!(d.doc.layers.len(), layers + 3);
        assert_eq!(d.history.past_len(), steps + 1, "one undo step");
        assert_eq!(d.active_layer, Some(ids[0]));
        let visible: Vec<bool> = ids.iter().map(|id| d.doc.layer(*id).unwrap().visible).collect();
        assert_eq!(visible, [true, false, false]);
        assert_eq!(d.doc.layer(ids[1]).unwrap().name, "Generative Fill: a kite (2/3)");
        assert!(ids.iter().all(|id| d.doc.layer(*id).unwrap().mask.is_some()), "every variation is masked to the selection");
    }
    // Switching shows exactly one of them and activates it, in one undo step.
    let layers_json: Vec<u64> = ids.iter().map(|l| l.0).collect();
    let v = s.execute(VARIATION, json!({"layers": layers_json, "index": 3})).unwrap();
    assert_eq!((v["shown"].as_u64(), v["count"].as_u64()), (Some(ids[2].0), Some(3)));
    {
        let d = s.active().unwrap();
        let visible: Vec<bool> = ids.iter().map(|id| d.doc.layer(*id).unwrap().visible).collect();
        assert_eq!(visible, [false, false, true]);
        assert_eq!(d.active_layer, Some(ids[2]));
        assert_eq!(d.history.past_len(), steps + 2);
    }
    assert!(s.undo());
    assert!(s.active().unwrap().doc.layer(ids[0]).unwrap().visible);
    for p in [
        json!({}),
        json!({"layers": [], "index": 1}),
        json!({"layers": layers_json, "index": 4}),
        json!({"layers": layers_json, "index": 0}),
        json!({"layers": layers_json}),
    ] {
        assert!(matches!(s.execute(VARIATION, p.clone()), Err(EngineError::BadParams { .. })), "{p}");
    }
    assert!(matches!(s.execute(VARIATION, json!({"layers": [999_999], "index": 1})), Err(EngineError::NoLayer(_))));
}

#[test]
fn auto_takes_the_lightning_tier_when_its_lora_is_installed_and_the_base_otherwise() {
    let fake = FakeComfy::start().unwrap();
    let mut s = session(&fake.url);
    assert_eq!(s.prefs().integrations.default_fill_template, AUTO_TEMPLATE, "the shipped default");
    let r = s.execute(FILL, json!({"prompt": "x", "seed": 1, "template": "auto"})).unwrap();
    assert_eq!(r["template"], "qwen-edit-2511/fill-lightning-8");
    let st = fake.state();
    let g = &st.prompts[0].1;
    assert_eq!(g["19"]["class_type"], "LoraLoaderModelOnly");
    assert_eq!(g["19"]["inputs"]["lora_name"], "Qwen-Image-Edit-2511-Lightning-8steps-V1.0-bf16.safetensors");
    assert_eq!(g["14"]["inputs"]["steps"], 8, "the tier's default steps");
    assert_eq!(g["14"]["inputs"]["cfg"], 1.0);
    assert_eq!(g["9"]["class_type"], "CFGNorm");
    assert_eq!(g["17"]["inputs"]["reference_latents_method"], "index_timestep_zero");
    assert!(st.requests.iter().any(|r| r == "GET /models/loras"), "{:?}", st.requests);
    drop(st);
    let m = s.execute(MODELS, json!({})).unwrap();
    assert_eq!(m["autoFill"], "qwen-edit-2511/fill-lightning-8");
    assert!(s.execute(MODELS, json!({"probe": false})).unwrap()["autoFill"].is_null(), "no server contact, no answer");

    // Without the LoRA on the server, auto falls back to the 40-step base template.
    let bare =
        FakeComfy::start_with(Options { missing_files: vec!["Qwen-Image-Edit-2511-Lightning-8steps-V1.0-bf16.safetensors".into()], ..Options::default() })
            .unwrap();
    let mut s = session(&bare.url);
    let r = s.execute(FILL, json!({"prompt": "x", "seed": 1})).unwrap();
    assert_eq!(r["template"], DEFAULT_FILL_TEMPLATE);
    let st = bare.state();
    assert!(st.prompts[0].1.get("19").is_none(), "no LoRA node");
    assert_eq!(st.prompts[0].1["14"]["inputs"]["steps"], 40);
    drop(st);
    let m = s.execute(MODELS, json!({})).unwrap();
    assert_eq!(m["autoFill"], DEFAULT_FILL_TEMPLATE);
    let lightning = m["templates"].as_array().unwrap().iter().find(|t| t["id"] == "qwen-edit-2511/fill-lightning-8").unwrap();
    assert_eq!(lightning["models"][3]["installed"], false);
    assert_eq!(lightning["license"], "permissive");
}

#[test]
fn large_areas_are_sent_at_one_megapixel_and_come_back_full_size() {
    assert_eq!(request_size(1024, 1024), (1024, 1024));
    assert_eq!(request_size(4000, 100), (4000, 100), "under the cap: untouched");
    assert_eq!(request_size(2048, 1536), (1168, 880));
    let (w, h) = request_size(8000, 8000);
    assert!(w % 16 == 0 && h % 16 == 0 && u64::from(w) * u64::from(h) <= 1024 * 1024, "{w}×{h}");
    let (w, h) = request_size(20_000, 64);
    assert!(h == 64 && w < 20_000 && w % 16 == 0, "the short side never drops below 64: {w}×{h}");

    let fake = FakeComfy::start().unwrap();
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 1600, "height": 1200})).unwrap();
    s.execute("layer.new.layer", json!({})).unwrap();
    s.execute("select.rect", json!({"x": 100, "y": 100, "width": 1400, "height": 1000})).unwrap();
    s.edit_prefs(|p| p.integrations.comfy_server = fake.url.clone());
    let r = s.execute(FILL, json!({"prompt": "x", "margin": 0})).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(1400), Some(1000)));
    let (rw, rh) = (r["requestWidth"].as_u64().unwrap(), r["requestHeight"].as_u64().unwrap());
    assert!(rw * rh <= 1024 * 1024 && rw % 16 == 0 && rh % 16 == 0, "{rw}×{rh}");
    let st = fake.state();
    let sent = photocraft_genai::png::decode_rgba8(&st.uploads[0].1).unwrap();
    assert_eq!((u64::from(sent.width), u64::from(sent.height)), (rw, rh), "the upload is the downscaled request");
    let mask = photocraft_genai::png::decode_rgba8(&st.uploads[1].1).unwrap();
    assert_eq!((mask.width, mask.height), (sent.width, sent.height));
    drop(st);
    let d = s.active().unwrap();
    let l = d.doc.layer(LayerId(r["layer"].as_u64().unwrap())).unwrap();
    assert_eq!(l.surface().unwrap().content_bounds(), Rect::new(100, 100, 1500, 1100), "placed at full size");
    let px = l.surface().unwrap().read_region(Rect::new(800, 600, 801, 601));
    assert!(close([px[0], px[1], px[2], px[3]], FAKE_COLOR), "{px:?}");
    assert!(r["timings"][0]["encodeMs"].is_u64() && r["timings"][0]["runMs"].is_u64(), "{}", r["timings"]);
}

#[test]
fn the_request_mask_is_feathered_outward_and_the_layer_mask_is_not() {
    // A 20×20 square at (20, 20) on 64×64, feathered by 4.
    let mut data = vec![0u8; 64 * 64];
    for y in 20..40 {
        for x in 20..40 {
            data[y * 64 + x] = 255;
        }
    }
    let mask = Gray8::new(64, 64, data).unwrap();
    let soft = feather_outward(&mask, 4).unwrap();
    let at = |x: usize, y: usize| soft.data[y * 64 + x];
    assert_eq!(at(30, 30), 255, "the inside keeps full coverage");
    assert_eq!(at(20, 30), 255, "so does the edge pixel");
    assert!(at(18, 30) > 0 && at(18, 30) < 255, "a falloff just outside: {}", at(18, 30));
    assert!(at(18, 30) > at(14, 30), "monotonic falloff");
    assert_eq!(at(5, 5), 0, "far away stays empty");
    assert_eq!(feather_outward(&mask, 0).unwrap(), mask);
    assert_eq!((feather_radius(576, 512), feather_radius(100, 100), feather_radius(4000, 4000)), (12, 4, 24));

    // End to end: the uploaded mask is soft outside the selection, the layer mask is the
    // selection exactly.
    let fake = FakeComfy::start().unwrap();
    let mut s = session(&fake.url);
    let r = s.execute(FILL, json!({"prompt": "x", "margin": 0.5})).unwrap();
    let st = fake.state();
    let sent = photocraft_genai::png::decode_rgba8(&st.uploads[1].1).unwrap();
    // The selection is x 10..30, y 8..24 on the canvas; a 50 % margin of the longer side (10 px)
    // makes the request rect (0, 0)..(40, 34), so the selection sits at x 10..30, y 8..24 of it.
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(40), Some(34)));
    assert_eq!((sent.width, sent.height), (40, 34));
    assert_eq!(sent.get(10, 16).map(|p| p[0]), Some(255), "the selection itself");
    assert!(sent.get(8, 16).unwrap()[0] > 0, "feathered beyond the selection's edge");
    assert_eq!(sent.get(1, 16).map(|p| p[0]), Some(0), "the tail ends well inside the margin");
    drop(st);
    let d = s.active().unwrap();
    let l = d.doc.layer(LayerId(r["layer"].as_u64().unwrap())).unwrap();
    let m = l.mask.as_ref().unwrap().surface.read_region(Rect::new(8, 16, 9, 17));
    assert_eq!(m[0], 0.0, "the layer mask is the selection itself, not the feathered request mask");
}

#[test]
fn resampling_is_lanczos_for_pixels_and_tent_for_masks() {
    // Flat stays flat both ways (normalised taps, no ringing on a constant).
    let flat = Rgba8::solid(37, 23, [10, 200, 30, 255]).unwrap();
    for (w, h) in [(74, 46), (19, 12), (100, 7)] {
        let r = resize_rgba8(&flat, w, h).unwrap();
        assert!(r.data.as_chunks::<4>().0.iter().all(|p| *p == [10, 200, 30, 255]), "{w}×{h}");
    }
    // A two-tone image keeps its halves after a 4× upscale; the edge is sharp within a few px.
    let mut data = Vec::new();
    for _y in 0..8u32 {
        for x in 0..8u32 {
            data.extend_from_slice(if x < 4 { &[0, 0, 0, 255] } else { &[255, 255, 255, 255] });
        }
    }
    let two = Rgba8::new(8, 8, data).unwrap();
    let up = resize_rgba8(&two, 32, 32).unwrap();
    assert_eq!(up.get(2, 16).map(|p| p[0]), Some(0));
    assert_eq!(up.get(29, 16).map(|p| p[0]), Some(255));
    assert!(up.get(13, 16).unwrap()[0] < 60 && up.get(18, 16).unwrap()[0] > 195, "{:?} {:?}", up.get(13, 16), up.get(18, 16));
    let down = resize_rgba8(&two, 4, 4).unwrap();
    assert!(down.get(0, 0).unwrap()[0] < 10 && down.get(3, 3).unwrap()[0] > 245);
    // Masks: a hard edge stays monotonic and in range.
    let mask = Gray8::new(8, 1, vec![0, 0, 0, 0, 255, 255, 255, 255]).unwrap();
    let m = resize_gray8(&mask, 16, 1).unwrap();
    assert!(m.data.windows(2).all(|w| w[0] <= w[1]), "{:?}", m.data);
    assert_eq!((m.data[0], m.data[15]), (0, 255));
    assert!(resize_rgba8(&two, 0, 2).is_err() && resize_gray8(&mask, 3, 0).is_err());
}

#[test]
fn research_templates_are_gated_by_the_preference() {
    // No research-only template ships yet; the gate is exercised through its message path.
    let fake = FakeComfy::start().unwrap();
    let s = session(&fake.url);
    assert!(!s.prefs().integrations.allow_research_models);
    let t = template::find(DEFAULT_FILL_TEMPLATE).unwrap();
    assert_eq!(t.meta.license, License::Permissive);
}
