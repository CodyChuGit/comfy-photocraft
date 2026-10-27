//! Ask a running native worker's chat model one question per picture, for tuning the engine's
//! rewriter wording without running whole jobs (bench tooling, `docs/comfy/bench/scripts/
//! naming-probe.ps1`). The worker is joined through `PHOTOCRAFT_NATIVE_ENGINE=port:token`; with
//! `PHOTOCRAFT_COMFY_URL` set instead, ComfyUI's rewriter (the 4B) answers.
//!
//! ```text
//! cargo run --release -p photocraft-genai --example chat_probe -- <system.txt> <prompt> <seed> <picture.png>...
//! ```
//!
//! Prints one line per picture: its file name, the milliseconds the answer took, the answer.

use std::collections::BTreeMap;
use std::time::Duration;

use photocraft_genai::comfy::ComfyBackend;
use photocraft_genai::native::NativeBackend;
use photocraft_genai::{GenerativeBackend, NoProgress, Request, Rgba8};

fn picture(path: &str) -> Result<Rgba8, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let img = photocraft_codecs::decode(&bytes).map_err(|e| format!("{path}: {e}"))?;
    let rgba = img.convert(photocraft_codecs::ChannelLayout::Rgba, photocraft_codecs::SampleType::U8);
    Rgba8::new(rgba.width(), rgba.height(), rgba.data().to_vec()).map_err(|e| format!("{path}: {e}"))
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [system, prompt, seed, pictures @ ..] = args.as_slice() else {
        return Err("usage: chat_probe <system.txt> <prompt> <seed> <picture.png>...".into());
    };
    let system = std::fs::read_to_string(system).map_err(|e| format!("{system}: {e}"))?;
    let seed: u64 = seed.parse().map_err(|e| format!("seed: {e}"))?;
    let backend: Box<dyn GenerativeBackend> = match std::env::var("PHOTOCRAFT_COMFY_URL") {
        Ok(url) if !url.trim().is_empty() => Box::new(ComfyBackend::new(url.trim(), Duration::from_secs(600)).map_err(|e| e.to_string())?),
        _ => {
            let spec = std::env::var("PHOTOCRAFT_NATIVE_ENGINE").map_err(|_| "set PHOTOCRAFT_NATIVE_ENGINE=port:token or PHOTOCRAFT_COMFY_URL".to_string())?;
            let (port, token) = spec.split_once(':').ok_or("PHOTOCRAFT_NATIVE_ENGINE is not port:token")?;
            let port: u16 = port.trim().parse().map_err(|e| format!("port: {e}"))?;
            Box::new(NativeBackend::attached(port, token.trim()).map_err(|e| e.to_string())?)
        }
    };
    for path in pictures {
        let mut params = BTreeMap::new();
        params.insert("system".to_string(), serde_json::Value::String(system.clone()));
        let req = Request {
            seed,
            steps: 0,
            image: Some(picture(path)?),
            params,
            ..Request::new("enhance/edit", prompt)
        };
        let started = std::time::Instant::now();
        let answer = match backend.run(&req, &NoProgress) {
            Ok(r) => r.texts.join(" | ").replace('\n', " "),
            Err(e) => format!("ERROR {e}"),
        };
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        println!("{name}\t{}\t{answer}", started.elapsed().as_millis());
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(2);
    }
}
