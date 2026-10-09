# Comfy PhotoCraft dev log

Newest entry first. Terse: what landed, numbers, what is still open. Upstream keeps its log in
the gitignored `log/devlog.md`; this one is tracked so the next session (any machine, any agent)
can pick up.

## 2026-10-09 (late): the leftovers of Phases 2 and 3

**Landed**

- Generative layer metadata (Phase 2 item 5): every layer a generative command makes carries a
  `GenerativeInfo` (command, prompt as typed, resolved template, seed, steps, guidance, edge,
  request rect, name; images add size and transparent) as JSON in a PSD additional-layer-info
  block keyed `cpGn`, so upstream's `Layer` is untouched and the record survives PSD round
  trips and `Layer::duplicate`. `generate.info` reads it.
- `generate.similar` (Edit › Generate Similar, translated ×13, enabled on a generative layer):
  runs what made the layer again with a new seed in the same place, the layer's mask as the
  area (no selection needed), the result a layer above it. Fills and edits re-run as they were
  (the same resolved template); an expanded area is filled again with the expand's prompt
  through the fill templates (its canvas already exists); a generated image is generated
  again as a layer. Research gate as usual.
- The picker's installed badges: `generate.models` takes `async: true` and probes the server
  on a worker (`jobs::run`, result = the usual answer); the bar starts it when it loads its
  templates, keeps the job id, and marks templates whose files the server lacks "(not
  installed)" (translated). The bar never waits for the server.
- The Crop tool's expand state (Phase 3): a Generative Expand checkbox in the Crop options bar
  (`ToolOptions::crop_generative`); on ↵ with a frame beyond the canvas, `commit_crop` crops to
  the part inside first (when that is not the whole canvas) and then runs `generate.expand`
  with the overhang as pads, so the model paints the new area; an empty prompt continues the
  scene. Off, the canvas just grows as before.
- The control-protocol script test of the Phase 2 DoD already existed as
  `generative_bar_tests::the_menu_opens_the_bar_and_generate_runs_the_fill_with_variations`
  (select → `ui.set` → generate → `ui.inspect` against the fake); the roadmap now says so.
- Tests: two engine tests (metadata + similar for fills, duplication; images and expands), the
  crop test (beyond, partly inside, option off), the bar test checks the badges.

**Open**: preview frames in the bar; a permissive matting default; Qwen-Image-Layered.

## 2026-10-09 (late, Phase 3): Generative Edit, and the automatic purge on a model switch

**Landed**

- `generate.edit` (Edit › Generative Edit…, translated ×13, generated dialog): the whole
  composite goes to the official 2511 edit graph as the model is meant to be used, no noise
  mask, the instruction decides what changes (`qwen-edit-2511/edit-lightning-8` and `edit`,
  `auto` between them, `AUTO_EDIT_ORDER`, `generate.models` reports `autoEdit`). The result
  is a layer above the active one, masked to the selection when there is one (soft dithered
  edge or hard), so "turn the red boat blue" with a marquee on the boat changes the boat and
  nothing else on the canvas. Descriptions are wrapped as "Change this image so that it shows
  {prompt}, keeping everything else exactly as it is."; instructions get ". Keep everything
  else exactly as it is." Shares `fill_region` (variations, sizing, Lanczos, timings); the
  request carries no mask when the template takes none.
- `run_switching`: every generative run (fill, expand, edit, image, matte, select by text)
  goes through it. The engine remembers the model files (`folder/file`) of the last run per
  server URL (a process-wide static) and asks the server to unload its models before a run
  whose files differ: the Lightning run after the 40-step base went from 121 s to 20.2 s
  (`benchmarks.md` §5c). Never on a server's first run; a server that cannot purge runs as
  before. A test drives Lightning → base → base → Lightning against the fake and counts the
  `POST /free`s (1, then 2) and their position.
- Corrected a diagnosis: the 40-step base's 130–148 s is its own cost (two passes a step at
  CFG 4), not thrash; the thrash was the Lightning tier after it.
