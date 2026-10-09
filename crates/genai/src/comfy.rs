//! The ComfyUI backend: a thin client for the server's HTTP routes and WebSocket, and the
//! [`GenerativeBackend`] that drives a [`Template`] through them.
//!
//! The flow for one run (see `docs/comfy/comfyui-setup.md` › API primer):
//! `GET /system_stats` (reachable, version) → `POST /upload/image` for the picture and the mask →
//! open `/ws?clientId=…` → `POST /prompt` with the filled graph → progress from the socket while
//! polling `GET /history/{id}` → `GET /view` for every output → decode. Cancellation calls
//! `POST /interrupt` and removes the queued prompt. The socket is optional: when it cannot be
//! opened or drops, polling alone finishes the run.
//!
//! Blocking I/O on purpose: the engine runs generations on job worker threads (`jobs::edit_job`),
//! and blocking calls with short read timeouts let [`Progress::cancelled`] be checked every few
//! hundred milliseconds. Native only; the web build has no client yet.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::template::{self, Template};
use crate::{Error, GenerativeBackend, Health, Progress, Request, Response, Result, Rgba8, Timings, png};

/// Largest response body the client accepts (a 2K RGBA PNG is a few MB; this leaves room).
const MAX_BODY: u64 = 256 * 1024 * 1024;
/// How often the run loop checks for cancellation and polls the history.
const TICK: Duration = Duration::from_millis(200);
const POLL: Duration = Duration::from_millis(500);

/// A ComfyUI server at one base URL.
#[derive(Clone)]
pub struct ComfyClient {
    base: String,
    agent: ureq::Agent,
}

impl std::fmt::Debug for ComfyClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComfyClient").field("base", &self.base).finish()
    }
}

/// The server's base URL, checked: `http://` or `https://`, no spaces, no trailing slash.
pub fn normalize_url(url: &str) -> Result<String> {
    let u = url.trim().trim_end_matches('/');
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err(Error::Request(format!("the ComfyUI server URL must start with http:// or https:// (got `{url}`)")));
    }
    if host_of(u).is_empty() || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(Error::Request(format!("`{url}` is not a usable server URL")));
    }
    Ok(u.to_string())
}

/// The host part of an `http(s)://` URL (without port, brackets stripped from IPv6).
fn host_of(url: &str) -> &str {
    let rest = url.trim().trim_start_matches("http://").trim_start_matches("https://");
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    }
}

