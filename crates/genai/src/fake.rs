//! A fake ComfyUI server for tests: the routes and WebSocket messages the client uses, on a
//! loopback port, answering in milliseconds. It records what it received so tests can assert on
//! uploads, graphs and interrupts, and it can be told to fail, stall or lack a node.
//!
//! "Generation" is a solid colour the size of the uploaded image, so geometry can be checked.
//! Feature `fake-server`, native only.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{Rgba8, png, template};

/// How the fake behaves.
#[derive(Clone, Debug)]
pub struct Options {
    /// Time a prompt stays "running" before its history reports completion.
    pub delay_ms: u64,
    /// Make every prompt fail with this message (reported in the history and on the socket).
    pub fail: Option<String>,
    /// Reject `POST /prompt` with this message (HTTP 400, like a validation failure).
    pub reject_prompt: Option<String>,
    /// The colour of every generated pixel.
    pub color: [u8; 4],
    /// `progress` messages sent over the socket per prompt.
    pub progress_steps: u32,
    pub version: String,
    /// Leave this class out of `/object_info`.
    pub missing_node: Option<String>,
    /// Serve the WebSocket (false: connections to `/ws` are refused, so the client must poll).
    pub websocket: bool,
    /// What a detector graph (`SAM3_Detect`) "finds": one mask image per rectangle, given as
    /// normalised `[x0, y0, x1, y1]` of the uploaded image. Empty = nothing found.
    pub segments: Vec<[f32; 4]>,
    /// Model files left out of `/models/<folder>` (by default every built-in template's files
    /// are "installed"); e.g. a Lightning LoRA, to exercise the `auto` fallback.
    pub missing_files: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            delay_ms: 0,
            fail: None,
            reject_prompt: None,
            color: [200, 40, 40, 255],
            progress_steps: 4,
            version: "0.39.0".into(),
            missing_node: None,
            websocket: true,
            segments: vec![[0.25, 0.25, 0.75, 0.75]],
            missing_files: Vec::new(),
        }
    }
}

/// Everything the fake received.
#[derive(Debug, Default)]
pub struct State {
    /// `(file name, PNG bytes)` in upload order.
    pub uploads: Vec<(String, Vec<u8>)>,
    /// `(prompt_id, graph, client_id)` in queue order.
    pub prompts: Vec<(String, Value, String)>,
    /// `METHOD /path` of every request.
    pub requests: Vec<String>,
    pub interrupts: u32,
    pub deleted: Vec<String>,
    queued_at: BTreeMap<String, Instant>,
    outputs: BTreeMap<String, Vec<u8>>,
    /// Output file names per prompt, in order.
    output_files: BTreeMap<String, Vec<String>>,
}

pub struct FakeComfy {
    /// `http://127.0.0.1:<port>`
    pub url: String,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeComfy {
    pub fn start() -> std::io::Result<Self> {
        Self::start_with(Options::default())
    }

    pub fn start_with(opts: Options) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (st, sp, opts) = (Arc::clone(&state), Arc::clone(&stop), Arc::new(opts));
        let thread = std::thread::Builder::new().name("fake-comfy".into()).spawn(move || {
            while !sp.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (st, sp, opts) = (Arc::clone(&st), Arc::clone(&sp), Arc::clone(&opts));
                        let _ = std::thread::Builder::new().name("fake-comfy-conn".into()).spawn(move || handle(stream, &opts, &st, &sp));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(5)),
                    Err(_) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        })?;
        Ok(Self { url, state, stop, thread: Some(thread) })
    }

    /// A snapshot of what the server has seen.
    pub fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for FakeComfy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn lock(state: &Arc<Mutex<State>>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn handle(mut stream: TcpStream, opts: &Options, state: &Arc<Mutex<State>>, stop: &Arc<AtomicBool>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut peek = [0u8; 16];
    let n = stream.peek(&mut peek).unwrap_or(0);
    if peek[..n].starts_with(b"GET /ws") {
        if opts.websocket {
            websocket(stream, opts, state, stop);
        }
        return;
    }
    let Some((method, path, body, content_type)) = read_request(&mut stream) else { return };
    lock(state).requests.push(format!("{method} {path}"));
    let (status, ctype, out) = route(&method, &path, &body, &content_type, opts, state);
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&out);
    let _ = stream.flush();
}

