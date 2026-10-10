# Generative architecture

Design for the generative toolset, written 2026-10-08 against upstream commit `d836e34`
(PhotoCraft 0.5.0). It reuses upstream's machinery wherever one exists and adds exactly one crate,
one engine command module, one preference group and one UI widget family. File references are
`path:line` in that commit; [`codebase-orientation.md`](codebase-orientation.md) explains each
subsystem in more depth.

## 1. Shape of the system

```text
 egui shell (ui-egui)             CLI / control channel / MCP
   Generative task bar  ─┐          photocraft-cli run --cmd generate.fill
   Edit › Generative…    │          command_run / engine.execute
                         ▼
             engine: generate_cmds.rs  (generate.fill | expand | image | edit | removeBackground | upscale)
                         │  jobs::edit_job  (worker thread, progress, cancel, one undo step)
                         ▼
             photocraft-genai (L4)
               GenerativeBackend trait ── ComfyBackend (HTTP + WebSocket, pure Rust)
               request builder: composite crop + mask → PNG            workflow templates (API JSON)
               response: PNG → Surface → new layer + layer mask        model catalogue (data)
                         │
                         ▼
             ComfyUI server on 127.0.0.1:8188  (Krea 2, Qwen-Image-Edit-2511, Qwen-Image-2.1, Z-Image, BiRefNet…)
```

Three facts about upstream make this cheap:

1. **Every action is a `CommandSpec`** (`crates/engine/src/commands.rs:15-28`): `id`, `label`,
   `menu`, `shortcut`, a `params` doc string, `enabled: fn(&Session) -> Result<(), String>`,
   `run: fn(&mut Session, &Value) -> Result<Value>`, `journal`. Modules expose `specs()` and are
   collected by `v.extend(...)` in `commands.rs` (`build()`, around line 1020). Menus, the ⌘K
   palette, the CLI, the control channel and MCP all dispatch by id, so new generative commands are
   reachable from every surface the moment they are registered.
2. **Background jobs exist** (`crates/engine/src/jobs.rs`): `jobs::edit_job` (line 347) runs a
   closure against a *copy* of the active document on a worker thread when the command was started
   with `Session::start` (line 398), reports progress through `JobCtx::progress` (line 65), is
   cancelled through `JobCtx::cancelled` / `check` (lines 78–90), and is applied on the UI thread as
   one undo step by `Session::poll_jobs` (line 473). While it runs, other edits of that document are
   refused with a message naming the job (`job_conflict`, line 573). The CLI and MCP block with
   `wait_job` (line 497). `jobs.list` and `jobs.cancel` are commands (lines 739–773). Filters
   already use this (`crates/engine/src/filters.rs:213-262`); generation is the same pattern with a
   network round trip instead of a tile loop.
3. **Layering is a table** (`xtask/src/layers.rs:35-72`): L4 already reserves `ml` and holds
   `plugins`; a new L4 crate may depend on L0–L3 (`geom`, `cms`, `color`, `raster`, `doc`, `ops`,
   `algo`, `paint`, `text`, `vector`, `compose`, `gpu`, `format`) and the standalone codecs, and on
   nothing from `engine` or the UI. UI crates are forbidden below L6 (`UI_MIN_LAYER`, line 103).

Upstream's menu catalogue deliberately omits Photoshop's Generative Fill row
(`crates/ui-egui/src/canvas_tool_menu.rs:37-39`: "has no PhotoCraft command and is left out"), so
the fork adds the entries rather than wiring existing parity rows.

## 2. The `photocraft-genai` crate (L4)

`crates/genai`, package `photocraft-genai`, registered as `("genai", Class::Layer(4))` in
`xtask/src/layers.rs` (`ml` stays reserved for a future in-process inference crate). Lints: the
never-crash set from `AGENTS.md` (`#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic,
clippy::unimplemented, clippy::todo, clippy::unreachable)]`), `unsafe_code = "forbid"` inherited.

### 2.1 Public API

