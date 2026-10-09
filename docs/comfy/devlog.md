# Comfy PhotoCraft dev log

Newest entry first. Terse: what landed, numbers, what is still open. Upstream keeps its log in
the gitignored `log/devlog.md`; this one is tracked so the next session (any machine, any agent)
can pick up.

## 2026-10-09 (late night): Phase 2, the generative task bar and variations

**Landed**

- `generate.fill` takes `variations` (1..4): one backend run per variation with consecutive
  seeds, each its own masked layer, only the first visible, all in one undo step; the result
  lists `layers` and `seeds`, and the job's progress spans the runs ("Variation 2/3").
  `generate.variation {layers, index}` shows one of them, hides the others and activates it, as
  one undo step. `generate.models {probe: false}` lists the templates without touching the server.
- `crates/ui-egui/src/generative_bar.rs`: the task bar under the selection. Prompt (Enter
  generates), template picker (research templates only with the preference on), variations,
  Generate; a progress bar with Cancel while a fill runs on the document (its own or one a script
  started); a ‹ 1/3 › switcher after a multi-variation run; × hides it until the next selection.
  It sits centred below the selection, above it when there is no room, always on the canvas.
  Edit › Generative Fill… and the selection context menu open it; the generated dialog remains
  the fallback when the new `integrations.showGenerativeBar` preference is off or for scripts
  that pass params. `ui.set` gained `generativeBar`, `generativePrompt`, `generativeTemplate`,
  `generativeVariations`; `ui.inspect` reports `generativeBar`; the modal progress dialog stays
  away while the bar shows the job (`docs/control-protocol.md` updated).
- Tests: two engine tests (variations, the variation command and its validation), four UI tests
  (menu → bar → generate → switch against the fake server; the preference's dialog fallback; the
  bar's placement in a kittest harness; the placement arithmetic); six strings and one
  preference label translated in the 13 languages.
- Live: a two-variation Qwen-Image-2.1 fill from the bar on the lighthouse (a boat on the water),
  captured as `ui-genbar-idle/running/results.png` in `C:\Users\5090\ComfyUI\photocraft-tests`.

**Findings for the performance pass (next)**

- ComfyUI's official 2511 template chains UNETLoader → ModelSamplingAuraFlow 3.1 → CFGNorm 1.0 →
  (Lightning LoRA, 4 steps, CFG 1 | nothing, 40 steps, CFG 4) → KSampler euler/simple, with
  `FluxKontextMultiReferenceLatentMethod index_timestep_zero` on both conditionings. Our 2511
  fill lacks CFGNorm and the reference-method nodes. The 2511 Lightning LoRAs (lightx2v) are
  Apache-2.0: 850 MB bf16 files for 4 and 8 steps.
- Qwen-Image-2.1's few-step LoRA (Viggle turbo, 6 steps) carries the same research licence as
  the base model and needs the author's custom nodes (a sigma schedule and an unmerged LoRA
  loader); not a plain `LoraLoaderModelOnly`. `QwenImage21Cache` (already in our templates) is
  the other 2.1 speed lever.
- ComfyUI flags worth measuring on the 5090: `--fast fp16_accumulation` (ships in Comfy's own
  bat file; changes outputs slightly), `--highvram` (keeps models resident). SageAttention needs
  a wheel matching torch 2.14 / cu130 / Python 3.13 and is reported to break Qwen when enabled
  globally; left out.
- Photoshop's Generative Fill renders at most 1024 px on the longer side and upsamples; our
  2511 path does the same through `FluxKontextImageScale`, the 2.1 path keeps the input size.

**Still open (Phase 2)**: generative layer metadata, a committed control-protocol script test,
installed-model badges in the picker.

## 2026-10-09 (night): Phase 2, first slice: the generative commands are clickable

**Landed**

