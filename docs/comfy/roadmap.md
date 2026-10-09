# Comfy PhotoCraft roadmap

Status legend: ✅ done · 🟡 in progress · ⬜ not started. Updated 2026-10-08.

Each phase ends with something a user can run. Every phase obeys upstream's gates
(`AGENTS.md` §5): tests, clippy, `cargo xtask layers`, `cargo xtask wasm`, `cargo xtask parity`,
`panic_hunt`, a dev-log entry. Design details are in [`architecture.md`](architecture.md); the
code map is in [`codebase-orientation.md`](codebase-orientation.md).

| Phase | Status | Ships |
|---|---|---|
| 0 Foundation | ✅ 2026-10-08 | This documentation, the Windows toolchain, a green release build, the `comfy-photocraft` branch |
| 1 Backend + headless Generative Fill | ✅ 2026-10-09: live against ComfyUI 0.39.0 with Qwen-Image-2.1, Krea 2 Turbo and Qwen-Image-Edit-2511 (numbers in the dev log) | `photocraft-genai` crate, ComfyUI client, `generate.fill` / `generate.image` (pulled forward from Phase 3) / `generate.health` / `generate.models` from the CLI and MCP |
| 2 Generative Fill in the app | ⬜ | Prompt bar, progress, variations, generative layers with masks, model picker, preferences |
| 3 The Firefly core set | ⬜ | Expand, Generate Image, instruction Edit, Remove Background |
| 3.5 Select by text (SAM 3.1) | 🟡 2026-10-09: `select.byText` and `select.subjectML` live (text prompts); point/box prompts and soft-matte refinement pending | `select.byText`, ML Select Subject, point/box object selection, SAM-backed mattes for Remove Background |
| 4 Models and workflows as data | ⬜ | Model catalogue, workflow template import, references, LoRAs, downloads with checksums |
| 4.5 Assistant | ⬜ | In-app assistant panel over a local LLM/VLM, driving the command registry; prompt enhancement, auto-naming |
| 5 Quality and depth | ⬜ | Harmonize, Generative Upscale, Generate Similar, generative smart objects, PSD interop |
| 6 Beyond ComfyUI | ⬜ | Native inference backend behind the same trait; web-build path |

## Phase 0: Foundation ✅

Done 2026-10-08 (see [`devlog.md`](devlog.md)): repository cloned, rustup + MSVC build verified
(release build 2 min 39 s), `cargo test --workspace` run for a baseline, research on the models
and the ComfyUI API, the design, and this documentation set. Open item: publish the branch (needs
`gh auth login`; [`dev-environment-windows.md`](dev-environment-windows.md) › Publishing).

## Phase 1: Backend crate and headless Generative Fill

**Goal.** `photocraft-cli run photo.png --cmd select.all --cmd generate.fill --params '{"prompt":"a red bicycle"}' --out out.psd`
produces a layered file with a new "Generative Fill" layer, against a local ComfyUI.

1. New crate `crates/genai` (`photocraft-genai`), registered at **L4** in `xtask/src/layers.rs`
   (same layer as `plugins` and the reserved `ml`). Depends on `geom`, `color`, `raster`, `doc`,
   `codecs` (PNG encode/decode) only. Carries the never-crash lints.
2. `GenerativeBackend` trait: `capabilities()`, `models()`, `submit(request) -> JobTicket`,
   `poll/wait(ticket, progress, cancel)`, `interrupt(ticket)`, `health()`.
3. `comfy` module: blocking HTTP client (pure Rust, rustls) + WebSocket progress; the flow in
   [`comfyui-setup.md`](comfyui-setup.md) › API primer. Cancellation calls `/interrupt` and drops
   the ticket. Every failure is an `Err` with the server's message.
4. Workflow templates as API-format JSON with `{{placeholders}}` in `crates/genai/workflows/`
   (fill, expand, image, edit, matte), one per model family, validated at load.
5. Engine: `crates/engine/src/generate_cmds.rs` with `generate.fill` using `jobs::edit_job`
   (background, cancellable, one undo step). Params: `prompt`, `negative`, `model`, `seed`, `steps`,
   `variations` (1 in Phase 1), `margin` (context around the selection), `layer`, `target`.
   Result: the new layer id, the seed used, timing.
