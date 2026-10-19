//! The app-managed ComfyUI process ("the sidecar", plan-native-engine §5.4): PhotoshopEX starts
//! the user's ComfyUI install itself, hidden (no console window), logs it to a file, knows when
//! it is ready, stops it on quit, and reports its state for the status bar. The server is still
//! the same separate program talking HTTP on loopback; only its lifecycle moved into the app.
//!
//! One child per app process (a process-wide state, like `generate_cmds::LAST_MODELS`); *what*
//! to launch is a [`Launch`] the caller builds from the preferences on each call, so nothing
//! here caches a stale folder or URL. The `generate.engine.*` commands drive it; the ComfyUI
//! backend calls [`ensure_running`] before a run so a generation waits for the server instead
//! of failing with "cannot reach the server".
//!
//! Cross-platform by construction (§3.5): the interpreter is found per layout (the Windows
//! portable build's `python_embeded`, a `venv`, the system `python3`), the child gets the
//! platform's no-window flag on Windows and its own process group on Unix, and paths are
//! `PathBuf`s throughout. Nothing here is reachable on wasm.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::{Error, Progress, Result};

/// How the sidecar is launched: the install folder, the URL the preferences name (the host and
/// port come from it), the extra server flags, and where its output goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// A ComfyUI install: the Windows portable build's folder (holding `python_embeded/` and
    /// `ComfyUI/`), or a checkout (holding `main.py`, with a `venv`/`.venv` or a system Python).
    pub root: PathBuf,
    /// `http://127.0.0.1:8188`: the server binds this host and port.
    pub url: String,
    /// Extra flags after `--listen` and `--port` (`--fast fp8_matrix_mult`).
    pub args: Vec<String>,
    /// The server's stdout and stderr, appended; started again when it grows past [`LOG_LIMIT`].
    pub log: PathBuf,
}

/// Where the sidecar is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    /// Configured, not running (or stopped by us).
    Stopped,
    /// Spawned, not answering `/system_stats` yet (the first start loads Python and the nodes,
    /// 10–40 s on this machine).
    Starting,
    /// Our child answers.
    Ready,
    /// Something else serves the URL (a ComfyUI started by hand); we leave it alone.
    External,
    /// The child exited or could not be spawned; `detail` says why, with the log's last line.
    Failed,
}

/// `generate.engine.status`'s answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: State,
    pub pid: Option<u32>,
    pub url: String,
    pub root: String,
    pub log: String,
    /// A sentence for the UI (the failure reason, the interpreter in use).
    pub detail: String,
    /// Milliseconds since the child was spawned.
    pub since_ms: Option<u64>,
}

/// Bytes of server output kept before the log file is started again.
pub const LOG_LIMIT: u64 = 10 * 1024 * 1024;
/// How long a start may take before `ensure_running` gives up (a cold start that imports the
/// custom nodes can take a minute on a slow disk).
pub const START_TIMEOUT: Duration = Duration::from_secs(180);
/// A readiness probe of the same URL is repeated at most this often by `status()`.
const PROBE_EVERY: Duration = Duration::from_millis(1000);
/// How much of the log a failure message carries.
const TAIL_BYTES: u64 = 2048;

struct Inner {
    child: Option<Child>,
    started: Option<Instant>,
    failed: Option<String>,
    /// The interpreter the child runs with (status detail).
    python: Option<PathBuf>,
    /// The last probe: the URL asked, when, and whether it answered.
    probed: Option<(String, Instant, bool)>,
}

static SIDECAR: Mutex<Inner> = Mutex::new(Inner { child: None, started: None, failed: None, python: None, probed: None });

fn lock() -> MutexGuard<'static, Inner> {
    SIDECAR.lock().unwrap_or_else(|e| e.into_inner())
}

/// The host and port of `url` (`http://127.0.0.1:8188` → `("127.0.0.1", 8188)`); the default
/// port when none is given.
pub fn host_port(url: &str) -> (String, u16) {
    let rest = url.trim().trim_end_matches('/');
    let rest = rest.split_once("://").map_or(rest, |(_, r)| r);
    let rest = rest.split('/').next().unwrap_or(rest);
    match rest.rsplit_once(':') {
        Some((h, p)) if !h.contains(']') || h.ends_with(']') => {
            let host = h.trim_matches(|c| c == '[' || c == ']');
            (if host.is_empty() { "127.0.0.1".into() } else { host.to_string() }, p.parse().unwrap_or(8188))
        }
        _ => (if rest.is_empty() { "127.0.0.1".into() } else { rest.to_string() }, 8188),
    }
}