```rust
/// What a backend can do; the picker and `enabled` predicates read this.
pub struct Capabilities { pub tasks: Vec<Task>, pub max_side: u32, pub references: u8, pub rgba_output: bool, pub preview_frames: bool }

pub enum Task { Generate, Inpaint, Outpaint, Edit, Matte, Upscale }

/// One request, backend-agnostic. Pixels travel as straight-alpha RGBA8 (or RGBA16 where the model
/// accepts it) with a document-space rectangle; the mask is 8-bit coverage over the same rect.
pub struct Request {
    pub task: Task,
    pub model: ModelId,
    pub prompt: String,
    pub negative: Option<String>,
    pub seed: Option<u64>,
    pub steps: Option<u16>,
    pub guidance: Option<f32>,
    pub variations: u8,                 // 1..=8
    pub image: Option<ImageInput>,      // composite crop (+ context margin) or the source layer
    pub mask: Option<MaskInput>,        // selection coverage, feathered, same rect as `image`
    pub references: Vec<ImageInput>,    // style / identity references
    pub size: Option<(u32, u32)>,       // text-to-image only
}

pub struct Response { pub images: Vec<GeneratedImage>, pub seed: u64, pub timing: Timing, pub server: ServerInfo }
pub struct GeneratedImage { pub rect: Rect, pub rgba: Surface /* photocraft-raster */, pub alpha_is_matte: bool }

pub trait GenerativeBackend: Send + Sync {
    fn health(&self) -> Result<Health>;                       // version, free VRAM, queue length
    fn models(&self) -> Result<Vec<ModelInfo>>;               // installed = catalogue ∩ server files
    fn capabilities(&self, model: &ModelId) -> Result<Capabilities>;
    fn run(&self, req: &Request, progress: &dyn Progress) -> Result<Response>;  // blocking; polls `progress.cancelled()`
}

pub trait Progress { fn report(&self, fraction: f32, message: &str); fn cancelled(&self) -> bool; }
```

`Progress` is implemented for `photocraft_engine::jobs::JobCtx` **in the engine** (the genai crate
must not depend on the engine), by a small adapter in `generate_cmds.rs`. On wasm the trait still
compiles; `ComfyBackend::new` returns `Err(Unsupported("the web build cannot reach a local server
yet"))` so the commands' `enabled` predicates grey out, exactly as file-system commands do today.

### 2.2 `comfy` module: the client

A thin, blocking client written for this crate (two Rust ComfyUI crates exist, see
[`comfyui-setup.md`](comfyui-setup.md), but neither is wasm-aware or dependency-light; the protocol
is five routes and one socket). Dependencies, all pure Rust with rustls where TLS is involved:

| Need | Crate | Why |
|---|---|---|
| HTTP | `ureq` (rustls) native; `ehttp`/`fetch` on wasm later | blocking fits the job worker; no tokio in a L4 crate (tokio appears only in `automation` and the CLI today, `crates/automation/Cargo.toml:24`) |
| WebSocket | `tungstenite` (rustls) native; `web_sys::WebSocket` on wasm later | progress and preview frames; the socket is optional, polling `/history` is the fallback |
| JSON | `serde_json` (workspace) | templates, requests, responses |
| PNG | `photocraft-codecs` | encode the crop and mask, decode results; honours depth |
| UUID | a 128-bit random `client_id` from `getrandom` (workspace dep) formatted as hex | no `uuid` crate needed |

Flow per `run` (details and field names in [`comfyui-setup.md`](comfyui-setup.md) › API primer):

1. `health()` once per session: `GET /system_stats`; refuse with a clear error if the server is
   missing or older than the template's `min_version`.
2. `POST /upload/image` (crop) and `POST /upload/mask` (or a second image) with unique names
   (`photocraft-<job>-<n>.png`), `type=input`, `overwrite=true`.
3. Fill the template; `POST /prompt` with `client_id`; keep `prompt_id`.
4. Open `/ws?clientId=…`; map `progress.value/max` to `Progress::report`, `executing{node:null}`
   or `execution_success` to done, `execution_error` to `Err` with the server's
   `exception_message`. Check `progress.cancelled()` on every message and every poll tick; on
   cancel `POST /interrupt`, `POST /queue {"delete":[prompt_id]}`, return `Err(Cancelled)`.
5. `GET /history/{prompt_id}` → `outputs.<SaveImage node>.images[]` → `GET /view` each → decode.
6. Optionally `POST /free` when the preference says to release VRAM after a run.

Timeouts: connect 3 s, read 30 s per request, overall from `integrations.generativeTimeoutSecs` (default 600).
Every path returns `GenError` (`Server(String)`, `Protocol(String)`, `Unsupported(String)`,
`Cancelled`, `Io`), never panics; malformed JSON from the server is a `Protocol` error carrying the
first 200 bytes.