6. Preferences in the existing `integrations` section (Edit › Preferences › AI Integrations…):
   `integrations.comfyServer` (URL), `defaultGenerateModel`, `defaultEditModel`,
   `generativeTimeoutSecs`, read by the command (so the "settings that do nothing" audit stays at
   its number).
7. Tests: unit tests for template filling and response parsing; an in-process fake ComfyUI
   (a `std::net::TcpListener` serving canned `/prompt`, `/history`, `/view`, `/ws`) for the
   command tests; one `#[ignore]` live test against a real server (env `PHOTOCRAFT_COMFY_URL`);
   `panic_hunt` coverage; graceful failures (server down, bad JSON, cancelled mid-run).

**DoD.** The CLI line above works on the development PC with Qwen-Image-Edit-2511; `cargo xtask
layers`, `wasm` (the crate compiles for wasm with the client behind `cfg(not(target_arch = "wasm32"))`,
returning "unsupported on the web"), `parity`, `panic_hunt` and clippy are green; VRAM and timing
numbers recorded in the dev log.

**Status 2026-10-09: done.** Items 1–7 are implemented (`crates/genai`,
`crates/engine/src/generate_cmds.rs`, the `integrations.*` preferences, 25 crate tests and 21
command tests against the in-process fake server), and the live DoD was met on the dev PC against
ComfyUI 0.39.0: Generative Fill with Qwen-Image-2.1 (4.7 s warm) and Qwen-Image-Edit-2511, text to
image with Krea 2 Turbo (9.8 s cold) and 2.1; timings and VRAM in [`devlog.md`](devlog.md).

## Phase 2: Generative Fill in the app

**Goal.** Select, type a prompt in the contextual bar, press Generate, pick one of three
variations, keep it as a masked layer. Feels like Photoshop.