/// The interpreter, the script and the working directory for a ComfyUI install at `root`, by
/// layout: the Windows portable build (`python_embeded\python.exe` + `ComfyUI\main.py`), a
/// checkout with a virtual environment (`venv` or `.venv`), or a checkout run with the system
/// Python. A folder with no `main.py` in either place is not an install.
pub fn resolve(root: &Path) -> std::result::Result<(PathBuf, PathBuf, PathBuf), String> {
    let (main, cwd) = if root.join("ComfyUI").join("main.py").is_file() {
        (root.join("ComfyUI").join("main.py"), root.to_path_buf())
    } else if root.join("main.py").is_file() {
        (root.join("main.py"), root.to_path_buf())
    } else {
        return Err(format!("{} is not a ComfyUI install (no main.py, no ComfyUI/main.py)", root.display()));
    };
    let candidates: Vec<PathBuf> = if cfg!(windows) {
        vec![
            root.join("python_embeded").join("python.exe"),
            root.join("venv").join("Scripts").join("python.exe"),
            root.join(".venv").join("Scripts").join("python.exe"),
        ]
    } else {
        vec![root.join("venv").join("bin").join("python"), root.join(".venv").join("bin").join("python")]
    };
    let python = candidates.into_iter().find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "python" } else { "python3" }));
    Ok((python, main, cwd))
}

/// The command line a launch runs: `python [-s] main.py --listen host --port port
/// --disable-auto-launch <args>`. `-s` keeps the portable interpreter off the user's site
/// packages, as its own start script does.
pub fn command_line(launch: &Launch) -> std::result::Result<(PathBuf, Vec<String>, PathBuf), String> {
    let (python, main, cwd) = resolve(&launch.root)?;
    let (host, port) = host_port(&launch.url);
    let mut args = Vec::new();
    if python.starts_with(launch.root.join("python_embeded")) {
        args.push("-s".to_string());
    }
    args.push(main.to_string_lossy().into_owned());
    args.extend(["--listen".to_string(), host, "--port".to_string(), port.to_string(), "--disable-auto-launch".to_string()]);
    args.extend(launch.args.iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()));
    Ok((python, args, cwd))
}

/// Does a ComfyUI answer at `url` right now? One quick request with a short connect timeout
/// (a closed loopback port answers at once; a server that is still importing does not listen
/// yet and fails the same way).
pub fn reachable(url: &str) -> bool {
    crate::comfy::ComfyClient::probe(url, Duration::from_millis(1500))
}