### 2.3 Workflow templates

`crates/genai/workflows/<family>/<task>.json`: API-format graphs exported from ComfyUI with the
official template for that model, with values replaced by placeholders:

```json
{ "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "{{prompt}}", "clip": ["38", 0]}},
  "41": {"class_type": "LoadImage",     "inputs": {"image": "{{image}}"}},
  "42": {"class_type": "LoadImage",     "inputs": {"image": "{{mask}}"}},
  "3":  {"class_type": "KSampler",      "inputs": {"seed": "{{seed}}", "steps": "{{steps}}", "cfg": "{{guidance}}", "...": "..."}},
  "9":  {"class_type": "SaveImage",     "inputs": {"filename_prefix": "{{prefix}}", "images": ["8", 0]}} }
```

A sidecar `template.toml` names the family, task, `min_comfy_version`, the loader nodes whose
combo values are model file names (so `models()` can intersect with `GET /models/<folder>`), the
SaveImage node id, and which placeholders are required. Filling is typed: a placeholder's type is
taken from `object_info` at first use and cached, so `"{{steps}}"` becomes an integer and
`"{{prompt}}"` a string. Templates are validated at load (every placeholder bound, every
`class_type` present on the server); a missing node names the custom-node pack to install.

Phase 1 ships: `qwen-edit-2511/fill.json`, `qwen-edit-2511/expand.json` (same graph with an
`ImagePadForOutpaint` stage done on our side instead), `krea2/image.json`, `zimage/image.json`,
`qwen-2.1/edit.json`, `qwen-2.1/rgba.json`, `birefnet/matte.json`. Phase 4 lets users import their
own.

### 2.4 Pixels in and out

- **Crop with context.** For Fill/Edit the request image is the flattened composite over the
  selection's bounds grown by `margin` (default 25 % of the larger side), rendered by
  `photocraft_compose::render(doc, rect)` (the CPU reference, already used by exports and tests),
  converted to RGBA8 sRGB through `photocraft-cms` when the document is 16/32-bit or not sRGB
  (rule 2 in `AGENTS.md`: never assume 8-bit sRGB inside the engine; the conversion happens at the
  backend boundary and is reversed on the way back). Since 2026-10-09 the request rectangle is
  grown to the 16-px grid, a request over 0.75 megapixels is sent downscaled (Lanczos-3 for
  pixels, a tent filter for the mask; the editing models work at about that size and the 32 GB
  card keeps the model resident at it) and one under 512 px on its longer side is sent upscaled;
  the result is resampled back under the selection's full-resolution layer mask. Segmentation
  requests are capped at 2 MP. Generative Expand shows the model only the picture as its
  reference (an `ImageCrop` in its templates) while the padded canvas is the sampling latent;
  the padding is pre-filled by replicating the picture's edge pixels (a wall of grey leaked its
  tone through the VAE into the edge band as a dark line).
  Upload names are content hashes, so ComfyUI's node cache (loader, text encoder, VAE encode) is
  reused across variations and re-rolls of the same selection. Every run goes through
  `run_switching`: the engine remembers the model files (`folder/file`) of the last run per
  server and, when a request loads a different set (the 2511 base instead of its Lightning
  tier, Qwen-Image-2.1 instead of 2511, SAM), asks the server to unload its models first.
  Without that ComfyUI loads the new set partially next to the old one and streams weights on
  every step (31–131 s a run on the 32 GB card); a purge costs one reload.
- **Mask.** The selection surface (`doc.selection: Option<Surface>`, used by filters at
  `filters.rs:222-223`) cropped to the same rect, as 8-bit coverage; Fill feathers it outward
  (4 % of the request's longer side, 6–48 px, a band of twice that; Expand's band runs into
  the picture) so the
  model re-renders a band around the area and blends the seam.