- Four engine tests for edit (whole canvas, no mask upload, wrapping; the selection confines
  the result; `auto` falls back to the base, validation, `autoEdit`), one for the purge.

**Live** (`bench/edit-*.png`): a dark storm with lightning and the lamp lit at 16 s; the boat
turned blue inside its selection and nothing else; a sunny day at 20 s after a switch.

**Open**: the Crop tool's expand state; a permissive matting default; Qwen-Image-Layered;
a 20-step middle tier for the base.

## 2026-10-09 (late, Phase 3): Remove Background on Qwen-Image-2.1's alpha channel

The user: "qwen2.1 supports alpha channels". It does, natively: the 2.1 VAE decodes RGBA.
Probed live before writing code (`bench/probe-rgba-s7.png`, `probe-t2i-alpha-s3.png`):
ComfyUI's official background-removal recipe returned a colour-type-6 PNG with a soft matte in
18 s, and a plain text-to-image prompt ending in "isolated on a transparent background, output
a PNG image" returned a boat with a clean matte in 10 s.

**Landed**

- `generate.removeBackground` (Edit › Remove Background (Generative)…, translated ×13, generated
  dialog): the layer (or the composite with `sampleAllLayers`) goes to `qwen-2.1/matte`, the
  official recipe as a template (the 2.1 edit graph, the picture as image_1 at its own size,
  "Remove the background, and output a PNG image"; `prompt` names what to keep and becomes
  "Remove the background, keeping only {prompt}, …"). The result's **alpha** comes back as the
  layer's mask (the Background becomes a normal layer, like the Quick Action) or, with
  `asSelection`, as the selection combined by `mode`. The pixels the model re-rendered are
  never used; the layer keeps its own. Requests go out at ≤ 1 MP with sides in multiples of 32
  (what `TextEncodeQwenImage21` rounds to), the matte comes back through the tent filter, the
  layer's own transparency goes to the model over mid-grey and multiplies the matte. One undo
  step; "kept nothing" is an error that changes nothing. Research licence gate as for the other
  2.1 templates.
- `generate.image` takes `"transparent": true`: the template's new `promptFormatTransparent`
  wraps the prompt (only `qwen-2.1/image` has one; others refuse with a clear error) and the
  layer keeps the alpha the PNG comes back with.
- Fake server: `Options.matte` gives generated images an alpha rectangle. Five engine tests
  (mask from alpha with pixels untouched and undo, subject wrapping + Background → layer,
  as-selection + add mode, validation + licence gate + "kept nothing", transparent image +
  refusal). Docs: models.md, architecture.md, roadmap; parity/scorecard regenerated.

**Live** (the 1024² lighthouse with the boat, `bench/matte-*.png`, `transparent-compass.png`):
16.8 s per matte (25 steps, 2.1 int8). The model's own reading kept the boat solid and the
lighthouse as a ghost; "the red boat" gave a perfect boat cutout (147×46 px); "the lighthouse"
kept the lighthouse with its rocks, the white tower semi-transparent against the bright sky
(a model quirk to note in the UI later: name the subject, re-roll, or refine the mask). A
transparent "vintage brass compass, product photo" came back in 10.8 s as a clean cutout with a
real soft alpha.