- Menu rows (fork additions in `menu_catalog.rs`, not parity rows): **Edit › Generative Fill…**
  and **Edit › Generate Image…** right after Content-Aware Fill, **Select › Select by Text…** (the
  command's own `menu`), and **Generative Fill…** first in the selection context menu
  (`canvas_tool_menu.rs`), where Photoshop puts it. Translated in the 13 complete-menu languages;
  `docs/parity.md` now counts 629 live items (the two Edit rows).
- Generated dialogs: the three commands are on `filter_dialog::has_dialog`'s allow-list (no
  preview). Their params docs were rewritten in the dialog notation, with 0 meaning "the
  template's default" for steps/guidance and "the document's size" for width/height, and
  `target: "auto|layer|document"`; the engine accepts those values from the CLI too (tests
  updated). OK runs the usual background job (`jobs_ui`: status-bar progress, Esc cancels), so
  the dialog itself needed no new UI code.
- Two fixes in upstream's `filter_dialog.rs`, both candidates for an upstream PR: `parse_spec`
  parses only the parameter object (a `:` in the notes after `→` leaked in as an `Int` field,
  visible as a stray `Ms"} (prompt` row), and ranges spanning ≤ 10 are stored with the two
  decimals the field shows (margin 0.25 was snapped to 0.3). Unit test added.