- **Result.** Decoded PNGs become a `Surface` positioned at `rect.x0, rect.y0` on a new raster layer
  named after the prompt ("Generative Fill: a red bicycle"), with a **layer mask**. With the
  default `edge: soft` the mask is that same feathered band, its ramp dithered with per-pixel
  noise (zero inside and outside, strongest mid-ramp, seeded by the request's seed) so the
  result fades into its surroundings as grain rather than meeting them at a line; `edge: hard`
  makes the mask exactly the selection (or the added canvas). The composite under the mask is
  untouched. For RGBA outputs (`alpha_is_matte`) the alpha becomes the mask instead of being baked
  into the pixels, so Remove Background never destroys data.
- **Memory.** Every layer a generative command makes carries a `GenerativeInfo` (the command,
  the prompt as typed and, when `enhance` rewrote it, the prompt the model actually got
  (`enhanced`), the resolved template, seed, steps, guidance, edge, the request rect and
  the name; images add their size and transparent flag) as JSON in a PSD additional-layer-info
  block keyed `cpGn`: no change to upstream's `Layer`, and it survives PSD round trips and
  `Layer::duplicate`. `generate.info` reads it; `generate.similar` re-runs it.
- **Depth.** The new layer takes the document's depth and profile; 8-bit model output is
  up-converted at the boundary. 32-bit float documents get linear-light results converted from
  sRGB, like `io` does for imports.

## 3. Engine commands: `crates/engine/src/generate_cmds.rs`

A new module with `specs()`, registered in `commands.rs` with one `v.extend(crate::generate_cmds::specs())`
line (upstream `AGENTS.md` §6 asks for exactly this: new commands in a new module, surgical edits
to shared files).

| Id | Menu | Params (doc string style) | Enabled when |
|---|---|---|---|
| `generate.fill` | Edit › Generative Fill… | `{"prompt":str,"negative":str?,"model":id?=pref,"seed":u64?,"steps":u16?,"guidance":f32?,"variations":1..8=1,"margin":0..1=0.25,"layer":id?=active,"wait":bool=false}` | document, selection, backend available |
| `generate.expand` | Edit › Generative Expand… | same + `{"width","height","anchor"}` | document, backend |
| `generate.image` | Edit › Generate Image… | `{"prompt","model","seed","steps","width","height","transparent":bool=false,"references":[path|layer]?,"target":"layer"|"document"}` | backend |
| `generate.edit` | Edit › Generative Edit… | `{"prompt":instruction,"negative"?,"steps"?,"guidance"?,"edge":"soft|hard","variations":1..4,"seed"?,"template":id?=auto,"model"?,"name"?}` → the composite re-rendered by instruction as a new layer, masked to the selection when there is one | document, backend |
| `generate.removeBackground` | Edit › Remove Background (Generative)… | `{"prompt":str?=what to keep,"asSelection":bool=false,"sampleAllLayers":bool=false,"mode","layer":id?=active,"seed"?,"template":id?=auto,"model"?}` → the matte (Qwen-Image-2.1's alpha with the research opt-in, else SAM 3.1's union of instance masks) as the layer's mask or the selection | unlocked pixel layer, backend |
| `select.byPoint` | the Object Selection tool's click (with a server configured; ⇧ adds, ⌥ subtracts; a drag stays the classical rectangle) | `{"x","y","mode","threshold","sampleAllLayers","template":id?=sam3.1/segment-point}` → the object under the point, as `select.byText` | document, backend |
| `select.byText` | Select › Select by Text…; the Object Selection options bar's "Select by text" field (Enter) | `{"prompt","mode","threshold","sampleAllLayers","instance"?,"template":id?=sam3.1/segment}` → the instances SAM 3.1 finds for the phrase, as the selection | document, backend |
| `select.subjectML` | the selection tools' Select Subject button when a server is configured (classical `select.subject` otherwise) | `{"what":"subject|person|face|hair|sky|…","mode",…}` → Select by Text with a fixed phrase | document, backend |
| `generate.splitLayers` | Edit › Split into Layers (Generative)… | `{"layers":int=3,"prompt"?,"negative"?,"sampleAllLayers":bool=true,"steps"?,"guidance"?,"seed"?,"template":id?=qwen-layered/split,"model"?,"name"?}` → N RGBA layers (background first) above the active one | document, backend |
| `generate.similar` | Edit › Generate Similar | `{"layer":id?=active,"seed"?,"variations":1..4}` → runs what made the layer again with a new seed in the same place (its mask is the area), a new layer above it | a generative layer, backend |
| `generate.info` | — | `{"layer":id?=active}` → `{"generative":{command,prompt,enhanced,template,seed,…}|null}` | document (query) |
| `generate.enhancePrompt` | — (the task bar's wand button) | `{"prompt","task":"fill|edit|image","useImage":bool=true,"seed"?}` → `{"prompt","enhanced"}`: the prompt rewritten by the Qwen3-VL 4B encoder run as a vision-language model (ComfyUI's `TextGenerate`), the composite (and a fill's selection place) in view; `enhance: true` on fill/edit/image does it on the way, Preferences › Enhance prompts makes it the default | backend |
| `generate.upscale` | Image › Generative Upscale… | `{"model","factor":2|4}` | pixel layer, backend |
| `generate.models` | — | `{}` → `{models:[…], server}` | always (query, `journal: false`) |
| `generate.health` | — | `{}` → `{ok, version, vramFree, queue}` | always (query) |

Each `run`:

```rust
run: |s, p| {
    let req = build_request(s, p)?;                       // validates every param; Err on anything odd
    let backend = s.genai_backend()?;                      // `Session.genai: Option<Arc<dyn GenerativeBackend>>`, set by the app/CLI like `preset_store` (lib.rs:304), else built from `integrations.comfyServer`
    let label = format!("Generative Fill: {}", short(&req.prompt));
    crate::jobs::edit_job(s, &label,
        move |doc, active, ctx| {
            let input = crop_composite(doc, &req)?;           // compose::render on the worker's copy
            let resp = backend.run(&req.with(input), &JobProgress(ctx))?;   // blocking, cancellable
            let id = add_generative_layer(doc, active, &req, &resp)?;       // new layer + mask + metadata
            Ok((id, resp.seed))
        },
        move |(id, seed)| json!({"layer": id, "seed": seed}))
}
```

`edit_job` gives exactly one undo step, a locked document while the job runs, "the document changed
while it ran" protection, and inline execution for tests, the CLI and wasm. With `"wait": true`
(the CLI's default through `wait_job`) the caller receives the final result instead of a job id.

Variations: `variations > 1` builds one request with a batch size (where the template supports it)
or N sequential requests with derived seeds, all inside one job and one undo step; the results land
as hidden siblings in a group named after the prompt, first one visible. The UI's variation strip
toggles visibility (`layer.visible` commands), so picking a variation is itself undoable and
scriptable.

Metadata: `LayerContent::Raster` layers gain an optional `generative: Option<GenerativeInfo>` on
`Layer` (`crates/doc`), serialised with `#[serde(default)]`; `crates/format` fails to compile until
the manifest and `convert.rs` carry it (`AGENTS.md` §8, by design), and `io` writes it to PSD as a
private tagged block kept verbatim on round trip. `GenerativeInfo { prompt, negative, model,
seed, steps, guidance, template, server_version, created }`.

Tests (next to the module, like every `*_cmds.rs`): param validation (bad types, out-of-range
variations, missing prompt → `Err`), disabled states (no selection, no backend), a fake backend
implementing `GenerativeBackend` that returns a checkerboard so the layer/mask geometry is checked
at 8/16/32-bit, undo/redo, cancellation mid-run, `panic_hunt` coverage. The `comfy` client gets a
fake server in `crates/genai/tests` (a `TcpListener` answering the five routes with canned JSON and
a 1×1 PNG) and one `#[ignore]` live test gated on `PHOTOCRAFT_COMFY_URL`.

## 4. Preferences

Upstream's preferences are typed serde sections collected in `Preferences`
(`crates/engine/src/prefs.rs:744-784`), exposed over MCP and the CLI through `prefs.get/set` and
`edit.preferences.<section>`, and audited by the scorecard ("settings that do nothing": a
preference no code reads is a scorecard failure, `docs/development.md` › Scorecard). Upstream
already has the section we need: **Edit › Preferences › AI Integrations…** →
`edit.preferences.integrations` (`crates/ui-egui/src/menu_catalog.rs:151`), backed by
`Integrations` (today `allow_agent_control`, `control_port`). The ComfyUI settings go there, so
no new section, menu row or dialog page is needed:

| Preference (`integrations.*`) | Default | Read by | Status |
|---|---|---|---|
| `comfyServer` | `http://127.0.0.1:8188` | `generate_cmds::backend` | **Phase 1, implemented** |
| `defaultEditModel` | `""` (the template's default file) | `generate_cmds::plan_fill` | **Phase 1, implemented** |
| `generativeTimeoutSecs` | 600 | `generate_cmds::backend` → the backend's deadline | **Phase 1, implemented** |
| `allowResearchModels` | false | `plan_fill` refuses research-only templates unless on; `generate.models` reports `allowed` | **Phase 1, implemented** |
| `defaultGenerateModel` | `""` (the template's default file) | `generate_cmds::plan_image` | **Phase 1, implemented** |
| `defaultFillTemplate` / `defaultImageTemplate` | `auto` / `krea2-turbo/image` | `plan_fill` / `plan_image` (a research template here still needs `allowResearchModels`); `auto` is resolved inside the job from the server's model lists (`generate_cmds::AUTO_FILL_ORDER`: the Lightning 8-step tier when its LoRA is installed, else the 40-step base) | **Phase 1, implemented; `auto` 2026-10-09** |
| `generativePreviews` | true | task bar overlay | Phase 2 |
| `freeVramAfterRun` | false | client (`POST /free`) | Phase 2 |
| `generativeContentFilter` | true | the moderation hook required by community licences | Phase 2 |

Each one is read by code in the same change that adds it, so the audit count does not move
(`crates/engine/tests/prefs_usage.rs` enforces it). Validation lives in `prefs::check_value`
(`comfyServer` must be an `http(s)://` URL or empty; `defaultEditModel` a plain file name) and
`prefs::range` (`generativeTimeoutSecs` 5..86 400).

## 5. UI (Phase 2)

- **Menu entries** in `crates/ui-egui/src/menu_catalog.rs` / `menus.rs` under Edit (Generative
  Fill…, Generative Expand…, Generate Image…, Generative Edit…) and Layer › Generative. They are
  additions beyond Photoshop's catalogue row set (upstream left the Generative Fill row out), so
  `cargo xtask parity` stays unaffected; a short note goes in the menu catalogue comments.
- **Generative task bar**: a `widgets::*`-based bar shown while a selection exists, with the
  prompt field, model picker, Generate, Cancel, and after a run the variation strip. State lives
  in `state.rs` (serde) so `ui.inspect` reads it and `ui.set` drives it; strings go through `tl!`
  and the i18n coverage test.
- **Progress**: the existing job progress UI (fed by `JobInfo.progress/message`) shows ComfyUI's
  step count; Esc cancels (upstream's convention, `jobs.rs:578`). Preview frames (binary WebSocket
  messages) are drawn as a dimmed overlay inside the selection when `integrations.generativePreviews` is on.
- **Dialogs**: `generate.image` and `generate.expand` get schema-driven dialogs
  (`filter_dialog.rs` / `dialogs.rs` pattern) because they have size parameters; Fill and Edit use
  the task bar only, like Photoshop.
- **Preferences › Generative** page: server URL with a "Test connection" button (runs
  `generate.health`), default model, timeouts, previews, research-model opt-in, content filter.

## 6. Headless and agents

Nothing extra is needed: `photocraft-cli run … --cmd generate.fill --params '{…}'` and MCP
`command_run` dispatch the same specs; the CLI already waits on jobs. `generate.models` and
`generate.health` are queries so an agent can discover what is installed before asking for a fill.
The ten-task MCP acceptance test (`crates/automation/tests/agent_tasks.rs`) gains generative tasks
against the fake backend.

## 7. Security and privacy

- The server is loopback by default and PhotoCraft refuses non-loopback URLs unless the preference
  is explicitly set; it warns once that ComfyUI has no authentication.
- No telemetry, no cloud fallback. Prompts and pixels never leave the machine unless the user
  points the server URL elsewhere.
- Uploaded crops are named per job and overwritten; a "Clear server inputs" action deletes them
  (`/userdata` is not involved; inputs live in ComfyUI's `input/` folder, documented for the user).
- Community-licence models that require moderation get the `integrations.generativeContentFilter` hook: the
  template can include the model vendor's recommended filter node when one exists, and the UI
  shows the licence conditions once per session.

## 8. What stays untouched

The compositors, the document model's pixel paths, PSD I/O (apart from one passthrough block), the
GPU canvas, the plugin sandbox and the automation transports. The design adds a crate, a command
module, a preference group and UI widgets; the rest of PhotoCraft does not know generation exists,
which is also what makes the eventual upstream PR small.
