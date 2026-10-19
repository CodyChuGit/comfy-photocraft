//! The native engine as the editor sees it (plan-native-engine §3.4): a [`GenerativeBackend`]
//! that talks to a worker process over `gn-engine`'s channel. The worker is this very
//! executable started with `--engine` ([`engine_main`]), so the editor and its engine ship as
//! one program; a CUDA fault ends the worker, never a document, and the next call starts a
//! fresh one.
//!
//! Until the native models exist the worker serves `gn-engine`'s fake engine, which is enough to
//! route a request end to end (and to test the editor without a GPU). Pixels cross as binary
//! frames; everything else is the same `Request`/`Response` the ComfyUI backend speaks.

use std::path::PathBuf;
use std::sync::Mutex;

use gn_engine::client::{ClientError, EngineClient};
use gn_engine::protocol::{Blob, ErrorKind, RunRequest};
use serde_json::Value;

use crate::{Error, GenerativeBackend, Gray8, Health, Progress, Request, Response, Result, Rgba8, Task, Timings, template};

/// The engine worker, started on first use and restarted after it dies.
pub struct NativeBackend {
    program: PathBuf,
    args: Vec<String>,
    token: String,
    client: Mutex<Option<EngineClient>>,
    /// A worker we did not start (tests serve one in-process): never respawned.
    attached: bool,
}

impl std::fmt::Debug for NativeBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeBackend").field("program", &self.program).field("args", &self.args).finish()
    }
}

/// The argument that turns this executable into the engine worker.
pub const ENGINE_FLAG: &str = "--engine";

impl NativeBackend {
    /// A backend that starts `program args…` as its worker when first needed.
    pub fn new(program: PathBuf, args: Vec<String>) -> Self {
        Self { program, args, token: gn_engine::client::new_token(), client: Mutex::new(None), attached: false }
    }

    /// This executable with `--engine` (the editor and the CLI both answer it).
    pub fn this_executable() -> Result<Self> {
        let exe = std::env::current_exe().map_err(|e| Error::Unavailable(format!("cannot find this executable: {e}")))?;
        Ok(Self::new(exe, vec![ENGINE_FLAG.to_string()]))
    }

    /// A backend over a worker already listening (tests serve `gn_engine::fake::FakeEngine` on
    /// a thread).
    pub fn attached(port: u16, token: &str) -> Result<Self> {
        let client = EngineClient::connect(port, token).map_err(|e| Error::Unavailable(e.to_string()))?;
        Ok(Self { program: PathBuf::new(), args: Vec::new(), token: token.to_string(), client: Mutex::new(Some(client)), attached: true })
    }

    /// Run `f` on a live client, starting or restarting the worker first when needed. A call
    /// that finds the channel broken drops the client so the next one starts afresh.
    fn with_client<T>(&self, f: impl FnOnce(&mut EngineClient) -> std::result::Result<T, ClientError>) -> Result<T> {
        let mut guard = self.client.lock().unwrap_or_else(|e| e.into_inner());
        let dead = guard.as_mut().is_some_and(|c| !c.alive());
        if dead {
            *guard = None;
        }
        if guard.is_none() {
            if self.attached {
                return Err(Error::Unavailable("the engine is gone".into()));
            }
            let c = EngineClient::spawn(&self.program, &self.args, &self.token).map_err(|e| Error::Unavailable(e.to_string()))?;
            *guard = Some(c);
        }
        let Some(client) = guard.as_mut() else { return Err(Error::Unavailable("no engine".into())) };
        match f(client) {
            Ok(v) => Ok(v),
            Err(ClientError::Gone(m)) => {
                *guard = None;
                Err(Error::Unavailable(format!("the engine stopped: {m}")))
            }
            Err(e) => Err(map_err(e)),
        }
    }

    /// Stop the worker (the app is quitting, or Purge).
    pub fn stop(&self) {
        let mut guard = self.client.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = guard.take()
            && !self.attached
        {
            c.quit();
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.client.lock().ok().and_then(|g| g.as_ref().and_then(EngineClient::pid))
    }
}

fn map_err(e: ClientError) -> Error {
    match e {
        ClientError::Engine { kind: ErrorKind::Cancelled, .. } => Error::Cancelled,
        ClientError::Engine { kind: ErrorKind::BadRequest, message } => Error::Request(message),
        ClientError::Engine { message, .. } => Error::Server(message),
        ClientError::Gone(m) | ClientError::Start(m) => Error::Unavailable(m),
        ClientError::Protocol(m) => Error::Protocol(m),
    }
}

/// A task's wire name (`serde`'s lowercase spelling).
fn task_name(task: Task) -> String {
    serde_json::to_value(task).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "image".into())
}

