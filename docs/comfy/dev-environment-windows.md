# Development environment (Windows)

What is on the development PC, how it got there, and the commands that work. Recorded
2026-10-08. Upstream's generic guide is [`docs/development.md`](../development.md); this page is
the Windows-specific layer on top of it.

## Machine

| | |
|---|---|
| OS | Windows 10 Home 10.0.19045 |
| GPU | NVIDIA GeForce RTX 5090, 32 607 MiB, driver 617.42 |
| Disk | C: 1.7 TB free; D: and E: are data drives |
| Checkout | `C:\Users\5090\Projects\comfy-photocraft` (clone of `https://github.com/storytold/photocraft.git`, branch `comfy-photocraft`) |

## Toolchain

| Tool | Version | How it was installed | Where |
|---|---|---|---|
| git | 2.56.0.windows.2 | scoop (pre-existing) | `C:\Users\5090\scoop\shims\git.exe` |
| rustup | 1.29.1 | `scoop install rustup` (2026-10-08) | `C:\Users\5090\scoop\apps\rustup\current` |
| Rust | stable 1.99.0 (b940084d7 2026-09-28), host `x86_64-pc-windows-msvc` | installed by rustup-init | `CARGO_HOME=C:\Users\5090\scoop\persist\rustup\.cargo`, `RUSTUP_HOME=C:\Users\5090\scoop\persist\rustup\.rustup` |
| MSVC | Visual Studio 2019 Build Tools with the VC x64/x86 tools and a Windows 10 SDK | pre-existing | `C:\Program Files (x86)\Microsoft Visual Studio\2019\BuildTools` (found via `vswhere -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64`) |
| MSVC 2022 | Visual Studio 2022 Build Tools 17.14.42, `Microsoft.VisualStudio.Workload.VCTools` (MSVC 14.44) | `winget install --id Microsoft.VisualStudio.2022.BuildTools --exact --override "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"` (2026-10-19; the installer needs the UAC prompt accepted, exit 1602 = it was not) | `C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools`; CUDA 13's host compiler |
| CUDA Toolkit | 13.4.3 (nvcc V13.4.92) | `scoop install cuda` (2026-10-19; user-level, no admin) | `C:\Users\5090\scoop\apps\cuda\current` (`bin` and `bin\x64` on the user PATH for new shells) |
| CMake | 4.4.4 | `scoop install cmake` (2026-10-18) | scoop shim |
| gh | 2.102.0 | `scoop install gh` (2026-10-08) | scoop shim; **not yet authenticated** (`gh auth login`) |
| Python | none (only the Microsoft Store stub on PATH) | — | needed only for ComfyUI, which brings its own |
| Node | none | — | not needed (the project is Rust only, by rule) |
| Pinokio | installed, no apps | pre-existing | `C:\Users\5090\AppData\Local\Programs\Pinokio` |
| Codex CLI / agy | 0.161.0 / 1.3.1 | pre-existing | agent delegates used by the orchestration skill (see below) |

Workspace requirement: `rust-version = "1.95"`, edition 2024 (`Cargo.toml`). 1.99 satisfies it.

### PATH

scoop appended `~\scoop\apps\rustup\current\.cargo\bin` to the *user* PATH and set `CARGO_HOME`
and `RUSTUP_HOME` as user environment variables. Shells opened **before** the install do not see
them; either open a new terminal or prepend for the session:

```powershell
$env:PATH = "C:\Users\5090\scoop\persist\rustup\.cargo\bin;$env:PATH"
```

### CUDA kernels (the native engine, `genai-native`)

`nvcc` wants VS 2022's `cl.exe` as its host compiler; name it explicitly (the 2019 one is first
in `vswhere`'s order and nvcc 13 refuses it). Checked 2026-10-19 with a one-kernel smoke test:
compiled for `sm_120a` in 3.1 s and ran on the 5090 (compute capability 12.0).

```powershell
$env:PATH = "C:\Users\5090\scoop\apps\cuda\current\bin;$env:PATH"
$cl = (Get-ChildItem "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\*\bin\Hostx64\x64\cl.exe" | Select-Object -First 1).FullName
nvcc -arch=sm_120a -ccbin "$cl" -o kernel.exe kernel.cu
```

stable-diffusion.cpp's prebuilt CUDA 12 build (arch list includes 120) lives in
`C:\Users\5090\Documents\_eval\sdcpp-bin` with its `cudart` zip; it needs no toolkit.

### ONNX Runtime and the SAM 3.1 export (`gn-perception`)

