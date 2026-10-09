//! The ComfyUI client and backend against the in-process fake server (`--features fake-server`).
#![cfg(all(feature = "fake-server", not(target_arch = "wasm32")))]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use photocraft_genai::comfy::ComfyBackend;
use photocraft_genai::fake::{FakeComfy, Options};
use photocraft_genai::{Error, GenerativeBackend, Gray8, NoProgress, Progress, Request, Rgba8};

/// Records every progress report; `cancel_at` flips cancellation once that fraction is reached.
struct Recorder {
    fractions: Mutex<Vec<f32>>,
    messages: Mutex<Vec<String>>,
    cancelled: AtomicBool,
    cancel_at: Option<f32>,
}

impl Recorder {
    fn new(cancel_at: Option<f32>) -> Self {
        Self { fractions: Mutex::new(Vec::new()), messages: Mutex::new(Vec::new()), cancelled: AtomicBool::new(false), cancel_at }
    }
}

impl Progress for Recorder {
    fn report(&self, fraction: f32, message: &str) {
        self.fractions.lock().unwrap_or_else(|e| e.into_inner()).push(fraction);
        self.messages.lock().unwrap_or_else(|e| e.into_inner()).push(message.to_string());
        if self.cancel_at.is_some_and(|c| fraction >= c) {
            self.cancelled.store(true, Ordering::Relaxed);
        }
    }
    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

fn request() -> Request {
    let mut r = Request::new("qwen-edit-2511/fill", "a red bicycle leaning on the wall");
    r.image = Some(Rgba8::solid(40, 24, [10, 20, 30, 255]).unwrap_or_else(|e| panic!("{e}")));
    r.mask = Some(Gray8::new(40, 24, vec![255; 40 * 24]).unwrap_or_else(|e| panic!("{e}")));
    r.seed = 1234;
    r
}

fn backend(url: &str) -> ComfyBackend {
    ComfyBackend::new(url, Duration::from_secs(20)).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn a_run_uploads_queues_waits_and_downloads() {
    // A short server delay so socket progress arrives before the history reports completion.
    let fake = FakeComfy::start_with(Options { delay_ms: 400, ..Options::default() }).unwrap_or_else(|e| panic!("{e}"));
    let b = backend(&fake.url);
    let rec = Recorder::new(None);
    let resp = b.run(&request(), &rec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(resp.images.len(), 1);
    let img = &resp.images[0];
    assert_eq!((img.width, img.height), (40, 24), "the result has the input's size");
    assert_eq!(img.get(3, 3), Some([200, 40, 40, 255]));
    assert_eq!(resp.seed, 1234);
    assert_eq!(resp.run_id, "fake-prompt-1");

    let st = fake.state();
    assert_eq!(st.uploads.len(), 2, "image and mask");
    assert!(st.uploads[0].0.ends_with("-image.png") && st.uploads[1].0.ends_with("-mask.png"));
    assert_eq!(st.prompts.len(), 1);
    let (_, graph, client_id) = &st.prompts[0];
    assert_eq!(client_id.len(), 32);
    assert_eq!(graph["6"]["inputs"]["prompt"], "a red bicycle leaning on the wall");
    assert_eq!(graph["14"]["inputs"]["seed"], 1234);
    assert_eq!(graph["14"]["inputs"]["steps"], 40, "template default");
    assert_eq!(graph["14"]["inputs"]["cfg"], 4.0, "template default");
    assert_eq!(graph["4"]["inputs"]["image"], st.uploads[0].0);
    assert_eq!(graph["11"]["inputs"]["image"], st.uploads[1].0);
    assert!(graph["16"]["inputs"]["filename_prefix"].as_str().is_some_and(|p| p.starts_with("photocraft/")));
    assert!(st.requests.iter().any(|r| r.starts_with("GET /view?filename=fake-prompt-1.png")), "{:?}", st.requests);
    assert_eq!(st.interrupts, 0);

    let fr = rec.fractions.lock().unwrap_or_else(|e| e.into_inner());
    assert!(fr.windows(2).all(|w| w[1] >= w[0]), "progress never goes backwards: {fr:?}");
    assert_eq!(fr.last().copied(), Some(1.0));
    assert!(fr.iter().any(|f| (0.1..0.9).contains(f)), "socket progress arrived: {fr:?}");
}

#[test]
fn polling_alone_finishes_a_run_without_a_socket() {
    let fake = FakeComfy::start_with(Options { websocket: false, delay_ms: 300, ..Options::default() }).unwrap_or_else(|e| panic!("{e}"));
    let b = backend(&fake.url);
    let t = Instant::now();
    let resp = b.run(&request(), &NoProgress).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(resp.images.len(), 1);
    assert!(t.elapsed() >= Duration::from_millis(250), "waited for the server");
}

#[test]
fn overrides_steps_guidance_and_models() {
    let fake = FakeComfy::start().unwrap_or_else(|e| panic!("{e}"));
    let b = backend(&fake.url);
    let mut r = request();
    r.steps = 4;
    r.guidance = 1.0;
    r.negative = "blurry".into();
    r.models = vec![("unet".into(), "qwen_image_edit_2511_bf16.safetensors".into())];
    b.run(&r, &NoProgress).unwrap_or_else(|e| panic!("{e}"));
    let st = fake.state();
    let g = &st.prompts[0].1;
    assert_eq!(g["14"]["inputs"]["steps"], 4);
    assert_eq!(g["14"]["inputs"]["cfg"], 1.0);
    assert_eq!(g["7"]["inputs"]["prompt"], "blurry");
    assert_eq!(g["1"]["inputs"]["unet_name"], "qwen_image_edit_2511_bf16.safetensors");
}

#[test]
fn a_failing_workflow_is_a_server_error() {
    let fake = FakeComfy::start_with(Options { fail: Some("Error while deserializing header: file not found".into()), ..Options::default() })
        .unwrap_or_else(|e| panic!("{e}"));
    let b = backend(&fake.url);
    match b.run(&request(), &NoProgress) {
        Err(Error::Server(m)) => assert!(m.contains("file not found") && m.contains("KSampler"), "{m}"),
        other => panic!("expected a server error, got {other:?}"),
    }
}

#[test]
fn a_rejected_prompt_names_the_problem() {
    let fake = FakeComfy::start_with(Options {
        reject_prompt: Some("Cannot execute because node TextEncodeQwenImageEditPlus does not exist.".into()),
        ..Options::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let b = backend(&fake.url);
    match b.run(&request(), &NoProgress) {
        Err(Error::Server(m)) => assert!(m.contains("TextEncodeQwenImageEditPlus"), "{m}"),
        other => panic!("expected a server error, got {other:?}"),
    }
    assert!(fake.state().uploads.len() == 2, "the failure came after the uploads");
}

#[test]
fn cancellation_interrupts_the_server_and_returns_quickly() {
    let fake = FakeComfy::start_with(Options { delay_ms: 10_000, progress_steps: 2, ..Options::default() }).unwrap_or_else(|e| panic!("{e}"));
    let b = backend(&fake.url);
    let rec = Recorder::new(Some(0.03)); // cancel once queued
    let t = Instant::now();
    assert!(matches!(b.run(&request(), &rec), Err(Error::Cancelled)));
    assert!(t.elapsed() < Duration::from_secs(3), "cancel took {:?}", t.elapsed());
    let st = fake.state();
    assert_eq!(st.interrupts, 1);
    assert_eq!(st.deleted, vec!["fake-prompt-1".to_string()]);
}

#[test]
fn the_deadline_interrupts_a_stalled_run() {
    let fake = FakeComfy::start_with(Options { delay_ms: 60_000, websocket: false, ..Options::default() }).unwrap_or_else(|e| panic!("{e}"));
    let b = ComfyBackend::new(&fake.url, Duration::from_secs(2)).unwrap_or_else(|e| panic!("{e}"));
    let t = Instant::now();
    assert!(matches!(b.run(&request(), &NoProgress), Err(Error::Timeout(2))));
    assert!(t.elapsed() < Duration::from_secs(5));
    assert_eq!(fake.state().interrupts, 1);
}

#[test]
fn an_unreachable_server_is_unavailable_not_a_panic() {
    let b = ComfyBackend::new("http://127.0.0.1:1", Duration::from_secs(5)).unwrap_or_else(|e| panic!("{e}"));
    assert!(matches!(b.run(&request(), &NoProgress), Err(Error::Unavailable(_))));
    let h = b.health();
    assert!(!h.ok && h.error.is_some());
}

#[test]
fn an_old_server_is_refused_before_uploading() {
    let fake = FakeComfy::start_with(Options { version: "0.3.9".into(), ..Options::default() }).unwrap_or_else(|e| panic!("{e}"));
    let b = backend(&fake.url);
    match b.run(&request(), &NoProgress) {
        Err(Error::Unavailable(m)) => assert!(m.contains("0.3.9") && m.contains("0.4.0"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(fake.state().uploads.is_empty());
}

#[test]
fn health_and_model_files() {
    let fake = FakeComfy::start().unwrap_or_else(|e| panic!("{e}"));
    let b = backend(&fake.url);
    let h = b.health();
    assert!(h.ok, "{h:?}");
    assert_eq!(h.version, "0.37.0");
    assert_eq!(h.vram_total, Some(32_000_000_000));
    assert_eq!(h.queue_remaining, Some(0));
    let files = b.model_files("diffusion_models").unwrap_or_else(|e| panic!("{e}"));
    assert!(files.iter().any(|f| f == "qwen_image_edit_2511_fp8mixed.safetensors"), "{files:?}");
    assert!(b.model_files("../etc").is_err());
}

#[test]
fn requests_are_checked_before_any_network_call() {
    let b = ComfyBackend::new("http://127.0.0.1:1", Duration::from_secs(5)).unwrap_or_else(|e| panic!("{e}"));
    let mut r = request();
    r.mask = None;
    assert!(matches!(b.run(&r, &NoProgress), Err(Error::Request(_))));
    let mut r = request();
    r.prompt = "   ".into();
    assert!(matches!(b.run(&r, &NoProgress), Err(Error::Request(_))));
    let mut r = request();
    r.template = "nope/fill".into();
    assert!(matches!(b.run(&r, &NoProgress), Err(Error::Template(_))));
    let mut r = request();
    r.mask = Some(Gray8::new(2, 2, vec![0; 4]).unwrap_or_else(|e| panic!("{e}")));
    assert!(matches!(b.run(&r, &NoProgress), Err(Error::Request(_))));
}