**Open**: a permissive matting model (BiRefNet) as the default so the command works without
the research opt-in; Qwen-Image-Layered ("image to layers": ComfyUI 0.39 ships
`EmptyQwenImageLayeredLatentImage` and official templates; needs `qwen_image_layered_bf16` +
its VAE, not installed) as a Layers › "Split into Layers" feature; transparent fills (the
model's alpha times the selection) for adding cut-out objects.

## 2026-10-09 (late): soft, dithered edges for fills and expands

The user: "the borders are harsh, is there a way to do like a noise opacity in the edges so
that there aren't hard cutoffs?" Yes; it took three changes, each found by measuring the
right-side expand's sky profile (`benchmarks.md` §3, "Edges").

**Landed**

- `generate.fill` and `generate.expand` take `"edge":"soft|hard"` (default soft; a choice in
  the generated dialogs, labels already translated). Soft: the result layer's mask is the
  feathered request band itself, its ramp **dithered** with per-pixel noise (`dither_edge`: a
  splitmix64 hash of pixel index and seed, amplitude `0.6 × (1 − |2m − 1|)`, so nothing changes
  inside or outside the band and the grain peaks mid-ramp). The result fades into its
  surroundings as grain, not a gradient or a line. Hard: exactly the selection (or the added
  canvas). The feather is now 4 % of the longer side for both (fill 6–48 px, was 2 %; expand
  8–80 px, was 8 %: at 8 % the model moved the horizon inside the band at one seed, a ghosted
  double horizon; at 2 % the old edge still read as a line).
- `feather_outward` starts its ramp at **full** coverage on the edge and falls to nothing at
  twice the radius (the GrowMask + FeatherMask shape). The old `max(selection, blur)` ramp
  started at half coverage: a 0.5 step at the edge in both the noise mask and the layer mask,
  which is half the tone difference left standing as a line.
- Expand's padding is pre-filled by replicating the picture's edge pixels instead of mid-grey:
  the grey never reached the model as content (cropped reference, full noise mask) but the VAE
  carried its tone into the latents of the edge band, a dark line right on the old edge
  (luminance 213 between 225 and 228). `PREFILL` grey remains only for an empty picture.
- Tests: the soft-edge test (full on the edge pixel, high just outside, falling off, grain in
  the row, hard = selection, `edge: fuzzy` rejected), `dither_edge` unit test, the prefill test
  now checks edge continuity, expectations for the new radii and ramp.

**Live** (`bench/edge-s11-*.png`, `bench/expand-right-{soft,hard}.png`, `xab*-*.png`): the
expand's sky goes 229 → 226 across the band with no step or dip, nothing visible in a
contrast-stretched crop; the hard variant keeps its step. The soft and hard variants of one
seed are the same request, so the second returns from ComfyUI's cache in 0.1 s. The three
placement cases (boat ×2, balloon) are unchanged by the wider feather. Back-to-back expands
run 13–20 s (13 s with the reference's encoding cached, 19 s after another reference evicted
it).

**Found on the way** (`benchmarks.md` §3): what no edge can hide is the new side being a
different photograph. With a described prompt the Lightning model painted a crisp, sharp-
horizon sea next to the hazy original at two seeds; the empty prompt (the default instruction)
continued the haze; the 40-step base (130 s) and several wordings copied the lighthouse into
the new area at one seed, and so did the empty prompt at the same seed when re-run later. The
server is bit-deterministic in a steady state and not across model-load states (pixel diffs
0.00 on repeats, up to 21/765 between states: a second lighthouse against none), so two-seed
wording comparisons prove little; the wrappers stay as they are and the dialog's note that an
empty prompt continues the scene is the advice. Also: the first run after a graph switch
(base ↔ Lightning, or purge → fill → expand) took 31–131 s with the server at 2.4 GB free
(partial model load), then 13–20 s; the engine should purge before a run that changes the
model set (roadmap, next).

**Open**: the automatic purge on a model-set change; a prompt study for Expand over more seeds;
the Crop tool's expand state; `generate.edit`, `generate.removeBackground`.

## 2026-10-09 (night, Phase 3): Generative Expand

**Landed**

- `generate.expand` (Edit › Generative Expand…, translated ×13, generated dialog): adds canvas
  (`left/top/right/bottom` px, or a larger `width/height` with Canvas Size's `anchor`) and has
  the model paint it, as **one undo step**: `translate_doc` + the new size inside the job, the
  picture moves by the left/top pads, the selection and vector geometry move along. The request
  is the bounding box of the added area (an L or a frame when several sides grow) plus the fill
  margin of the picture. An empty prompt becomes "extend the scene beyond its original edges,
  continuing it naturally". Result adds `canvas` and `offset`.
- Its own templates, `qwen-edit-2511/expand-lightning-8` and `expand` (task `expand`,
  `AUTO_EXPAND_ORDER`, `generate.models` reports `autoExpand`): the fill graph plus an
  `ImageCrop` so the text encoder's reference is **the picture alone** while the padded canvas is
  the sampling latent under the noise mask. Found the hard way: shown the padded canvas as its
  reference, the edit model reproduced the padding as content (replicated edge pixels came back
  as streaks; mid-grey came back as a grey frame at one seed). The empty canvas is still painted
  mid-grey before upload (`prefill_outside`), and the result layer's mask is the soft request
  mask with an 8 % feather into the picture (`outpaint_feather_radius`), so the re-rendered band
  carries the new area's tone across the old edge instead of meeting it at a line.
- The fill job body is now `fill_region` (shared by fill and expand): Lightning tiers, `auto`,
  grid alignment, the request cap and 512 px floor, the feathered request mask, variations. The
  cap went from 1 MP to **0.75 MP**: with 2511 fp8 and its 7.9 GB text encoder resident on the
  32 GB card, a 1 MP latent plus the 1 MP reference pushed ComfyUI into offloading part of the
  model (a 45 s run instead of 18 s).
- Tests: two engine tests (one-step L-shaped expand with pixel/mask/undo checks, the grey
  pre-fill and the crop rectangle; target size + anchor, selection follows, validation, disabled
  without a document).

**Live** (the 1024² lighthouse, Lightning 8, `bench/expand-*.png`): widened to the right by
384 px with "more open sea and evening sky" in 20.2 s (a 736×1024 request), and framed by 192 px
on three sides with the default prompt in 14.8 s (a 1408×1216 canvas sent at 944×816). Both
continue rocks, sea and the pink evening sky coherently; the old edges are faint at most. The
earlier variants (padded reference: streaks, a grey frame, a blue sky) are kept next to them with
suffixes for comparison.

## 2026-10-09 (performance pass): Lightning tiers, `auto`, request sizing, server flags

**Landed**

- `qwen-edit-2511/fill` now matches ComfyUI's official 2511 graph exactly (CFGNorm after
  ModelSamplingAuraFlow, `FluxKontextMultiReferenceLatentMethod index_timestep_zero` on both
  conditionings). Two Lightning tiers, `fill-lightning-8` and `fill-lightning-4` (Apache-2.0
  LoRAs by lightx2v, `LoraLoaderModelOnly` after CFGNorm, CFG 1), and the preference default
  `defaultFillTemplate = auto`: the job asks the server which files it has (`/models/<folder>`)
  and takes the first of `AUTO_FILL_ORDER` (`fill-lightning-8`, then the 40-step base) whose
  files are all installed. `generate.models` reports `autoFill`; the task bar's picker starts
  with Auto. BYOB stays: the 850 MB LoRA is a download the user makes.
- Request sizing: a fill request over 1 MP is sent downscaled (Lanczos-3 pixels, tent-filtered
  mask, multiples of 16) and the result is resampled back under the full-resolution layer mask;
  `resize_rgba8` is Lanczos-3 everywhere (was bilinear); segmentation requests are capped at 2 MP
  and their masks come back through the tent filter. `requestWidth/Height` and per-variation
  `timings` (`encodeMs`, `uploadMs`, `queueMs`, `runMs`, `downloadMs`) are in the result.
- Uploads: PNG at the fastest deflate level (loopback, read once) and **content-addressed
  names** (FNV-1a of the bytes), so ComfyUI's node cache keeps the loader, the 7 B vision text
  encoder and the VAE encode across variations and re-rolls of one selection.
- `docs/comfy/bench/bench-fill.ps1` (the live benchmark), `docs/comfy/benchmarks.md` (numbers),
  `start-comfyui-fast.ps1` on the dev PC with `--fast fp8_matrix_mult --highvram`, the flag table
  and a 24 GB / RTX 3090 section in `comfyui-setup.md`, Lightning facts in `models.md`.
- Tests: auto resolution with and without the LoRA on the fake (`Options.missing_files`),
  the 1 MP cap end to end (upload size, placement, timings), resampler behaviour, the 2 MP
  segmentation cap; 33 engine generate/select tests, genai 29.

- Prompt wrapping for the 2511 fills: a description ("a red boat") becomes "Add {prompt} to
  this image, fitting the scene's perspective, lighting and surroundings naturally. Change
  nothing else."; a prompt that already opens with an imperative verb (`template::is_imperative`,
  "remove the car") becomes "{prompt}. Fit the result…" (`promptFormatImperative`, also on the
  2.1 fill). The request mask is **feathered outward** (2 % of the longer side, 4–24 px, two box
  blurs then `max` with the original) so the latent noise mask blends the edge; the layer mask
  stays the selection. Opt-in `qwen-edit-2511/fill-guided` passes the mask as a second
  reference image for the best placement at 1.5–3× the time.