| What | Where | Notes |
|---|---|---|
| ONNX Runtime 1.31.0, Windows x64 GPU, **CUDA 13** build | `C:\Users\5090\Documents\_eval\onnxruntime-cuda13\onnxruntime-win-x64-gpu_cuda13-1.31.0\lib` | `onnxruntime.dll` + `onnxruntime_providers_{shared,cuda,tensorrt}.dll`; the `ort` crate loads it at run time (`load-dynamic`), nothing is linked |
| CUDA 13 runtime libraries | `C:\Users\5090\scoop\apps\cuda\current\bin\x64` | `cudart64_13`, `cublas64_13`, `cublasLt64_13`, `cufft64_12`; preloaded by name |
| cuDNN 9.24 | `…\ComfyUI_windows_portable\python_embeded\Lib\site-packages\torch\lib` | every `cudnn*_9.dll` is preloaded (9.24's `cudnn_engines_tensor_ir64_9.dll` is not in `ort`'s own list) |
| SAM 3.1 ONNX graphs (ours) | `C:\Users\5090\Documents\_eval\sam31-onnx` | `vision/text/geometry/decoder/heads.onnx` + `.data`, `tokenizer.json`; 3.4 GB; SAM licence |
| SAM 3 community export (fp32 inside) | `C:\Users\5090\Documents\_eval\sam3-onnx-fp16` | `kishanstar2003/SAM3_ONNX_FP16`; the bring-up set, detector only |
| Python packages added to ComfyUI's interpreter (2026-10-19) | `python_embeded` | `onnx` 1.23.2, `onnxscript` 0.7.2 (the exporter), `onnxruntime` 1.31.0 CPU (verification) |

```powershell
$py = "C:\Users\5090\ComfyUI\ComfyUI_windows_portable\python_embeded\python.exe"
# export (≈1 min; the vision graph is 1.87 GB) and verify the small graphs against torch
& $py -s C:\Users\5090\Documents\genai-native\tools\export_sam31.py --comfy C:\Users\5090\ComfyUI\ComfyUI_windows_portable\ComfyUI --out C:\Users\5090\Documents\_eval\sam31-onnx --verify --image <png>
# run one prompt natively
C:\Users\5090\Documents\genai-native\target\release\gn-sam.exe run --ort <ort lib dir> --lib <cuda x64> --lib <torch\lib> --models C:\Users\5090\Documents\_eval\sam31-onnx --image <png> --text fox --out mask.png
```

`tools/sam31_smoke.py` runs ComfyUI's own node from a script (the ground truth: bounds and
timings); `comfy.model_management.in_training = True` during the export keeps RoPE on the
plain-torch path (`comfy_kitchen.apply_rope` has no ONNX translation).

## Build and run

Cold release build of the desktop app on this machine: **2 min 39 s**
(`cargo build --release -p photocraft`, 2026-10-08), producing `target\release\photocraft.exe`
(58.5 MB). Dependencies are compiled at `opt-level 2` even in dev builds (workspace profile), so a
debug build is not much faster; use `--release` for anything interactive, as upstream says.

```powershell
cargo build --release -p photocraft           # desktop app
cargo build --release -p photocraft --features alerts   # with the completion sounds (as the packages)
cargo build --release -p photocraft-cli       # headless CLI + MCP server
.\target\release\photocraft.exe --version     # prints "photocraft 0.5.0 (dev build)"
.\target\release\photocraft.exe image.psd     # open a file
.\target\release\photocraft-cli.exe --help    # the CLI explains itself
```

Gotcha (PowerShell): JSON arguments to the CLI need their inner quotes backslash-escaped even
inside single quotes, because Windows PowerShell rebuilds native command lines:
`--new '{\"width\":64,\"height\":48}'`. Plain `'{"width":64}'` reaches the program as `{width:64}`.

Gotcha: `photocraft.exe --help` is not a CLI flag; the app starts its window. Only `--version`,
`--control <port>`, `--control-token-file`, `--automation-*-root` and `--safe-gpu` are understood
(see `docs/development.md`). Kill a stray window with `Stop-Process -Name photocraft`.

Drive the running app from PowerShell (the upstream docs show `nc`; Windows has none by default):

```powershell
.\target\release\photocraft.exe --control 7878 --control-token-file .private\control.token
# in another shell, once the token file exists:
$token = (Get-Content .private\control.token -Raw).Trim()
$c = New-Object Net.Sockets.TcpClient('127.0.0.1', 7878); $s = $c.GetStream()
$w = New-Object IO.StreamWriter($s); $w.AutoFlush = $true; $r = New-Object IO.StreamReader($s)
$w.WriteLine('{"id":"auth","method":"auth","params":{"token":"' + $token + '"}}'); $r.ReadLine()
$w.WriteLine('{"id":1,"method":"ui.screenshot","params":{"path":"evidence/shot.png"}}'); $r.ReadLine()
```

Offscreen UI snapshots without a window (upstream rule 6: verify UI changes visually):

