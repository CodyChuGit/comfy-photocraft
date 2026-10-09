# Comfy PhotoCraft dev log

Newest entry first. Terse: what landed, numbers, what is still open. Upstream keeps its log in
the gitignored `log/devlog.md`; this one is tracked so the next session (any machine, any agent)
can pick up.

## 2026-10-08 (later): Phase 1, the generative backend and `generate.fill`

**Landed**

- `crates/genai` (`photocraft-genai`, L4, registered in `xtask/src/layers.rs`): the
  `GenerativeBackend` trait (`health`, `run`, `model_files`), `Request`/`Response` over RGBA8 +
  Gray8 pixels, `Progress` for job progress and cancellation, `template` (API-format graphs with
  typed `{{placeholders}}`; built-in `qwen-edit-2511/fill` from ComfyUI's official Qwen-Image-Edit
  2511 template plus `ImageToMask` → `SetLatentNoiseMask` for inpainting), `comfy` (a blocking
  client on ureq 3.4 + tungstenite 0.30, no TLS: `/system_stats` version check → `/upload/image`
  ×2 → `/ws` → `/prompt` → socket progress + `/history` polling → `/view`; cancel = `/interrupt` +
  queue delete; deadline from the preference), `png` (via photocraft-codecs), and `fake` (feature
  `fake-server`: an in-process ComfyUI stand-in with HTTP + WebSocket, knobs for delay, failure,
  rejection, missing nodes, no-socket).
- `crates/engine/src/generate_cmds.rs`: `generate.fill` (validated params → composite crop with a
  25 % context margin + selection coverage → backend → result resampled if needed → **new raster
  layer in the document's own depth, above the active layer, with a layer mask equal to the
  selection**; one undo step; background job with progress and Esc-cancel), `generate.health`,
  `generate.models`. Commands have `menu: &[]` until Phase 2 (so parity and i18n are untouched).
- `generate.image` (pulled forward from Phase 3 on request): text to image with the built-in
  `krea2-turbo/image` template (from ComfyUI's official `image_krea2_turbo_t2i.json`: UNETLoader,
  CLIPLoader type `krea2`, CLIPTextEncode, ConditioningZeroOut negative, EmptyLatentImage,
  KSampler euler/simple 8 steps CFG 1, VAEDecode). `target: "layer"` covers the whole canvas
  (size defaults to the document, rounded down to the 16-px grid, min 64, resampled back),
  `target: "document"` opens a new RGB 8-bit document of the result's size (default 1024²).
  Krea 2 is a community-licence model: the catalogue says so and nothing pre-selects it for fill.
- Preferences › AI Integrations: `comfyServer`, `defaultEditModel`, `defaultGenerateModel`,
  `generativeTimeoutSecs`, `allowResearchModels`, each read by the commands and validated.
- Tests: 25 in the genai crate (unit + the client against the fake server: upload/queue/wait/
  download, polling without a socket, overrides, server failure, rejected prompt, cancellation
  interrupts within 3 s, deadline, unreachable server, old server version, health/model files,
  request validation) and 18 engine command tests (fill: masked layer geometry and pixels, undo,
  the uploaded crop and mask, background job parity with inline, cancel leaves the document
  alone, server failure, unreachable server, health/models, parameter validation, enablement,
  model overrides, a 16-bit document, resampling; image: canvas layer, new document, size
  rounding and validation, background job, template/task checks, model preference).
  `cargo test -p photocraft-engine --test prefs_usage` green.
- Gates on 2026-10-08: `cargo fmt`, `cargo clippy --all-targets -- -D warnings` on both crates,
  `cargo xtask layers` (29 crates, no violations), `cargo xtask wasm` (genai and engine check for
  wasm32; the client is native-only and seeds fall back to a counter there), `cargo xtask parity`
  (unchanged: 627/627, the commands have no menu rows yet), `cargo xtask scorecard` (57 unread
  settings of 144; the five new preferences are read), `panic_hunt --ignored` green in 25 s with
  the new commands, release `photocraft-cli` built (1 min 43 s); `photocraft-cli commands --filter
  generate` lists the four commands. Headless `generate.health` / `generate.fill` without a server
  return "cannot reach the ComfyUI server at http://127.0.0.1:8188 … start ComfyUI or change the
  server URL in Preferences › AI Integrations".

**Still open**

- A live run against a real ComfyUI (not installed on this PC yet) with the 2511 files; the
  template's node names come from the official template but have not executed here. First live
  run should also decide whether to add back `CFGNorm`.
- Phase 2: menu rows + translations, the generative task bar, variations, previews.
- `docs/comfy/models.toml` catalogue and `generate.image` / `generate.edit` / `removeBackground`
  (Phase 3) reuse the same crate; `select.byText` (Phase 3.5) needs a SAM 3.1 template.

## 2026-10-08: Phase 0, foundation

**Landed**

- Cloned `storytold/photocraft` (`main` @ `d836e34`, 642 commits, 49 MB) to
  `C:\Users\5090\Projects\comfy-photocraft`; branch `comfy-photocraft` created; local git identity
  set to the owner's email.