/// `(method, path, body, content-type)` of one HTTP/1.1 request.
fn read_request(stream: &mut TcpStream) -> Option<(String, String, Vec<u8>, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > 1 << 20 {
            return None;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
    let mut len = 0usize;
    let mut ctype = String::new();
    for l in lines {
        let Some((k, v)) = l.split_once(':') else { continue };
        match k.trim().to_ascii_lowercase().as_str() {
            "content-length" => len = v.trim().parse().unwrap_or(0),
            "content-type" => ctype = v.trim().to_string(),
            _ => {}
        }
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    Some((method, path, body, ctype))
}

fn query(path: &str, key: &str) -> String {
    path.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| unpercent(v))
        .unwrap_or_default()
}

fn unpercent(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        if c == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(v) = u8::from_str_radix(hex, 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `image` part of a multipart body: `(filename, bytes)`.
fn multipart_image(body: &[u8], content_type: &str) -> Option<(String, Vec<u8>)> {
    let boundary = content_type.split("boundary=").nth(1)?.trim().trim_matches('"');
    let marker = format!("--{boundary}");
    let mut parts = Vec::new();
    let mut start = 0;
    while let Some(i) = find(&body[start..], marker.as_bytes()) {
        let s = start + i + marker.len();
        parts.push(s);
        start = s;
        if body[s..].starts_with(b"--") {
            break;
        }
    }
    for (a, b) in parts.iter().zip(parts.iter().skip(1)) {
        let part = &body[*a..*b - marker.len()];
        let hdr_end = find(part, b"\r\n\r\n")?;
        let hdr = String::from_utf8_lossy(&part[..hdr_end]).to_string();
        if !hdr.contains("name=\"image\"") {
            continue;
        }
        let filename = hdr.split("filename=\"").nth(1).and_then(|s| s.split('"').next()).unwrap_or("upload.png").to_string();
        let mut data = part[hdr_end + 4..].to_vec();
        if data.ends_with(b"\r\n") {
            data.truncate(data.len() - 2);
        }
        return Some((filename, data));
    }
    None
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn route(method: &str, path: &str, body: &[u8], ctype: &str, opts: &Options, state: &Arc<Mutex<State>>) -> (u16, &'static str, Vec<u8>) {
    let json = |status: u16, v: Value| (status, "application/json", v.to_string().into_bytes());
    let bare = path.split('?').next().unwrap_or(path);
    match (method, bare) {
        ("GET", "/system_stats") => json(
            200,
            json!({"system": {"os": "fake", "python_version": "3.12.0", "comfyui_version": opts.version, "ram_total": 64_000_000_000u64, "ram_free": 32_000_000_000u64},
                   "devices": [{"name": "Fake GPU", "type": "cuda", "index": 0, "vram_total": 32_000_000_000u64, "vram_free": 30_000_000_000u64}]}),
        ),
        ("GET", "/prompt") => json(200, json!({"exec_info": {"queue_remaining": 0}})),
        ("GET", "/object_info") => {
            let mut m = serde_json::Map::new();
            for t in template::builtin() {
                for c in t.class_types() {
                    if opts.missing_node.as_deref() != Some(c.as_str()) {
                        m.insert(c, json!({"input": {"required": {}}, "output": []}));
                    }
                }
            }
            json(200, Value::Object(m))
        }
        ("GET", p) if p.starts_with("/models/") => {
            let folder = &p["/models/".len()..];
            let mut files: Vec<String> = template::builtin()
                .iter()
                .flat_map(|t| t.meta.models.iter().filter(|s| s.folder == folder).map(|s| s.default.clone()).collect::<Vec<_>>())
                .filter(|f| !opts.missing_files.contains(f))
                .collect();
            files.sort();
            files.dedup();
            json(200, json!(files))
        }
        ("POST", "/upload/image") | ("POST", "/upload/mask") => match multipart_image(body, ctype) {
            Some((name, data)) => {
                lock(state).uploads.push((name.clone(), data));
                json(200, json!({"name": name, "subfolder": "", "type": "input"}))
            }
            None => json(400, json!({"error": "no image part"})),
        },
        ("POST", "/prompt") => {
            if let Some(msg) = &opts.reject_prompt {
                return json(400, json!({"error": {"type": "invalid_prompt", "message": msg, "details": "", "extra_info": {}}, "node_errors": {}}));
            }
            let Ok(v) = serde_json::from_slice::<Value>(body) else { return json(400, json!({"error": "bad json"})) };
            let Some(graph) = v.get("prompt").filter(|g| g.is_object()) else {
                return json(400, json!({"error": {"message": "no prompt"}, "node_errors": {}}));
            };
            let client_id = v["client_id"].as_str().unwrap_or("").to_string();
            let mut st = lock(state);
            let n = st.prompts.len() + 1;
            let id = format!("fake-prompt-{n}");
            // The output: a solid colour the size of the uploaded image the graph loads.
            let graph_text = graph.to_string();
            // Text-to-image graphs have no upload: their EmptyLatentImage node gives the size.
            let latent_size = graph
                .as_object()
                .and_then(|nodes| nodes.values().find(|n| n["class_type"] == "EmptyLatentImage"))
                .and_then(|n| Some((n["inputs"]["width"].as_u64()? as u32, n["inputs"]["height"].as_u64()? as u32)))
                .filter(|(w, h)| (1..=8192).contains(w) && (1..=8192).contains(h));
            let size = st
                .uploads
                .iter()
                .filter(|(name, _)| graph_text.contains(name.as_str()))
                .find_map(|(_, bytes)| png::decode_rgba8(bytes).ok())
                .map(|i| (i.width, i.height))
                .or(latent_size)
                .unwrap_or((64, 64));
            let files: Vec<(String, Vec<u8>)> = if graph_text.contains("SAM3_Detect") {
                // A detector: one mask image per configured segment (none = nothing found).
                let (w, h) = size;
                opts.segments
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let px = |f: f32, n: u32| ((n as f32 * f.clamp(0.0, 1.0)).round() as u32).min(n);
                        let (x0, y0, x1, y1) = (px(r[0], w), px(r[1], h), px(r[2], w), px(r[3], h));
                        let mut data = Vec::with_capacity(w as usize * h as usize * 4);
                        for y in 0..h {
                            for x in 0..w {
                                let v = if x >= x0 && x < x1 && y >= y0 && y < y1 { 255 } else { 0 };
                                data.extend_from_slice(&[v, v, v, 255]);
                            }
                        }
                        (format!("{id}_{i}.png"), Rgba8::new(w, h, data).and_then(|img| png::encode_rgba8(&img)).unwrap_or_default())
                    })
                    .collect()
            } else {
                vec![(format!("{id}.png"), Rgba8::solid(size.0, size.1, opts.color).and_then(|i| png::encode_rgba8(&i)).unwrap_or_default())]
            };
            let names: Vec<String> = files.iter().map(|(n, _)| n.clone()).collect();
            for (name, bytes) in files {
                st.outputs.insert(name, bytes);
            }
            st.output_files.insert(id.clone(), names);
            st.queued_at.insert(id.clone(), Instant::now());
            st.prompts.push((id.clone(), graph.clone(), client_id));
            json(200, json!({"prompt_id": id, "number": n, "node_errors": {}}))
        }
        ("GET", p) if p.starts_with("/history/") => {
            let id = &p["/history/".len()..];
            let st = lock(state);
            let Some(t) = st.queued_at.get(id) else { return json(200, json!({})) };
            if t.elapsed() < Duration::from_millis(opts.delay_ms) {
                return json(200, json!({}));
            }
            let entry = match &opts.fail {
                Some(msg) => {
                    json!({"prompt": [], "outputs": {}, "status": {"status_str": "error", "completed": false, "messages": [["execution_start", {"prompt_id": id}], ["execution_error", {"prompt_id": id, "node_id": "14", "node_type": "KSampler", "exception_message": msg}]]}})
                }
                None => {
                    let images: Vec<Value> =
                        st.output_files.get(id).into_iter().flatten().map(|n| json!({"filename": n, "subfolder": "photocraft", "type": "output"})).collect();
                    json!({"prompt": [], "outputs": {"16": {"images": images}}, "status": {"status_str": "success", "completed": true, "messages": []}})
                }
            };
            json(200, json!({id: entry}))
        }
        ("GET", "/view") => {
            let name = query(path, "filename");
            match lock(state).outputs.get(&name) {
                Some(bytes) => (200, "image/png", bytes.clone()),
                None => json(404, json!({"error": "no such file"})),
            }
        }
        ("POST", "/interrupt") => {
            lock(state).interrupts += 1;
            json(200, json!({}))
        }
        ("POST", "/queue") => {
            if let Ok(v) = serde_json::from_slice::<Value>(body) {
                let ids: Vec<String> = v["delete"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
                lock(state).deleted.extend(ids);
            }
            json(200, json!({}))
        }
        ("POST", "/free") => json(200, json!({})),
        _ => json(404, json!({"error": format!("no route {method} {bare}")})),
    }
}

/// Serve `/ws`: once a prompt is queued, send the message sequence a real server sends.
fn websocket(stream: TcpStream, opts: &Options, state: &Arc<Mutex<State>>, stop: &Arc<AtomicBool>) {
    let Ok(mut ws) = tungstenite::accept(stream) else { return };
    let send = |ws: &mut tungstenite::WebSocket<TcpStream>, v: Value| ws.send(tungstenite::Message::text(v.to_string())).is_ok();
    if !send(&mut ws, json!({"type": "status", "data": {"status": {"exec_info": {"queue_remaining": 0}}, "sid": "fake"}})) {
        return;
    }
    // Wait (up to 10 s) for a prompt to be queued.
    let t0 = Instant::now();
    let prompt_id = loop {
        if stop.load(Ordering::Relaxed) || t0.elapsed() > Duration::from_secs(10) {
            return;
        }
        if let Some((id, _, _)) = lock(state).prompts.last() {
            break id.clone();
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let steps = opts.progress_steps.max(1);
    let pause = Duration::from_millis(opts.delay_ms / u64::from(steps + 1));
    let _ = send(&mut ws, json!({"type": "execution_start", "data": {"prompt_id": prompt_id}}));
    let _ = send(&mut ws, json!({"type": "executing", "data": {"node": "14", "prompt_id": prompt_id}}));
    for i in 1..=steps {
        std::thread::sleep(pause);
        if stop.load(Ordering::Relaxed) || !send(&mut ws, json!({"type": "progress", "data": {"value": i, "max": steps, "prompt_id": prompt_id, "node": "14"}}))
        {
            return;
        }
    }
    std::thread::sleep(pause);
    if let Some(msg) = &opts.fail {
        let _ = send(
            &mut ws,
            json!({"type": "execution_error", "data": {"prompt_id": prompt_id, "node_id": "14", "node_type": "KSampler", "exception_message": msg}}),
        );
    } else {
        let _ = send(&mut ws, json!({"type": "executing", "data": {"node": null, "prompt_id": prompt_id}}));
        let _ = send(&mut ws, json!({"type": "execution_success", "data": {"prompt_id": prompt_id, "timestamp": 0}}));
    }
    // Stay open until the client goes away (answers pings), then exit.
    if let Ok(s) = ws.get_ref().try_clone() {
        let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
    }
    let t1 = Instant::now();
    while !stop.load(Ordering::Relaxed) && t1.elapsed() < Duration::from_secs(30) {
        match ws.read() {
            Ok(tungstenite::Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) | Err(tungstenite::Error::AlreadyClosed) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
    }
}