/// Is this URL on the local machine? (The default policy only talks to loopback servers.)
pub fn is_loopback(url: &str) -> bool {
    let host = host_of(url);
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

fn transport(what: &str, e: ureq::Error) -> Error {
    Error::Protocol(format!("{what}: {e}"))
}

/// A short, readable version of the server's error JSON (`{"error": {"message", "details"}}`).
fn server_error(status: u16, body: &str) -> Error {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        let e = &v["error"];
        let message = e["message"].as_str().or_else(|| e.as_str()).unwrap_or("");
        let details = e["details"].as_str().unwrap_or("");
        let nodes: Vec<String> = v["node_errors"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(id, ne)| {
                        let class = ne["class_type"].as_str().unwrap_or("?");
                        let msgs: Vec<&str> = ne["errors"].as_array().map(|a| a.iter().filter_map(|x| x["message"].as_str()).collect()).unwrap_or_default();
                        format!("node {id} ({class}): {}", msgs.join("; "))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut s = format!("HTTP {status}");
        if !message.is_empty() {
            s.push_str(": ");
            s.push_str(message);
        }
        if !details.is_empty() {
            s.push_str(" (");
            s.push_str(details);
            s.push(')');
        }
        if !nodes.is_empty() {
            s.push_str("; ");
            s.push_str(&nodes.join("; "));
        }
        return Error::Server(s);
    }
    let snippet: String = body.chars().take(200).collect();
    Error::Server(format!("HTTP {status}: {snippet}"))
}

impl ComfyClient {
    /// A client for the server at `base_url`. `timeout` bounds every single HTTP exchange (not a
    /// whole generation; that is the backend's deadline).
    pub fn new(base_url: &str, timeout: Duration) -> Result<Self> {
        let base = normalize_url(base_url)?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(3)))
            .timeout_recv_response(Some(timeout))
            .timeout_recv_body(Some(timeout))
            .http_status_as_error(false)
            .build()
            .into();
        Ok(Self { base, agent })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn read_body(resp: &mut ureq::http::Response<ureq::Body>, what: &str) -> Result<Vec<u8>> {
        resp.body_mut().with_config().limit(MAX_BODY).read_to_vec().map_err(|e| transport(what, e))
    }

    fn finish(mut resp: ureq::http::Response<ureq::Body>, what: &str) -> Result<Vec<u8>> {
        let status = resp.status().as_u16();
        let body = Self::read_body(&mut resp, what)?;
        if status >= 400 {
            return Err(server_error(status, &String::from_utf8_lossy(&body)));
        }
        Ok(body)
    }

    fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let resp = self.agent.get(&self.url(path)).call().map_err(|e| transport(&format!("GET {path}"), e))?;
        Self::finish(resp, path)
    }

    fn parse_json(bytes: &[u8], what: &str) -> Result<Value> {
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        serde_json::from_slice(bytes).map_err(|e| {
            let snippet: String = String::from_utf8_lossy(bytes).chars().take(200).collect();
            Error::Protocol(format!("{what}: not JSON ({e}): {snippet}"))
        })
    }

    pub fn get_json(&self, path: &str) -> Result<Value> {
        Self::parse_json(&self.get_bytes(path)?, path)
    }

    pub fn post_json(&self, path: &str, body: &Value) -> Result<Value> {
        let bytes = serde_json::to_vec(body).map_err(|e| Error::Request(format!("cannot serialise the request: {e}")))?;
        let resp =
            self.agent.post(&self.url(path)).header("Content-Type", "application/json").send(&bytes[..]).map_err(|e| transport(&format!("POST {path}"), e))?;
        Self::parse_json(&Self::finish(resp, path)?, path)
    }

    pub fn post_empty(&self, path: &str) -> Result<()> {
        let resp = self.agent.post(&self.url(path)).send_empty().map_err(|e| transport(&format!("POST {path}"), e))?;
        Self::finish(resp, path).map(|_| ())
    }

    /// `GET /system_stats`: version, devices and VRAM. The first call of every run; an
    /// unreachable server fails here with [`Error::Unavailable`].
    pub fn system_stats(&self) -> Result<Value> {
        let resp = self.agent.get(&self.url("/system_stats")).call().map_err(|e| {
            Error::Unavailable(format!(
                "cannot reach the ComfyUI server at {} ({e}); start ComfyUI or change the server URL in Preferences › AI Integrations",
                self.base
            ))
        })?;
        let bytes = Self::finish(resp, "/system_stats")?;
        let v = Self::parse_json(&bytes, "/system_stats")?;
        if !v.is_object() {
            return Err(Error::Protocol(format!("{} does not look like a ComfyUI server (/system_stats is not an object)", self.base)));
        }
        Ok(v)
    }

    /// The ComfyUI version string from `/system_stats` (`""` when absent).
    pub fn version(stats: &Value) -> String {
        stats["system"]["comfyui_version"].as_str().unwrap_or("").to_string()
    }

    /// `GET /object_info`: every node class the server knows (large).
    pub fn object_info(&self) -> Result<Value> {
        self.get_json("/object_info")
    }

    /// `GET /models/{folder}`: file names in one model folder.
    pub fn models(&self, folder: &str) -> Result<Vec<String>> {
        if folder.is_empty() || !folder.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(Error::Request(format!("`{folder}` is not a model folder name")));
        }
        let v = self.get_json(&format!("/models/{folder}"))?;
        Ok(v.as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default())
    }

    /// `GET /prompt`: `exec_info.queue_remaining`.
    pub fn queue_remaining(&self) -> Result<u64> {
        let v = self.get_json("/prompt")?;
        Ok(v["exec_info"]["queue_remaining"].as_u64().unwrap_or(0))
    }

    /// `POST /upload/image` with `png` as `name` into the server's `input` folder (overwriting).
    /// Returns the name a `LoadImage` node needs (`subfolder/name` when the server moved it).
    pub fn upload_image(&self, name: &str, png: &[u8]) -> Result<String> {
        if name.is_empty() || name.contains(['"', '\r', '\n', '/', '\\']) {
            return Err(Error::Request(format!("`{name}` is not a safe upload name")));
        }
        let boundary = format!("----photocraft{}", crate::random_id());
        let mut body = Vec::with_capacity(png.len() + 512);
        let mut field = |n: &str, v: &str| {
            body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{n}\"\r\n\r\n{v}\r\n").as_bytes());
        };
        field("overwrite", "true");
        field("type", "input");
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"{name}\"\r\nContent-Type: image/png\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(png);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let resp = self
            .agent
            .post(&self.url("/upload/image"))
            .header("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
            .send(&body[..])
            .map_err(|e| transport("POST /upload/image", e))?;
        let v = Self::parse_json(&Self::finish(resp, "/upload/image")?, "/upload/image")?;
        let stored = v["name"].as_str().filter(|s| !s.is_empty()).unwrap_or(name);
        Ok(match v["subfolder"].as_str().filter(|s| !s.is_empty()) {
            Some(sub) => format!("{sub}/{stored}"),
            None => stored.to_string(),
        })
    }

    /// `POST /prompt`: queue an API-format graph; returns the `prompt_id`.
    pub fn queue_prompt(&self, graph: &Value, client_id: &str) -> Result<String> {
        let v = self.post_json("/prompt", &json!({"prompt": graph, "client_id": client_id}))?;
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            return Err(server_error(200, &json!({"error": e, "node_errors": v["node_errors"]}).to_string()));
        }
        v["prompt_id"].as_str().map(str::to_string).ok_or_else(|| Error::Protocol(format!("/prompt answered without a prompt_id: {v}")))
    }

    /// `GET /history/{id}`: the entry for one prompt, or `None` while it has not finished.
    pub fn history(&self, prompt_id: &str) -> Result<Option<Value>> {
        let v = self.get_json(&format!("/history/{prompt_id}"))?;
        Ok(v.get(prompt_id).filter(|e| e.is_object()).cloned())
    }

    /// `GET /view`: the bytes of an output file.
    pub fn view(&self, filename: &str, subfolder: &str, kind: &str) -> Result<Vec<u8>> {
        let q = format!("/view?filename={}&subfolder={}&type={}", percent(filename), percent(subfolder), percent(kind));
        self.get_bytes(&q)
    }

    /// `POST /interrupt`: stop the running prompt.
    pub fn interrupt(&self) -> Result<()> {
        self.post_empty("/interrupt")
    }

    /// `POST /queue {"delete": [id]}`: drop a prompt that has not started.
    pub fn delete_queued(&self, prompt_id: &str) -> Result<()> {
        self.post_json("/queue", &json!({"delete": [prompt_id]})).map(|_| ())
    }

    /// `POST /free`: unload models and free VRAM.
    pub fn free(&self) -> Result<()> {
        self.post_json("/free", &json!({"unload_models": true, "free_memory": true})).map(|_| ())
    }

    /// Open the progress socket for `client_id` (`ws://` or `wss://` from the base URL). `None`
    /// when the server has no socket; the run then relies on polling.
    pub fn open_socket(&self, client_id: &str) -> Option<WebSocket<MaybeTlsStream<TcpStream>>> {
        let ws_base = if let Some(rest) = self.base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else {
            format!("ws://{}", self.base.trim_start_matches("http://"))
        };
        let (mut socket, _) = tungstenite::connect(format!("{ws_base}/ws?clientId={client_id}")).ok()?;
        if let MaybeTlsStream::Plain(s) = socket.get_mut() {
            let _ = s.set_read_timeout(Some(TICK));
        }
        Some(socket)
    }
}