- Preferences › AI Integrations had been failing two ui-egui tests since Phase 1: the generated
  labels of the seven settings had no translations, and a test assumed the section was entirely
  unimplemented. Labels in 13 languages, `humanize` overrides ("ComfyUI server", "Generative
  timeout (seconds)"), the test now asserts the section is visible while agent control stays
  hidden.
- Visual check with the offscreen snapshot example, captures in
  `C:\Users\5090\ComfyUI\photocraft-tests\ui-*.png`: the three dialogs, the Edit and Select menus,
  the selection context menu, the Preferences page in English and German.

**Findings**

- The snapshot example panics on this PC's DX12 adapter inside egui-wgpu's staging belt
  (`renderer.rs:984`, the same failure as the 7 baseline canvas tests). `$env:WGPU_BACKEND =
  "vulkan"` works, and so does `WGPU_FORCE_FALLBACK_ADAPTER=1`; written into
  `dev-environment-windows.md` with the PowerShell quoting for `--script`.
- `input_tests::eyedropper_and_alt_sampling_show_a_pipette` fails the same way with
  `PHOTOCRAFT_CONFIG_DIR` pointing at an empty folder, so it is not reading machine preferences.
  Still unexplained, still in the baseline.
- The generated dialog is a stopgap: "0" for width/height/steps cannot explain itself, and there
  is no model picker. The task bar (Phase 2 item 2) replaces it.

**Numbers**: ui-egui lib 916 passed / 1 baseline failure; engine 859 + the generate/select
command tests; genai 29; i18n 1467/1467 in every language; parity 629/629; scorecard unchanged
(57 unread settings of 146); clippy, layers, wasm green.

**Still open (Phase 2)**: the task bar anchored to the selection, variations, progress preview,
a model picker fed by `generate.models`, generative layer metadata, and a committed
control-protocol script that selects, fills and screenshots.

## 2026-10-09 (later): Phase 3.5, Select by Text with SAM 3.1, live

**Landed**

- Template `sam3.1/segment` (community licence: the SAM License) from ComfyUI's official
  "Image Segment (SAM3)" template: `CheckpointLoaderSimple` (MODEL + CLIP from the 1.75 GB
  `sam3.1_multiplex_fp16.safetensors`), `CLIPTextEncode` on that CLIP, `SAM3_Detect`
  (threshold, 2 refinement passes, `individual_masks` on), `MaskToImage` → `SaveImage`, so the
  server answers with one PNG per instance. The server's `object_info` confirmed the node set.
- `Request.params` (template-specific placeholder values such as `threshold`) and
  `Error::NoOutput` (a detector that finds nothing) in the genai crate; the fake server answers
  `SAM3_Detect` graphs with one mask per configured rectangle.
- `crates/engine/src/select_ml_cmds.rs`: `select.byText` (phrase → instances → coverage →
  `sel::combine` with `replace|add|subtract|intersect`, `instance` picks one, `threshold`,
  `sampleAllLayers`; a background job and one undo step; reports each instance's bounds and
  pixel count) and `select.subjectML` (`what`: subject, person, face, hair, sky, animal, vehicle,
  text → a fixed phrase). Shared plumbing with `generate_cmds` (`plan_common`, backend, job
  progress, resampling), now `pub(crate)`.
- 7 command tests and 3 crate tests against the fake; clippy, wasm gate green.

**Live, on the 1024² lighthouse from the morning** (`photocraft-cli run … --cmd select.byText`):

| Phrase | Found | Bounds `[x, y, w, h]` | Pixels | Time |
|---|---|---|---|---|
| "the lighthouse" | 1 | `[695, 322, 111, 287]` (the tower, exactly) | 17 555 | 4.6 s, cold checkpoint load |
| "sky" | 1 | `[0, 0, 1024, 612]` (down to the horizon) | 601 550 | 1.8 s, warm |

Then `generate.fill` with `qwen-2.1/fill` on the lighthouse selection ("bold red and white
horizontal stripes", margin 0.4 → a 341×517 request) took 8.2 s and repainted only the tower
(`sam-lighthouse.psd/.png` in `C:\Users\5090\ComfyUI\photocraft-tests`). Select → describe →
generate, with no lasso, is now a two-command script.

**Still open**

- Point and box prompts (`select.byPoint`, the Object Selection tool's ML mode): `SAM3_Detect`
  takes `positive_coords`/`negative_coords` strings and `bboxes`; the coordinate string format
  needs reading from the node source before wiring.
- `panic_hunt` on a machine with ComfyUI running at the default URL will make real calls for
  `select.subjectML {}` (every other command fails validation first); run the hunt with the
  server stopped, as CI effectively does.
- Soft mattes for hair/glass (BiRefNet refinement) remain Phase 3.5's second half.

## 2026-10-09: first live runs against a real ComfyUI (Phase 1 DoD met)

**Setup on the dev PC** (nothing of this is in the repo; the app ships no weights, BYO models):
ComfyUI v0.39.0 Windows portable (NVIDIA build, 2.0 GB) at `C:\Users\5090\ComfyUI\ComfyUI_windows_portable`,
embedded Python 3.13.14, torch 2.14.0+cu130, RTX 5090 recognised. Start script
`C:\Users\5090\ComfyUI\start-comfyui.ps1` (loopback :8188, `--preview-method auto`). Model files
(Hugging Face, ~100 MB/s): Qwen-Image-2.1 int8 set 16.1 GB, Krea 2 Turbo fp8 set 17.4 GB,
Qwen-Image-Edit-2511 fp8mixed 19.1 GB + its 8.7 GB encoder (VAE shared with Krea 2). The server
listed the files without a restart.

**Findings that changed code**

- `TextEncodeQwenImage21`'s `latent` output is an **empty** latent sized to image 1, not an
  encoding of it (checked in `comfy_extras/nodes_qwen.py`): 2.1 edits in context and regenerates
  the whole picture, so `SetLatentNoiseMask` cannot inpaint with it. `qwen-2.1/fill` now sends
  the selection mask as a second reference image and wraps the prompt in a local-edit instruction
  (`meta.promptFormat`, new); PhotoCraft's layer mask confines the visible change. Result: correct.
- Grouped ("autogrow") inputs are spelled `images.image_1` in the API format (the server prefixes
  the group id; the first attempt with `image_1` failed with "unexpected keyword argument").
- `/object_info` on 0.39.0 has every class the four templates use; `CLIPLoader` offers both
  `qwen_image` and `krea2`.

**Numbers** (1024² text-to-image; fill = 384×320 selection, 576×512 request; `ms` is the
command's own timing, which includes upload, queue, sampling and download):

| Run | Template | Time | Notes |
|---|---|---|---|
| Text to image, cold | `qwen-2.1/image` | 28.4 s | model + encoder load from disk included |
| Text to image, warm | `qwen-2.1/image` | 10.8 s | 25 steps |
| Generative Fill, warm | `qwen-2.1/fill` | 4.7 s | boat placed exactly in the selection, seamless |
| Text to image, cold | `krea2-turbo/image` | 9.8 s | 8 steps; includes the 12 GB load |
| Generative Fill, cold | `qwen-edit-2511/fill` | 124.2 s | 40 steps + 19 GB load; see result note below |

Result note: the selection covered water on its left and rocks on its right. 2.1 (mask as a
reference image + instruction) placed a small boat on the water, matching the scene's light and
scale, with no visible seam. 2511 (latent noise mask) painted a larger boat on the rocks, inside
the selection but less plausible; worth retrying with the prompt describing the whole scene and
with the official template's `CFGNorm` node. Both results stayed inside the mask, as designed.
Krea 2 Turbo's text-to-image (8 steps) produced a different, photographic composition at the
same seed; Qwen-Image-2.1's fox test image is near photo quality.

VRAM after the 2.1 runs: 20.6 GB in use of 32 GB (ComfyUI keeps models resident); after all
three families had run: 21 GB (it swaps between them).

Outputs in `C:\Users\5090\ComfyUI\photocraft-tests\` (`q21-image.png`, `q21-fill.psd/.png`,
`krea2-image.png`, `q2511-fill.psd/.png`, `q21-fox.png`); the PSDs carry the masked
"Generative Fill: …" layer above the Background. Headless invocation, from the repo root:

```powershell
.\target\release\photocraft-cli.exe --% run in.png --cmd prefs.set --params "{\"values\":{\"integrations.allowResearchModels\":true}}" --cmd select.rect --params "{\"x\":320,\"y\":560,\"width\":384,\"height\":320}" --cmd generate.fill --params "{\"prompt\":\"a small red wooden rowing boat floating on the water\",\"template\":\"qwen-2.1/fill\",\"seed\":7}" --out out.psd
```

(PowerShell: one `--%` command per line; everything after the token is passed verbatim.)

**Still open**

- Judge the 2511 fill quality against 2.1 on more images; consider the official template's
  `CFGNorm` node and the Lightning 4-step LoRA for 2511.
- Variations, the task bar and menu rows (Phase 2); `generate.edit` / `expand` /
  `removeBackground` (Phase 3); `select.byText` with SAM 3.1 (Phase 3.5).
- The ComfyUI server was left running after the session (`Stop-Process -Name python` or close it
  from the start script's window to free 21 GB of VRAM).

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
- Qwen-Image-2.1 templates (`qwen-2.1/fill`, `qwen-2.1/image`), from ComfyUI's official 2.1
  image-edit and text-to-image templates: `UNETLoader → QwenImage21Cache`, `CLIPLoader` type
  `qwen_image`, `TextEncodeQwenImage21` (positive, negative and the encoded latent of `image_1`,
  resolution 0 keeps the input size), KSampler euler/simple 25 steps CFG 1, the 2.1-specific VAE
  `qwen_image_2.1_vae_bf16`. Fill masks the encoder's latent with `SetLatentNoiseMask`. Both are
  **research-licence** templates: refused with a message until `allowResearchModels` is on, and
  listed as `allowed: false` by `generate.models` meanwhile.
- Preferences › AI Integrations: `comfyServer`, `defaultEditModel`, `defaultGenerateModel`,
  `defaultFillTemplate` (`qwen-edit-2511/fill`), `defaultImageTemplate` (`krea2-turbo/image`),
  `generativeTimeoutSecs`, `allowResearchModels`, each read by the commands and validated. Setting
  `defaultFillTemplate = qwen-2.1/fill` with the research opt-in makes 2.1 the everyday editor.
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