- **Native-size sampling** (second pass, same day): the 2511 templates VAE-encode the uploaded
  image instead of the Kontext-scaled one, so the sampling latent has the request's own size
  while the text encoder keeps its 1 MP reference. The engine grows the request rectangle to
  the 16-px grid (`align_to_grid`), caps it at 1 MP and floors it at 512 px on the longer side
  (`request_size`). 35–45 % faster and better placed (benchmarks §4). `generate.free`
  (Edit › Purge › Generative Models) asks the server to unload its models after a measured
  memory-pressure state made fills three times slower (benchmarks §5).

**Measured** (all tables and the method in `benchmarks.md`; the lighthouse, a 576×512 request):

| Fill, warm | default flags, unique uploads | `fp8_matrix_mult`, cached encoder (re-roll) |
|---|---|---|
| 2511 base, 40 steps CFG 4 | 162 s | 153 s |
| 2511 Lightning 8 | 18.6 s | 16.0 s |
| 2511 Lightning 4 | 12–16 s | 8.1 s |
| Qwen-Image-2.1, 25 steps | 4.5 s | 2.6 s |

A 2511 fill is sampling-bound (~2 s per Lightning step at 1 MP, ~4 s per base step with CFG 4);
the client's share is 2 ms encode, 5 ms upload, under 0.4 s download. With native-size sampling
the three A/B cases went from 16.2–16.5 s to 9.2–11.2 s, and a 120×90 selection fills in 10 s.

**Findings**

- Quality, three cases at fixed seeds (benchmarks §3): the plain prompt with a hard mask gave a
  cut-off boat with a rectangular seam at one seed; the shipped wrapper + feather gave no seam
  in any case and the boat on the water at seed 11; placement at seed 7 stays the model's
  choice (rocks) unless `fill-guided` or Qwen-Image-2.1 is used, both of which put it on the water.
- `--highvram` is harmful with 2511 fp8 on 32 GB (2 GB free, LoRA reload thrashed); dynamic
  VRAM already keeps a family resident. lightx2v's `fp8_e4m3fn_scaled` 2511 file ran 35 % faster
  but returned noise in the masked area with our graph; removed. `fp16_accumulation` does
  nothing for bf16/fp8/int8 models. Shipped flag: `--fast fp8_matrix_mult` (~5 %).
- Two benchmark runs overlapped on the GPU once (a background task kept running after I thought
  it had died); the numbers above come from clean, sequential foreground runs.
- PowerShell: variable names are case-insensitive (`$seed` is `$Seed`), `.NET`'s working
  directory is not PowerShell's (`[IO.File]` needs absolute paths), and `"$f:"` is a drive
  reference; all three bit the benchmark scripts once.

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