fn percent(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The `images` entries of a history entry's outputs, the template's save node first.
fn output_images(entry: &Value, save_node: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let Some(outputs) = entry["outputs"].as_object() else { return out };
    let mut nodes: Vec<(&String, &Value)> = outputs.iter().collect();
    nodes.sort_by_key(|(id, _)| if *id == save_node { 0 } else { 1 });
    for (_, node) in nodes {
        for img in node["images"].as_array().into_iter().flatten() {
            if let Some(name) = img["filename"].as_str() {
                out.push((name.to_string(), img["subfolder"].as_str().unwrap_or("").to_string(), img["type"].as_str().unwrap_or("output").to_string()));
            }
        }
    }
    out
}

/// A failure recorded in a history entry's `status.messages` (`execution_error`), if any.
fn history_error(entry: &Value) -> Option<String> {
    let status = &entry["status"];
    let failed = status["status_str"].as_str() == Some("error");
    let msg = status["messages"].as_array().and_then(|ms| {
        ms.iter().find_map(|m| {
            let kind = m.get(0)?.as_str()?;
            (kind == "execution_error").then(|| {
                let d = &m[1];
                let node = d["node_type"].as_str().unwrap_or("");
                let text = d["exception_message"].as_str().unwrap_or("");
                if node.is_empty() { text.to_string() } else { format!("{node}: {text}") }
            })
        })
    });
    match (failed, msg) {
        (_, Some(m)) if !m.is_empty() => Some(m),
        (true, _) => Some("the workflow failed on the server".to_string()),
        _ => None,
    }
}

/// A [`GenerativeBackend`] over one ComfyUI server.
#[derive(Clone, Debug)]
pub struct ComfyBackend {
    client: ComfyClient,
    /// Bound on a whole generation, from queueing to the last download.
    deadline: Duration,
}

impl ComfyBackend {
    /// `url` of the server; `deadline` bounds a whole run (preference `generativeTimeoutSecs`).
    pub fn new(url: &str, deadline: Duration) -> Result<Self> {
        let per_request = deadline.min(Duration::from_secs(120)).max(Duration::from_secs(2));
        Ok(Self { client: ComfyClient::new(url, per_request)?, deadline })
    }

    pub fn client(&self) -> &ComfyClient {
        &self.client
    }

    fn cancel(&self, prompt_id: &str) {
        let _ = self.client.interrupt();
        let _ = self.client.delete_queued(prompt_id);
    }

    /// Wait for `prompt_id`: progress from the socket, completion from the history. Also returns
    /// when the server was first seen working on it (`None` when no socket message said so).
    fn wait(
        &self,
        prompt_id: &str,
        mut socket: Option<WebSocket<MaybeTlsStream<TcpStream>>>,
        progress: &dyn Progress,
        started: Instant,
    ) -> Result<(Value, Option<Instant>)> {
        let mut last_poll: Option<Instant> = None;
        let mut poll_now = true;
        let mut first_activity: Option<Instant> = None;
        loop {
            if progress.cancelled() {
                self.cancel(prompt_id);
                return Err(Error::Cancelled);
            }
            if started.elapsed() > self.deadline {
                self.cancel(prompt_id);
                return Err(Error::Timeout(self.deadline.as_secs()));
            }
            match socket.as_mut() {
                Some(ws) => match ws.read() {
                    Ok(Message::Text(t)) => {
                        if let Ok(m) = serde_json::from_str::<Value>(t.as_str()) {
                            let data = &m["data"];
                            let mine = data["prompt_id"].as_str().is_none_or(|p| p == prompt_id);
                            let kind = m["type"].as_str().unwrap_or("");
                            if mine && first_activity.is_none() && matches!(kind, "execution_start" | "executing" | "progress" | "executed") {
                                first_activity = Some(Instant::now());
                            }
                            match kind {
                                "progress" if mine => {
                                    let (v, max) = (data["value"].as_f64().unwrap_or(0.0), data["max"].as_f64().unwrap_or(1.0).max(1.0));
                                    progress.report(0.05 + 0.85 * (v / max).clamp(0.0, 1.0) as f32, &format!("Sampling {}/{}", v as u64, max as u64));
                                }
                                "execution_error" if mine => {
                                    let node = data["node_type"].as_str().unwrap_or("");
                                    let text = data["exception_message"].as_str().unwrap_or("the workflow failed");
                                    return Err(Error::Server(if node.is_empty() { text.to_string() } else { format!("{node}: {text}") }));
                                }
                                "execution_success" | "executed" if mine => poll_now = true,
                                "executing" if mine && data["node"].is_null() => poll_now = true,
                                "executing" if mine => progress.report(0.05, "Running"),
                                _ => {}
                            }
                        }
                    }
                    Ok(Message::Close(_)) => socket = None,
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                    Err(_) => socket = None,
                },
                None => std::thread::sleep(TICK),
            }
            if poll_now || last_poll.is_none_or(|t| t.elapsed() >= POLL) {
                poll_now = false;
                last_poll = Some(Instant::now());
                if let Some(entry) = self.client.history(prompt_id)? {
                    if let Some(e) = history_error(&entry) {
                        return Err(Error::Server(e));
                    }
                    let done = entry["status"]["completed"].as_bool() == Some(true) || !entry["outputs"].as_object().is_none_or(|o| o.is_empty());
                    if done {
                        return Ok((entry, first_activity));
                    }
                }
            }
        }
    }
}

/// Upload names come from the bytes (FNV-1a 64), so the same picture always lands on the same
/// server file: ComfyUI keys its node cache on file content, and the loader, the text encoder
/// and the VAE encode are then reused across variations and re-rolls of one selection (several
/// seconds each with a 7 B vision encoder). The encoder writes no timestamps, so equal pixels
/// give equal bytes.
fn upload_name(kind: &str, bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("photocraft-{h:016x}-{kind}.png")
}

impl GenerativeBackend for ComfyBackend {
    fn health(&self) -> Health {
        let mut h = Health { backend: "comfyui".into(), url: self.client.base.clone(), ..Health::default() };
        match self.client.system_stats() {
            Ok(stats) => {
                h.ok = true;
                h.version = ComfyClient::version(&stats);
                if let Some(dev) = stats["devices"].as_array().and_then(|d| d.first()) {
                    h.vram_total = dev["vram_total"].as_u64();
                    h.vram_free = dev["vram_free"].as_u64();
                }
                h.queue_remaining = self.client.queue_remaining().ok();
            }
            Err(e) => h.error = Some(e.to_string()),
        }
        h
    }

    fn run(&self, req: &Request, progress: &dyn Progress) -> Result<Response> {
        let started = Instant::now();
        let tpl: Template = template::find(&req.template)?;
        if tpl.meta.needs_image && req.image.is_none() {
            return Err(Error::Request(format!("template `{}` needs an image", tpl.meta.id)));
        }
        if tpl.meta.needs_mask && req.mask.is_none() {
            return Err(Error::Request(format!("template `{}` needs a mask", tpl.meta.id)));
        }
        if let (Some(i), Some(m)) = (&req.image, &req.mask)
            && (i.width, i.height) != (m.width, m.height)
        {
            return Err(Error::Request("the mask must have the image's size".into()));
        }
        if req.prompt.trim().is_empty() {
            return Err(Error::Request("the prompt is empty".into()));
        }
        progress.report(0.0, "Connecting");
        let stats = self.client.system_stats()?;
        let version = ComfyClient::version(&stats);
        if !tpl.meta.min_comfy_version.is_empty() && !version.is_empty() && !template::version_at_least(&version, &tpl.meta.min_comfy_version) {
            return Err(Error::Unavailable(format!(
                "ComfyUI {version} is older than {} needed by `{}`; update ComfyUI",
                tpl.meta.min_comfy_version, tpl.meta.id
            )));
        }
        progress.check_cancel()?;

        let client_id = crate::random_id();
        // Template-specific values first; the standard bindings below win on a clash.
        let mut bindings: BTreeMap<String, Value> = req.params.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        bindings.extend(tpl.model_bindings(&req.models)?);
        bindings.insert("prompt".into(), Value::String(tpl.format_prompt(&req.prompt)));
        bindings.insert("negative".into(), Value::String(req.negative.clone()));
        bindings.insert("seed".into(), Value::from(req.seed & ((1u64 << 53) - 1)));
        bindings.insert("steps".into(), Value::from(if req.steps == 0 { tpl.meta.defaults.steps } else { req.steps }));
        let cfg = if req.guidance > 0.0 { req.guidance } else { tpl.meta.defaults.guidance };
        bindings.insert("cfg".into(), serde_json::Number::from_f64(f64::from(cfg)).map(Value::Number).unwrap_or(Value::from(1)));
        bindings.insert("prefix".into(), Value::String(format!("photocraft/{client_id}")));

        let mut timings = Timings::default();
        if let Some(img) = &req.image {
            progress.report(0.01, "Uploading");
            let t = Instant::now();
            let bytes = png::encode_rgba8(img)?;
            timings.encode_ms += t.elapsed().as_millis() as u64;
            let t = Instant::now();
            let name = self.client.upload_image(&upload_name("image", &bytes), &bytes)?;
            timings.upload_ms += t.elapsed().as_millis() as u64;
            bindings.insert("image".into(), Value::String(name));
        }
        if let Some(mask) = &req.mask {
            let t = Instant::now();
            let bytes = png::encode_gray8(mask)?;
            timings.encode_ms += t.elapsed().as_millis() as u64;
            let t = Instant::now();
            let name = self.client.upload_image(&upload_name("mask", &bytes), &bytes)?;
            timings.upload_ms += t.elapsed().as_millis() as u64;
            bindings.insert("mask".into(), Value::String(name));
        }
        let wants_size = tpl.placeholders().iter().any(|p| *p == "width" || *p == "height");
        match req.size {
            Some((w, h)) => {
                if w == 0 || h == 0 || w > 16_384 || h > 16_384 {
                    return Err(Error::Request(format!("{w}×{h} is not a usable output size")));
                }
                bindings.insert("width".into(), Value::from(w));
                bindings.insert("height".into(), Value::from(h));
            }
            None if wants_size => return Err(Error::Request(format!("template `{}` needs an output size", tpl.meta.id))),
            None => {}
        }
        progress.check_cancel()?;
        let graph = tpl.fill(&bindings)?;

        // The socket opens before the prompt is queued so no progress message is missed.
        let socket = self.client.open_socket(&client_id);
        let prompt_id = self.client.queue_prompt(&graph, &client_id)?;
        progress.report(0.03, "Queued");
        let queued = Instant::now();
        let (entry, first_activity) = self.wait(&prompt_id, socket, progress, started)?;
        let finished = Instant::now();
        let began = first_activity.unwrap_or(queued);
        timings.queue_ms = began.duration_since(queued).as_millis() as u64;
        timings.run_ms = finished.duration_since(began).as_millis() as u64;

        progress.report(0.92, "Downloading");
        let t = Instant::now();
        let mut images = Vec::new();
        for (name, sub, kind) in output_images(&entry, &tpl.meta.save_node) {
            progress.check_cancel()?;
            let bytes = self.client.view(&name, &sub, &kind)?;
            images.push(png::decode_rgba8(&bytes)?);
        }
        timings.download_ms = t.elapsed().as_millis() as u64;
        if images.is_empty() {
            return Err(Error::NoOutput(format!("prompt {prompt_id} finished without an image")));
        }
        progress.report(1.0, "Done");
        Ok(Response { images, seed: req.seed, run_id: prompt_id, elapsed_ms: started.elapsed().as_millis() as u64, timings })
    }

    fn model_files(&self, folder: &str) -> Result<Vec<String>> {
        self.client.models(folder)
    }
}

trait CheckCancel {
    fn check_cancel(&self) -> Result<()>;
}

impl CheckCancel for &dyn Progress {
    fn check_cancel(&self) -> Result<()> {
        if self.cancelled() { Err(Error::Cancelled) } else { Ok(()) }
    }
}

/// Keep [`Rgba8`] in this module's public surface for callers that only import `comfy`.
pub type Image = Rgba8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_normalised_and_checked() {
        assert_eq!(normalize_url(" http://127.0.0.1:8188/ ").unwrap_or_default(), "http://127.0.0.1:8188");
        assert!(normalize_url("127.0.0.1:8188").is_err());
        assert!(normalize_url("http://a b").is_err());
        assert!(is_loopback("http://127.0.0.1:8188"));
        assert!(is_loopback("http://localhost:8188/"));
        assert!(is_loopback("http://[::1]:8188"));
        assert!(!is_loopback("http://192.168.1.5:8188"));
        assert_eq!(percent("a b/c.png"), "a%20b/c.png");
    }

    #[test]
    fn server_errors_are_readable() {
        let e = server_error(
            400,
            r#"{"error":{"type":"invalid_prompt","message":"Cannot execute because node X does not exist.","details":"Node ID '#4'"},"node_errors":{}}"#,
        );
        assert_eq!(e.to_string(), "the generative server reported an error: HTTP 400: Cannot execute because node X does not exist. (Node ID '#4')");
        let e = server_error(400, r#"{"error":"bad","node_errors":{"14":{"class_type":"KSampler","errors":[{"message":"Value not in list"}]}}}"#);
        assert!(e.to_string().contains("node 14 (KSampler): Value not in list"));
        assert!(server_error(500, "<html>oops").to_string().contains("HTTP 500"));
    }

    #[test]
    fn history_entries_are_read() {
        let entry = json!({"outputs": {"9": {"images": [{"filename": "a.png", "subfolder": "", "type": "output"}]}, "16": {"images": [{"filename": "b.png", "subfolder": "photocraft", "type": "output"}]}}, "status": {"status_str": "success", "completed": true, "messages": []}});
        let imgs = output_images(&entry, "16");
        assert_eq!(imgs[0].0, "b.png", "the save node comes first");
        assert_eq!(imgs.len(), 2);
        assert!(history_error(&entry).is_none());
        let failed = json!({"outputs": {}, "status": {"status_str": "error", "completed": false, "messages": [["execution_start", {}], ["execution_error", {"node_type": "UNETLoader", "exception_message": "file not found"}]]}});
        assert_eq!(history_error(&failed).as_deref(), Some("UNETLoader: file not found"));
    }
}