/// The request's pixels as blobs (image, mask, references, in that order) and their bytes.
fn blobs_of(req: &Request) -> (Option<Blob>, Option<Blob>, Vec<Blob>, Vec<Vec<u8>>) {
    let mut bytes = Vec::new();
    let mut next = |w: u32, h: u32, c: u8, data: &[u8]| -> Blob {
        let b = Blob { width: w, height: h, channels: c, index: bytes.len() as u32 };
        bytes.push(data.to_vec());
        b
    };
    let image = req.image.as_ref().map(|i| next(i.width, i.height, 4, &i.data));
    let mask = req.mask.as_ref().map(|m| next(m.width, m.height, 1, &m.data));
    let refs = req.references.iter().map(|r| next(r.width, r.height, 4, &r.data)).collect();
    (image, mask, refs, bytes)
}

/// A returned blob as the editor's RGBA (coverage comes back as grey with full alpha, the way
/// the ComfyUI route delivers a detector's masks).
fn picture_of(b: &Blob, bytes: &[u8]) -> Result<Rgba8> {
    match b.channels {
        4 => Rgba8::new(b.width, b.height, bytes.to_vec()),
        1 => {
            let g = Gray8::new(b.width, b.height, bytes.to_vec())?;
            let mut data = Vec::with_capacity(g.data.len() * 4);
            for v in &g.data {
                data.extend_from_slice(&[*v, *v, *v, 255]);
            }
            Rgba8::new(b.width, b.height, data)
        }
        c => Err(Error::Protocol(format!("a picture with {c} channels"))),
    }
}

impl GenerativeBackend for NativeBackend {
    fn health(&self) -> Health {
        let url = format!("{} {}", self.program.display(), self.args.join(" "));
        match self.with_client(|c| c.health()) {
            Ok(h) => Health {
                ok: h.ok,
                backend: format!("native/{}", h.engine),
                url,
                version: String::new(),
                device: h.device,
                vram_total: h.vram_total,
                vram_free: h.vram_free,
                queue_remaining: Some(0),
                error: None,
            },
            Err(e) => Health { ok: false, backend: "native".into(), url, error: Some(e.to_string()), ..Health::default() },
        }
    }

    fn run(&self, req: &Request, progress: &dyn Progress) -> Result<Response> {
        let tpl = template::find(&req.template)?;
        let (image, mask, references, bytes) = blobs_of(req);
        let (width, height) = req.size.unwrap_or((0, 0));
        let rr = RunRequest {
            model: req.template.clone(),
            task: task_name(tpl.meta.task),
            prompt: req.prompt.clone(),
            negative: req.negative.clone(),
            seed: req.seed,
            steps: req.steps,
            guidance: req.guidance,
            width,
            height,
            image,
            mask,
            references,
            params: req.params.clone(),
        };
        let started = std::time::Instant::now();
        let (info, pictures) = self.with_client(|c| {
            let mut report = |f: f32, m: &str| progress.report(f, m);
            c.run(&rr, &bytes, &mut report, &|| progress.cancelled())
        })?;
        let mut images = Vec::with_capacity(info.images.len());
        for (i, b) in info.images.iter().enumerate() {
            let data = pictures.get(i).ok_or_else(|| Error::Protocol(format!("picture {i} missing")))?;
            images.push(picture_of(b, data)?);
        }
        Ok(Response {
            images,
            texts: info.texts,
            seed: info.seed,
            run_id: format!("native-{}", info.seed),
            elapsed_ms: started.elapsed().as_millis() as u64,
            timings: Timings { run_ms: info.elapsed_ms, ..Timings::default() },
        })
    }

    fn model_files(&self, folder: &str) -> Result<Vec<String>> {
        // The engine's index: a list under `installed`, or per-folder lists under `folders`.
        let v = self.with_client(|c| c.index(""))?;
        let list = |v: &Value| v.as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()).unwrap_or_default();
        if let Some(f) = v.get("folders").and_then(|f| f.get(folder)) {
            return Ok(list(f));
        }
        Ok(v.get("installed").map(list).unwrap_or_default())
    }

    fn free(&self) -> Result<()> {
        self.with_client(|c| c.free())
    }
}