1. Menu: **Edit › Generative Fill…** in `menu_catalog.rs` / `menus.rs` (upstream's catalogue has
   no generative entries, so this is an addition, not a parity row), plus the selection context
   menu entry (`canvas_tool_menu.rs` already lists it as Photoshop's order).
2. A **Generative task bar** widget (prompt field, model picker, Generate, variations strip,
   Cancel) anchored under the selection; UI state in `state.rs` so the control channel can drive
   it (`ui.set`), strings through `tl!` with catalogue coverage.
3. Progress: the job's `JobInfo` drives a progress bar with the ComfyUI step count; binary preview
   frames shown as a dimmed overlay inside the selection (optional, preference).
4. Variations: `variations: 3` runs one prompt with three seeds (batched in one workflow when the
   model allows); results are kept as hidden sibling layers in a group "Generative Fill" with the
   chosen one visible, or as a layer with stored alternates (decision in Phase 2 design).
5. Generative layer metadata (prompt, negative, model, seed, workflow hash, server version) stored
   on the layer (a `doc` field → also `format` manifest + PSD passthrough as an unknown block).
6. Preferences UI: Edit › Preferences › Generative (server URL, default model, timeouts, previews,
   NSFW filter toggle for community-licence models that require moderation).
7. Model picker reads the catalogue and the server's `object_info`; research-only models carry a
   badge and are never pre-selected.

**DoD.** Visual check with the offscreen snapshot example; a control-protocol script that selects,
fills and screenshots is committed as a test; `ui.inspect` exposes the task bar state.

## Phase 3: The Firefly core set

- **Generative Expand** (`generate.expand`): hooks Image › Canvas Size and the Crop tool's
  "expand" state; pads the composite, masks the padding with an inward feather.
- **Generate Image** (`generate.image`): new document or new layer; width/height from the
  document; Krea 2 Turbo or Z-Image Turbo; optional reference image.
- **Instruction Edit** (`generate.edit`): edit the active layer or the composite with an
  instruction; optional mask; references.
- **Remove Background** (`generate.removeBackground`): produces a layer mask from a matte
  (BiRefNet) or from Qwen-Image-2.1's RGBA output; never destroys pixels; also exposed as
  Select › Subject (generative) feeding the ordinary selection.

**DoD.** Each command documented, tested, in `docs/parity.md`-style generated listings, drivable
over MCP; the ten-task MCP acceptance test gains two generative tasks.

## Phase 3.5: Select by text (SAM 3.1)

Design in [`ai-integration-proposal.md`](ai-integration-proposal.md) §2.1. Adds a SAM 3.1
template to the genai crate (same client, no diffusion model), the commands `select.byText`,
`select.subjectML` and `select.byPoint`, Select › Select by Text… and the Object Selection tool's
ML mode, and swaps the matte source of Remove Background when the server is up. Licence: SAM
License (commercial allowed, military/weapons uses banned, licence passed on; the app ships no
weights). **DoD.** "select the dog" on a public-domain test image yields one undoable selection
per instance in under a second on the 5090; classical fallback when the server is down; `panic_hunt`
green; the MCP acceptance test gains a select-by-text task.

**Status 2026-10-09.** `select.byText` and `select.subjectML` landed and ran live ("the
lighthouse" in 4.6 s cold, "sky" in 1.8 s warm; see the dev log). Open: `select.byPoint`, the
BiRefNet soft-matte refinement, the MCP acceptance task, and the menu/tool UI (Phase 2 work).

## Phase 4: Models and workflows as data

- `docs/comfy/models.toml` → compiled catalogue: id, family, task, licence class + URL, files +
  sha256, VRAM class, default settings, template id.
- Template import: a user can drop an API-format workflow exported from ComfyUI with named
  placeholder nodes; PhotoCraft validates it against `object_info` and lists it in the picker.
- Downloads: a "Download models" helper that fetches the files for a template into the ComfyUI
  folder with checksum verification and progress (a background job; native only).
- LoRA and style reference selection (Krea 2 style-reference workflow), reference images for
  Qwen-Image-2.1 (up to 10).
- Health panel: server version, free VRAM, loaded models, queue length; one-click `/free`.

## Phase 5: Quality and depth

- Harmonize, Generative Upscale, Generate Similar.
- Generative smart objects: the generation becomes a smart object whose source records the
  request; "regenerate" re-runs it non-destructively; smart filters apply on top.
- PSD interop: generative layers export as ordinary raster layers with masks; metadata survives as
  a private tagged block so re-import keeps it.
- Batch: `generate.*` in actions and `photocraft-cli batch` (e.g. remove backgrounds for a folder).
- Measured quality: a small public-domain test set with prompts and expected properties (mask
  respected, seams within tolerance, no change outside the mask), run nightly like
  `perf-nightly.yml`.

## Phase 6: Beyond ComfyUI

- A second `GenerativeBackend`: in-process inference for small models (candle or burn, pure
  Rust, feature-gated, native only), starting with matting (BiRefNet-class) and a small
  text-to-image model; keeps the UI identical.
- Web build: the genai crate's HTTP path on wasm via `fetch` (`ehttp`-style), talking to a user's
  own loopback server when the browser allows it; otherwise the commands stay "unsupported on the
  web".

## Risks and decisions to make

| Risk / decision | Mitigation |
|---|---|
| Model licences (Qwen-Image-2.1 research-only; Krea 2 revenue cap and moderation duty) | Catalogue carries licence class; permissive defaults; moderation hook for community models |
| ComfyUI API drift (node names change per release) | Templates pinned per model family and validated against `object_info` at load; version check in `health()` |
| Blocking HTTP inside a job worker | Jobs already run on worker threads with cancellation; the client polls `ctx.cancelled()` between requests and uses short read timeouts |
| wasm gate | Client behind `cfg(not(target_arch = "wasm32"))`; the crate and the commands compile everywhere |
| Large transfers (2K RGBA PNG round trips) | Crop to selection bounds + margin; stream to disk when over a size cap; measure |
| Brand licence on publication | [`upstream.md`](upstream.md) checklist before the first published build |
| Upstream merges touching `menus.rs`, `commands.rs`, `state.rs` | Keep fork edits to those files surgical; put commands in a new module (`generate_cmds.rs`) |