- Toolchain on the Windows PC: `scoop install rustup gh` → rustup 1.29.1, Rust stable 1.99.0
  (MSVC, using the pre-installed VS 2019 Build Tools), gh 2.102.0. No Python/Node on the machine
  (not needed for Rust; ComfyUI brings its own Python).
- Release build: `cargo build --release -p photocraft` **2 min 39 s** cold; `photocraft.exe`
  58.5 MB; `--version` → `photocraft 0.5.0 (dev build)`. `cargo build --release -p photocraft-cli`
  2 min 10 s (incremental on the same deps).
- Test baseline, `cargo test --workspace --no-fail-fast` (debug profile, 138 s after the build):
  **171 test binaries, 4227 passed, 9 failed, 32 ignored.** All failures are environment-related
  on this RTX 5090 / driver 617.42 / wgpu DX12 machine, none in engine or format code:
  - `photocraft-gpu --test parity adjustment_layers`: "Hue/Saturation Normal: max diff 1.00/255
    at (18,23)", i.e. exactly at the tolerance edge (GPU rounding differs from CI's software
    adapter).
  - 7 egui-wgpu canvas tests (`canvas_16f` ×2, `color_managed_canvas` ×2, `drag_preview_canvas`
    ×2, `live_stroke_canvas` ×1) panic inside `egui-wgpu 0.36.2 renderer.rs:984` "Failed to create
    staging buffer for index data" (a wgpu staging-belt issue on this adapter, not PhotoCraft code).
  - `photocraft-ui-egui --lib input_tests::eyedropper_and_alt_sampling_show_a_pipette`:
    "Precise keeps the crosshair" (unexplained; may read the machine's real preferences; retest
    with `PHOTOCRAFT_CONFIG_DIR` pointing at an empty folder).
  Everything else (engine 831 tests, ui-egui lib 915, io, psd, codecs, compose, automation…) is green.
- Research (web, dated 2026-10-08) on Krea 2 (open weights 2026-06-22, Krea 2 Community License,
  ComfyUI ≥ 0.26), Qwen-Image-2.1 (2026-09-20, **research-only licence**), Qwen-Image-Edit-2511
  (Apache-2.0), Z-Image Turbo (Apache-2.0), FLUX.2 variants, the ComfyUI HTTP/WebSocket API,
  Photoshop 2026 generative feature list, Krita's ComfyUI plugin, Rust ComfyUI crates.
  Finding worth repeating: **"Krea 2 Edit" is a community LoRA, not an official Krea model.**
- Codebase analysis delegated to Codex (`codex exec`, read-only, gpt-5.5) and verified by hand;
  key facts: `CommandSpec` fn-pointer registry, `jobs::edit_job` background jobs with progress
  and cancel, `ml` reserved at L4 in `xtask/src/layers.rs`, no HTTP client in the tree, an
  existing `integrations` preference section with an "AI Integrations…" menu entry.
- Documentation set in `docs/comfy/` (README, codebase-orientation, architecture, models,
  comfyui-setup, roadmap, dev-environment-windows, upstream, this log) and a root `CLAUDE.md`;
  a fork notice in the top-level README.

- `ai-integration-proposal.md`: the full catalogue of ComfyUI + local-LLM integrations (17
  generative, 9 perception, 8 language items, 8 infrastructure items) with a recommended order;
  adds "Select by text" and the in-app Assistant as new phases 3.5 and 4.5.
- SAM 3.1 researched and written in: Meta, 2026-03-27, text/point/box prompts, native in ComfyUI
  (PR #13408, one 1.75 GB checkpoint), SAM License (commercial OK, military/weapons banned, licence
  passed on). It is the model behind Phase 3.5 (`select.byText`, `select.subjectML`,
  `select.byPoint`); design sketch in the proposal §2.1.

**Numbers to carry forward**

| Metric | Value | How |
|---|---|---|
| Release build, cold | 159 s | `cargo build --release -p photocraft` |
| Test suite | 4227 pass / 9 fail / 32 ignored, 171 binaries | `cargo test --workspace --no-fail-fast` |
| Upstream menu parity | 627/627 | `docs/parity.md` |
| Generative commands | 0 | — |

- Published: fork **https://github.com/CodyChuGit/comfy-photocraft** created with `gh repo fork`,
  `comfy-photocraft` pushed (`5aed6c2`), `upstream` → storytold/photocraft. The push first hung on
  scoop git's `helper-selector` credential GUI; the repo now uses gh as its only credential
  helper (`dev-environment-windows.md` › Remotes and pushing).

**Still open**

- Install ComfyUI (portable build) and the Phase 1 model files; record versions and VRAM here.
- Decide the published product name and the brand-asset removal before any public build
  (`docs/comfy/upstream.md`).
- Check whether the 7 egui-wgpu staging-buffer failures and the 1/255 parity edge are known
  upstream (search issues for "staging buffer" / "parity 1.00/255"); if not, file them with the
  adapter/driver details above.
- Phase 1 starts with `crates/genai` (see `docs/comfy/roadmap.md`).