```powershell
$env:WGPU_BACKEND = "vulkan"   # on this PC the DX12 path panics in egui-wgpu's staging belt (renderer.rs:984)
cargo run --release -p photocraft-ui-egui --example snapshot -- --out ui.png --size 1440x900 --scale 1
```

`WGPU_FORCE_FALLBACK_ADAPTER=1` (the software adapter) works too. `--script` takes a JSON array
of control-protocol calls; from PowerShell put `--%` before `--` and escape the quotes as `\"`,
one invocation per line, e.g. `--script "[[\"ui.menu.invoke\",{\"id\":\"generate.fill\"}]]"`.

PowerShell 5.1 and JSON arguments in general: an argument with `\"` escapes and **no spaces**
survives a normal call (`& $exe run $img --params '{\"x\":1}'`), but one with spaces (a prompt)
is re-quoted wrongly and the CLI sees extra positional arguments ("run needs exactly one of
<file> or --new"). The dependable form is the stop-parsing token with the dynamic parts in
environment variables, which `--%` expands cmd-style: `$env:P = '{\"prompt\":\"a red boat\"}'`
then `& $exe --% run %IMG% --cmd generate.fill --params "%P%"`. `docs/comfy/bench/bench-fill.ps1`
is written that way. Never assign to `$args` in a script (it is PowerShell's own parameter array).
`--click-at X,Y` opens a top menu (Edit is at 90,16; Select at 261,16 in a 1440×900 capture) and a
`ui.pointer` call with `"tool":"RectMarquee","button":"secondary"` opens the selection context
menu. The dialog and menu captures of the Phase 2 slice are in `C:\Users\5090\ComfyUI\photocraft-tests\ui-*.png`.

## Tests and gates

```powershell
cargo test --workspace                                  # everything (see devlog.md for the baseline run)
cargo test -p photocraft-engine                         # the command engine
cargo test -p photocraft-engine --test panic_hunt -- --ignored   # adversarial params: must stay green
cargo clippy -p <crate> --all-targets -- -D warnings
cargo xtask layers                                      # dependency layering (register new crates in xtask/src/layers.rs)
rustup target add wasm32-unknown-unknown; cargo xtask wasm   # the web gate for L0–L6
cargo xtask parity                                      # regenerates docs/parity.md after adding commands
cargo xtask scorecard --check                           # CI's staleness check for docs/scorecard.md
cargo xtask corpus --all; cargo xtask test-corpus       # real-file corpora (opt-in locally)
```

`cargo xtask` is the workspace's task runner (`xtask/`); `cargo xtask --help` lists everything.

## Agent delegation on this machine

The Claude Code session that set this up uses an orchestration skill that routes grind work to
Codex (repo analysis, tests, git) and agy/Gemini (research, visual QA). Findings on 2026-10-08:

- `codex exec - -s read-only -C <repo> -m gpt-5.5 -c model_reasoning_effort=xhigh -o <report>`
  with the prompt on stdin works and produced the codebase analysis behind
  [`codebase-orientation.md`](codebase-orientation.md).
- `agy -p` in headless mode cannot run tools without an allow-rule
  (`permissions.allow` in its `settings.json`), so research was done with web search directly.
  Passing a multi-line prompt as an argument also breaks on PowerShell quoting; pipe it on stdin.
- ComfyUI is not installed yet; see [`comfyui-setup.md`](comfyui-setup.md).

## Remotes and pushing

Published 2026-10-08: the fork is **https://github.com/CodyChuGit/comfy-photocraft** (a public
GitHub fork of storytold/photocraft, created from the checkout with
`gh repo fork --fork-name comfy-photocraft --remote`, which renamed the old `origin` to `upstream`).

```text
origin    https://github.com/CodyChuGit/comfy-photocraft.git
upstream  https://github.com/storytold/photocraft.git
```

`gh` is logged in as `CodyChuGit` (HTTPS, keyring). Two gotchas on this PC:

- scoop's git ships a system `credential.helper=helper-selector`, a GUI chooser that blocks a
  non-interactive push (git runs every configured helper in order). The repo's local config
  therefore resets the list and uses gh only; an empty helper entry is the reset, and Windows
  PowerShell 5.1 drops empty-string arguments to native programs, so write it with the
  stop-parsing token:

  ```powershell
  git --% config --local --add credential.helper ""
  git config --local --add credential.helper "!gh auth git-credential"
  git config --show-origin --get-all credential.helper    # expect: helper-selector, (empty), !gh auth git-credential
  ```

  For a one-off command elsewhere:
  `git -c credential.helper= -c credential.helper='!gh auth git-credential' push …`.
  `gh auth setup-git` would do the same globally.
- The SSH key in `~/.ssh` is not registered with GitHub; stay on HTTPS.

```powershell
git fetch upstream                 # upstream main moves several times a day
git push                           # comfy-photocraft tracks origin/comfy-photocraft
```