/// The last `n` bytes of the log as whole lines (the failure sentence's evidence).
pub fn log_tail(path: &Path, n: u64) -> String {
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(n);
    if f.seek(SeekFrom::Start(from)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buf);
    // A cut that landed mid-line drops that partial first line.
    let text = if from > 0 { text.split_once('\n').map_or("", |(_, rest)| rest) } else { text.as_ref() };
    text.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

fn open_log(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(path).map(|m| m.len() > LOG_LIMIT).unwrap_or(false) {
        let _ = std::fs::remove_file(path);
    }
    std::fs::OpenOptions::new().create(true).append(true).open(path)
}

fn spawn(launch: &Launch) -> std::result::Result<(Child, PathBuf), String> {
    let (python, args, cwd) = command_line(launch)?;
    let log = open_log(&launch.log).map_err(|e| format!("cannot write the engine log {}: {e}", launch.log.display()))?;
    let err = log.try_clone().map_err(|e| format!("cannot write the engine log {}: {e}", launch.log.display()))?;
    let mut cmd = Command::new(&python);
    cmd.args(&args).current_dir(&cwd).stdin(Stdio::null()).stdout(Stdio::from(log)).stderr(Stdio::from(err));
    // Unbuffered Python output reaches the log as it happens.
    cmd.env("PYTHONUNBUFFERED", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    let child = cmd.spawn().map_err(|e| format!("cannot start {} ({e}); is Python installed in {}?", python.display(), launch.root.display()))?;
    Ok((child, python))
}

/// Start the sidecar. Already running → its status; the URL already served by something else →
/// `External`, nothing spawned; otherwise spawn and return `Starting` at once (readiness comes
/// through [`status`] and [`ensure_running`]).
pub fn start(launch: &Launch) -> std::result::Result<Status, String> {
    let mut g = lock();
    if let Some(child) = g.child.as_mut()
        && matches!(child.try_wait(), Ok(None))
    {
        drop(g);
        return Ok(status(launch));
    }
    g.child = None;
    g.failed = None;
    if reachable(&launch.url) {
        g.probed = Some((launch.url.clone(), Instant::now(), true));
        drop(g);
        return Ok(status(launch));
    }
    match spawn(launch) {
        Ok((child, python)) => {
            g.child = Some(child);
            g.python = Some(python);
            g.started = Some(Instant::now());
            g.probed = None;
        }
        Err(e) => {
            g.failed = Some(e.clone());
            drop(g);
            return Err(e);
        }
    }
    drop(g);
    Ok(status(launch))
}

/// Stop our child (an external server is left alone). Returns the status afterwards.
pub fn stop(launch: &Launch) -> Status {
    stop_if_owned();
    status(launch)
}

/// Stop the child if we own one (the app is quitting and the preference says so, or `stop`).
pub fn stop_if_owned() {
    let mut g = lock();
    if let Some(mut child) = g.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    g.started = None;
    g.failed = None;
    g.probed = None;
}

/// Where the sidecar is now: polls the child (an exit becomes `Failed` with the log's last
/// line) and probes the URL at most once a second.
pub fn status(launch: &Launch) -> Status {
    let mut g = lock();
    let base = |state: State, g: &Inner, detail: String| Status {
        state,
        pid: g.child.as_ref().map(Child::id),
        url: launch.url.clone(),
        root: launch.root.to_string_lossy().into_owned(),
        log: launch.log.to_string_lossy().into_owned(),
        detail,
        since_ms: g.started.map(|t| t.elapsed().as_millis() as u64),
    };
    // A child that exited is a failure (ComfyUI does not stop by itself).
    if let Some(child) = g.child.as_mut() {
        match child.try_wait() {
            Ok(Some(code)) => {
                let last = log_tail(&launch.log, TAIL_BYTES).lines().last().unwrap_or("").to_string();
                g.child = None;
                g.started = None;
                g.failed = Some(if last.is_empty() { format!("the engine exited ({code})") } else { format!("the engine exited ({code}): {last}") });
            }
            Ok(None) => {}
            Err(e) => {
                g.child = None;
                g.started = None;
                g.failed = Some(format!("lost the engine process: {e}"));
            }
        }
    }
    if let Some(f) = g.failed.clone() {
        return base(State::Failed, &g, f);
    }
    let fresh = g.probed.as_ref().filter(|(u, at, _)| *u == launch.url && at.elapsed() < PROBE_EVERY).map(|(_, _, ok)| *ok);
    let ok = fresh.unwrap_or_else(|| {
        let ok = reachable(&launch.url);
        g.probed = Some((launch.url.clone(), Instant::now(), ok));
        ok
    });
    if g.child.is_some() {
        let py = g.python.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        if ok {
            return base(State::Ready, &g, py);
        }
        let waited = g.started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        return base(State::Starting, &g, if waited > START_TIMEOUT.as_secs() { format!("still not answering after {waited} s; see the log") } else { py });
    }
    if ok { base(State::External, &g, "a ComfyUI started outside PhotoshopEX serves this URL".into()) } else { base(State::Stopped, &g, String::new()) }
}

/// Make the server reachable before a generation: nothing to do when it answers; otherwise start
/// the sidecar and wait for it, reporting progress and honouring cancellation. Fails with a
/// sentence (and the log's tail) when it does not come up in [`START_TIMEOUT`].
pub fn ensure_running(launch: &Launch, progress: &dyn Progress) -> Result<()> {
    if reachable(&launch.url) {
        return Ok(());
    }
    progress.report(0.0, "Starting the generative engine");
    let st = start(launch).map_err(Error::Unavailable)?;
    if matches!(st.state, State::Ready | State::External) {
        return Ok(());
    }
    let t0 = Instant::now();
    while t0.elapsed() < START_TIMEOUT {
        if progress.cancelled() {
            return Err(Error::Cancelled);
        }
        std::thread::sleep(Duration::from_millis(500));
        let st = status(launch);
        match st.state {
            State::Ready | State::External => return Ok(()),
            State::Failed => return Err(Error::Unavailable(format!("{}\n{}", st.detail, log_tail(&launch.log, TAIL_BYTES)))),
            _ => progress.report(0.0, &format!("Starting the generative engine ({} s)", t0.elapsed().as_secs())),
        }
    }
    Err(Error::Unavailable(format!("the generative engine did not answer within {} s; its log is {}", START_TIMEOUT.as_secs(), launch.log.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pex-sidecar-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn host_and_port_come_from_the_url() {
        assert_eq!(host_port("http://127.0.0.1:8188"), ("127.0.0.1".into(), 8188));
        assert_eq!(host_port("http://localhost:9000/"), ("localhost".into(), 9000));
        assert_eq!(host_port("127.0.0.1"), ("127.0.0.1".into(), 8188));
        assert_eq!(host_port("http://[::1]:8200/api"), ("::1".into(), 8200));
        assert_eq!(host_port(""), ("127.0.0.1".into(), 8188));
    }

    #[test]
    fn layouts_resolve_to_an_interpreter_and_a_script() {
        // The Windows portable build (on Unix the same folder with a venv).
        let d = temp("portable");
        std::fs::create_dir_all(d.join("ComfyUI")).unwrap();
        std::fs::write(d.join("ComfyUI").join("main.py"), "").unwrap();
        let py = if cfg!(windows) { d.join("python_embeded").join("python.exe") } else { d.join("venv").join("bin").join("python") };
        std::fs::create_dir_all(py.parent().unwrap()).unwrap();
        std::fs::write(&py, "").unwrap();
        let (python, main, cwd) = resolve(&d).unwrap();
        assert_eq!((python, main, cwd), (py.clone(), d.join("ComfyUI").join("main.py"), d.clone()));
        let launch = Launch {
            root: d.clone(),
            url: "http://127.0.0.1:8199".into(),
            args: vec!["--fast".into(), "fp8_matrix_mult".into(), " ".into()],
            log: d.join("log.txt"),
        };
        let (_, args, _) = command_line(&launch).unwrap();
        assert_eq!(args.first().map(String::as_str) == Some("-s"), cfg!(windows), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["--listen", "127.0.0.1"]) && args.windows(2).any(|w| w == ["--port", "8199"]), "{args:?}");
        assert!(args.contains(&"--disable-auto-launch".to_string()) && args.ends_with(&["--fast".to_string(), "fp8_matrix_mult".to_string()]), "{args:?}");
        // A checkout without a venv falls back to the system interpreter.
        let c = temp("checkout");
        std::fs::write(c.join("main.py"), "").unwrap();
        let (python, main, _) = resolve(&c).unwrap();
        assert_eq!(main, c.join("main.py"));
        assert_eq!(python, PathBuf::from(if cfg!(windows) { "python" } else { "python3" }));
        // Not an install.
        let e = temp("empty");
        assert!(resolve(&e).unwrap_err().contains("not a ComfyUI install"));
        for d in [d, c, e] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn the_log_tail_is_whole_lines() {
        let d = temp("log");
        let p = d.join("engine.log");
        std::fs::write(&p, "first line\nsecond line\nthird line\n").unwrap();
        assert_eq!(log_tail(&p, 18), "third line");
        assert_eq!(log_tail(&p, 1000), "first line\nsecond line\nthird line");
        assert_eq!(log_tail(&d.join("missing.log"), 10), "");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn an_external_server_is_left_alone_and_a_closed_port_is_stopped() {
        let fake = crate::fake::FakeComfy::start().unwrap();
        let d = temp("external");
        std::fs::write(d.join("main.py"), "").unwrap();
        let launch = Launch { root: d.clone(), url: fake.url.clone(), args: Vec::new(), log: d.join("engine.log") };
        let st = start(&launch).unwrap();
        assert_eq!((st.state, st.pid), (State::External, None));
        assert!(ensure_running(&launch, &crate::NoProgress).is_ok());
        assert_eq!(stop(&launch).state, State::External, "stop leaves an external server alone");
        let closed = Launch { url: "http://127.0.0.1:9".into(), ..launch.clone() };
        assert_eq!(status(&closed).state, State::Stopped);
        // A folder that is not an install cannot start, with a sentence.
        let bad = Launch { root: d.join("nothing-here"), ..closed };
        assert!(start(&bad).unwrap_err().contains("not a ComfyUI install"));
        let _ = std::fs::remove_dir_all(d);
    }
}