/// `photocraft --engine [--token T] [--step-ms N]`: this process becomes the worker. Reads the
/// token from `GN_ENGINE_TOKEN` when not given, binds a loopback port, prints the port line,
/// serves until the editor says quit. Returns the exit code. Serves the fake engine until the
/// native models land.
pub fn engine_main(args: &[String]) -> i32 {
    let mut token = std::env::var("GN_ENGINE_TOKEN").unwrap_or_default();
    let mut step_ms = 5u64;
    let mut i = 0;
    while i < args.len() {
        match args.get(i).map(String::as_str) {
            Some("--token") => {
                token = args.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            Some("--step-ms") => {
                step_ms = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(5);
                i += 2;
            }
            _ => i += 1,
        }
    }
    if token.is_empty() {
        eprintln!("--engine: no token (GN_ENGINE_TOKEN or --token)");
        return 2;
    }
    let listener = match gn_engine::server::bind() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("--engine: cannot bind: {e}");
            return 1;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    println!("{}", gn_engine::server::port_line(port));
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let mut engine = gn_engine::fake::FakeEngine { step_ms, installed: template::builtin().iter().map(|t| t.meta.id.clone()).collect(), ..Default::default() };
    match gn_engine::server::serve(&listener, &token, &mut engine) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("--engine: {e}");
            1
        }
    }
}

/// Tests (this crate's and the engine's): a fake engine served on a thread in this process, as
/// the worker would be, knowing `installed` templates; its port and token.
#[cfg(feature = "fake-server")]
pub fn serve_fake(installed: Vec<String>, step_ms: u64) -> Result<(u16, String)> {
    let listener = gn_engine::server::bind().map_err(|e| Error::Unavailable(e.to_string()))?;
    let port = listener.local_addr().map_err(|e| Error::Unavailable(e.to_string()))?.port();
    let token = gn_engine::client::new_token();
    let t = token.clone();
    std::thread::spawn(move || {
        let mut engine = gn_engine::fake::FakeEngine { step_ms, installed, ..Default::default() };
        let _ = gn_engine::server::serve(&listener, &t, &mut engine);
    });
    Ok((port, token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn served() -> (u16, String) {
        serve_fake(vec!["krea2-turbo/image".into(), "qwen-edit-2511/edit".into(), "sam3.1/segment".into()], 1).unwrap()
    }

    struct Cancel(AtomicBool, Mutex<Vec<(f32, String)>>);
    impl Progress for Cancel {
        fn report(&self, fraction: f32, message: &str) {
            self.1.lock().unwrap().push((fraction, message.to_string()));
        }
        fn cancelled(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn requests_cross_the_channel_and_come_back_as_pictures() {
        let (port, token) = served();
        let b = NativeBackend::attached(port, &token).unwrap();
        let h = b.health();
        assert!(h.ok && h.backend == "native/fake", "{h:?}");
        // Text to image: the fake paints a solid picture of the asked size.
        let mut req = Request::new("krea2-turbo/image", "a fox");
        req.size = Some((48, 32));
        req.steps = 4;
        let p = Cancel(AtomicBool::new(false), Mutex::new(Vec::new()));
        let r = b.run(&req, &p).unwrap();
        assert_eq!((r.images.len(), r.images[0].width, r.images[0].height), (1, 48, 32));
        assert!(p.1.lock().unwrap().len() >= 4, "progress reports arrived");
        // An edit sends the picture across and gets the same size back.
        let mut edit = Request::new("qwen-edit-2511/edit", "make it winter");
        edit.image = Some(Rgba8::solid(20, 10, [10, 20, 30, 255]).unwrap());
        let r = b.run(&edit, &crate::NoProgress).unwrap();
        assert_eq!((r.images[0].width, r.images[0].height), (20, 10));
        assert_ne!(r.images[0].data, edit.image.as_ref().unwrap().data);
        // A detector's coverage comes back as a grey picture.
        let mut seg = Request::new("sam3.1/segment", "fox");
        seg.image = Some(Rgba8::solid(40, 40, [0, 0, 0, 255]).unwrap());
        let r = b.run(&seg, &crate::NoProgress).unwrap();
        let centre = r.images[0].get(20, 20).unwrap();
        let corner = r.images[0].get(0, 0).unwrap();
        assert_eq!((centre, corner), ([255, 255, 255, 255], [0, 0, 0, 255]));
        // A template the engine lacks is a server error, not a dead backend.
        let missing = Request::new("qwen-2.1/image", "x");
        assert!(matches!(b.run(&missing, &crate::NoProgress), Err(Error::Server(_))));
        assert!(b.health().ok);
        // Cancel reaches the run.
        let mut long = Request::new("krea2-turbo/image", "slow");
        long.size = Some((8, 8));
        long.steps = 1000;
        let c = Arc::new(Cancel(AtomicBool::new(false), Mutex::new(Vec::new())));
        let c2 = Arc::clone(&c);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            c2.0.store(true, Ordering::SeqCst);
        });
        assert!(matches!(b.run(&long, &*c), Err(Error::Cancelled)));
        assert_eq!(b.model_files("diffusion_models").unwrap().len(), 3);
        b.free().unwrap();
    }
}
