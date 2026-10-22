# Generative benchmarks (comfy-photocraft)

Measured 2026-10-09 with [`bench/bench-fill.ps1`](bench/bench-fill.ps1) on the development PC:
RTX 5090 (32 GB, driver 617.42), ComfyUI 0.39.0 portable, Python 3.13.14, torch 2.14.0+cu130.
Same image (the 1024² lighthouse, `q21-image.png`), a 384×320 selection with a 25 % margin (a
576×512 request), the same prompt. `ms` is `generate.fill`'s own wall time; the breakdown comes
from the command's `timings`: `encode` (PNG), `upload`, `queue` (queued → the server starts),
`sampling` (the server's execution, model loads included), `download` (fetch + decode). All
numbers are milliseconds. The raw CSVs and the output PNGs sit in
`C:\Users\5090\ComfyUI\photocraft-tests\bench\` (not in the repository).

## 1. Fill templates, default server flags, no cache (run 1)

Uploads had unique names, so every run paid the 7 B vision text encoder and the VAE encode;
"warm" = same pixels and seed again. The 2511 base was already resident from earlier work, so
its "cold" is not a disk load.

| Fill | cold | warm | encode | upload | queue | download |
|---|---|---|---|---|---|---|
| 2511 base, 40 steps CFG 4 | 166 703 | 162 129 | 2 | 5–6 | 0–1 | 294–372 |
| 2511 Lightning 8 steps CFG 1 | 37 406 (LoRA load) | 18 597 | 2 | 5–6 | 0–1 | 362–366 |
| 2511 Lightning 4 steps CFG 1 | 12 529 | 16 525 | 2 | 6–21 | 0–1 | 355–366 |
| Qwen-Image-2.1, 25 steps CFG 1 | 8 563 | 4 504 | 2 | 4–6 | 0–1 | 7–229 |

Reading: Qwen-Image-Edit-2511 is a 20 B model working at its 1 MP size whatever the request
(`FluxKontextImageScale` upsamples a 576×512 request to ~1 MP), and CFG 4 means two passes per
step: ~4 s per step, so 40 steps is 2½ minutes. The Lightning LoRA (CFG 1, one pass) makes a
step ~2 s and 8 steps a 16–19 s fill. The 4-step tier's "warm" being slower than its "cold" is
the text-encoder cost landing differently between runs: the encoder is several seconds of every
run in this table. Qwen-Image-2.1 (7 B, CFG 1, KV prefix cache) does the same fill in 4.5 s.
The client's own share is negligible: 2 ms encode, 5 ms upload, under 0.4 s download.

## 2. Fill templates, `--fast fp8_matrix_mult`, content-addressed uploads (run 3)

Uploads are now named by content hash, so ComfyUI's node cache keeps the loader, the text
encoder and the VAE encode for the same pixels. "cold" = a selection shifted by 16 px (new
pixels, encoder runs; the family's model already resident); "warm" = the same pixels with the
next seed (the variation / re-roll case: only sampling runs).

| Fill | cold | warm | sampling (warm) |
|---|---|---|---|
| 2511 base, 40 steps CFG 4 | 158 935 | 153 331 | 153 055 |
| 2511 Lightning 8 steps | 19 284 | 15 960 | 15 913 |
| 2511 Lightning 4 steps | 11 664 | 8 115 | 8 068 |
| Qwen-Image-2.1, 25 steps | 8 027 | 2 614 | 2 576 |

Reading: `fp8_matrix_mult` is worth about 5 % on the `fp8mixed` 2511 file (159 s vs 167 s). The
content-addressed cache is worth the encoder's 3–5 s on every re-roll: a 2.1 variation is 2.6 s,
a 2511 Lightning-4 variation 8 s, a Lightning-8 one 16 s.

Not adopted, measured:

- `--highvram` with 2511 fp8 (20 GB) + its 8.7 GB encoder resident left 2 GB of 32 free; the
  40-step run got slower (192 s) and the Lightning LoRA run thrashed on reload. The two runs
  that measured it also overlapped (two benchmarks on one GPU), so only the direction is
  reliable; the plain dynamic-VRAM default keeps the models resident anyway while they fit.
- lightx2v's `qwen_image_edit_2511_fp8_e4m3fn_scaled.safetensors` (the FP8-tensor-core
  layout) was 35 % faster per fill (10.6–12.6 s for Lightning 8) but produced **noise** in the
  masked area with this graph under `fp8_matrix_mult`; removed again. Worth a second look with
  the ComfyUI-specific `_lightning_comfyui_4steps` file or without the flag.
- `--fast fp16_accumulation`: only speeds up fp16 models; ours run bf16, fp8 or int8.

## 3. Quality: placement and seams (Lightning 8, same seeds)

Three cases on the lighthouse: the water/rock edge with seeds 7 and 11 ("a small red wooden
rowing boat floating on the water") and the sky ("a hot air balloon drifting in the evening
sky"). Judged by eye on the output PNGs.

| Variant | seed 7 boat | seed 11 boat | sky | cost |
|---|---|---|---|---|
| plain prompt, hard mask | on the rocks | cut off, visible rectangular seam | good | 16–18 s |
| mask as `image2` + instruction (`fill-guided`) | on the water | on the water | good, flatter palette | 28–51 s (another 1 MP of reference tokens) |
| "Add {prompt} to this image…" wrapper | on the rocks | on the water, no seam | good | same as plain |
| "Edit this image: {prompt}…" wrapper | on the water | on the water | rectangular seam | same as plain |
| **shipped**: "Add…" / imperative wrapper + outward-feathered request mask | on the rocks | on the water, no seam | good, no seam | same as plain |

What shipped: every 2511 fill template wraps the prompt ("Add {prompt} to this image, fitting
the scene's perspective, lighting and surroundings naturally. Change nothing else."; a prompt
that already starts with an imperative verb gets "{prompt}. Fit the result…" instead), and the
request mask is feathered outward so the latent noise mask blends the edge. `fill-guided`
remains in the picker for the cases where placement matters more than time. Qwen-Image-2.1 put
the boat on the water at every seed tried; it is the research-licensed option.

**Edges (later the same day).** With the layer mask equal to the selection, the model's blended
band was cut off at the selection's edge: a hard border wherever the re-rendered pixels differed
from the original in tone (the user's "the borders are harsh"). Measured on the right-side
expand's sky (mean luminance per column, y 200–420): the original stepped from 225 to 228 at
the old edge, with a dark line of 213 right on it. Three changes, in the order they were found:

1. The layer mask is by default the soft request band itself, its ramp **dithered**: per-pixel
   noise (a splitmix64 hash of pixel index and seed), amplitude `0.6 × (1 − |2m − 1|)`, zero
   inside and outside the band and strongest mid-ramp, clamped to 0..1. The transition reads as
   grain, like film, not as a gradient or a line, and 8-bit exports cannot band. Fills feather
   4 % of the longer side (6–48 px, was 2 %), a band of twice that; expands the same 4 %
   (8–80 px, was 8 %): at 8 % the model took the licence to move the horizon inside the band
   (a ghosted double horizon at one seed), at 2 % the old edge still read as a line.
   `"edge":"hard"` restores the exact selection (or exactly the added canvas) for compositing
   work that wants it.
2. The feather ramp now starts at **full** coverage on the edge and falls to nothing at twice
   the radius (ComfyUI's GrowMask + FeatherMask shape). The first version, two box blurs
   `max`ed with the selection, started at half coverage: as the noise mask it kept half of the
   original latent one pixel outside the edge, and as a layer mask it stepped from 1 to 0.5,
   so half the tone difference stayed as a line.
3. Expand's empty canvas is pre-filled by replicating the picture's edge pixels, not mid-grey.
   The model never sees it as content (its reference is the cropped picture and the noise mask
   is full over it), but the VAE's latents beside a wall of grey carried its tone into the edge
   band: that was the dark line of 213.

After the three: 229 → 226 across 160 px, no step and no dip in the profile, nothing visible in
a contrast-stretched crop; the hard variant keeps its step (as asked). Server cost: none, the
request's size and graph are unchanged (the hard/soft variants of one seed hit ComfyUI's cache
and return in 0.1 s); the dither is one pass over the mask. The grey pre-fill was A/B'd again
with the full-coverage ramp (`xab-f08-grey-*`, `xab-f04-grey-*`): still a 2–3 level step right
on the edge at both seeds, so the edge replication stays.

**Consistency of the new side (the thing no edge can hide).** With the user's prompt "more
open sea and evening sky", seeds 6 and 8 painted a crisper, darker sea with a sharp horizon
next to the hazy original: a different photograph, visible through any blend. Same widen,
Lightning 8 unless noted (`bench/xab2-*.png`, `xab3-*.png`):

| Prompt → what the model got | seed 6 | seed 8 | time |
|---|---|---|---|
| "more open sea and evening sky" (the description wrapper) | crisp sea, sharp horizon | sharp horizon, higher than the original's | 13–15 s |
| the same through the **40-step base** (`qwen-edit-2511/expand`, CFG 4) | a second lighthouse in the new area | hazy, consistent | 130 s |
| "Add more open sea…, keeping exactly the same soft haze… as the original photo" | hazy, consistent | a second lighthouse | 14–16 s |
| **empty prompt** ("extend the scene beyond its original edges, continuing it naturally") | hazy, consistent | consistent, a little sharper | 14–18 s |
| "Extend this image…, continuing the scene naturally with more open sea… Match the original's … haze… and do not repeat its objects" | a second lighthouse | consistent | 13 s |
| "Extend this image to show more open sea… continuing the original photo naturally with the same haze, light and colours" | a stub of railing copied | consistent | 13–15 s |

Lessons: the empty prompt is the most faithful continuation, and it is what the dialog and the
docs recommend for "more of the same"; a description gives the model licence to restyle; the
40-step base is no more consistent and ten times slower. The shipped wrappers are unchanged.

**But the table is indicative at best.** Re-running the empty prompt at seed 6 in a steady
server state gave a second lighthouse (`final-expand-empty-s6.png`) where the run above had
none, and the two runs were bit-identical with each other on repeat. Pixel diffs of identical
requests run at different times: 0.00 (seed 8 widen, two repeats of the empty seed 6), 0.58
(the seed-11 fill), 2.19 and 4.53 (seed-6 widens), 21.44 of 765 (the empty seed 6, a second
lighthouse against none). So ComfyUI is deterministic in a steady state and not across
model-load states: the partially offloaded model after a graph switch (next paragraph)
computes slightly differently, and the 8-step Lightning sampler amplifies that into a
different composition at a knife-edge seed. Conclusions about wordings need many seeds in one
server state (a prompt study is listed under §6); what stands is that the model does, at some
seeds, repeat the picture's main object in the new area, whatever the wording.

**Graph switches thrash the server.** Every first Lightning run after the 40-step base, or
after a purge followed by a fill, took 31, 56, 93 and 121 s (2.3–2.5 GB free, torch 63 MB:
ComfyUI loads the newly patched model partially and streams the rest every step), then
13–20 s for the runs after it. (The base's own 130–148 s is not thrash: 40 steps at CFG 4 are
two model passes a step, about 3.6 s each at 0.75 MP against Lightning's single pass at
CFG 1.) **Fixed the same day** by the engine's `run_switching` (§5c): it remembers the model
files of the last run per server and purges before a run that changes them.

## 4. Native-size sampling (shipped)

The official 2511 graph encodes the `FluxKontextImageScale` output, so every request is sampled
at ~1 MP whatever its size. The shipped templates now VAE-encode the uploaded image itself: the
text encoder still sees the 1 MP reference, the sampling latent has the request's own size
(the engine grows the request rectangle to the 16-px grid, caps it at 0.75 MP, see §5, and
floors it at 512 px on the longer side). Same three cases, Lightning 8, same seeds:

| Case | 1 MP latent | native latent | result |
|---|---|---|---|
| edge, seed 7 | 16.2 s | 11.2 s | boat on the water (was on the rocks) |
| edge, seed 11 | 16.5 s | 9.2 s | boat on the water, no seam |
| sky, seed 7 | 16.4 s | 11.0 s | balloon, no seam |
| 120×90 selection (192×160 rect, sent at 512×416) | — | 10.4 s steady, 16.3 s with a model reload | a crisp seagull on the rock |
| 56×70 selection (96×112 rect, sent at 432×512) | — | 10.4 s | a brass porthole on the door |

35–45 % faster, and better: with the latent at the request's size the mask lands exactly
where the selection is, and both boat cases came out on the water.

## 5. Server memory pressure

After several checkpoints had been loaded in one server session (the 2511 base, its LoRA
tiers, Qwen-Image-2.1, the scaled fp8 file), the small-selection fills took 36 and 58 s with
2.5 GB of VRAM free: ComfyUI was staging the 19.6 GB model on every run. `POST /free`
(unload models) brought 30 GB back and the next fills took 16 s (reload included) and 10 s.
That is **Edit › Purge › Generative Models** (`generate.free`) in PhotoCraft. The steady state
with 2511 fp8 is ~1.6–2.5 GB free on a 32 GB card (UNET 19.6 GB + text encoder 7.9 GB + VAE).
In that state a 1 MP latent plus the 1 MP reference did not fit the headroom: Generative
Expand's frame case took 45 s (the log shows the 19.6 GB model re-staged for the run) against
18 s a moment earlier. The request cap is therefore 0.75 MP (768×1024), which keeps the model
resident and costs little: the editing models' own working size is about that.

## 5b. Remove Background and transparent generation (Qwen-Image-2.1, int8)

| Case | time | result |
|---|---|---|
| `generate.removeBackground`, 1024² lighthouse, no prompt | 16.8 s | boat solid, lighthouse a ghost (the model's reading of "the subject") |
| …, prompt "the red boat" | 16.7 s | a perfect 147×46 px boat cutout |
| …, prompt "the lighthouse" | 16.8 s | lighthouse with its rocks; the white tower semi-transparent against the bright sky |
| …, `asSelection` of the same request | 0.14 s | ComfyUI's cache |
| `generate.image` + `transparent`, "a vintage brass compass, product photo", 1024² | 10.8 s | a clean cutout with a real soft alpha |
| the first run after switching from the 2511 models (a purge first) | +~8 s | the 2.1 model and encoder load (6.8 + 8.7 GB) |

The matte is 25 steps of the full model, so it costs a text-to-image. The permissive route
(`template: auto` without the research opt-in) is SAM 3.1 instead, hard-edged but fast:

| Case | time | result |
|---|---|---|
| Remove Background, prompt "the red boat", SAM 3.1 | 1.5 s (checkpoint load included) | the boat, 146×38 px |
| Remove Background, no prompt ("the main subject"), SAM 3.1 | 0.5 s | the lighthouse tower, 111×281 px |
| `select.byPoint` at (350, 665) | 0.3 s | the boat |

A soft matte from a dedicated matting model (BiRefNet) would sit between the two in quality;
it needs a custom node pack the server does not have, so it is not planned for the default.
The detector route softens its mask with the classical edge refinement instead (`refine`).

## 5d. Split into Layers (Qwen-Image-Layered, fp8)

| Case | time | result |
|---|---|---|
| the 1024² lighthouse into 3 layers, 20 steps CFG 2.5 at 640² | 72 s (the 19 GB model loaded in it) | layer 0 a near-white base with the boat's shadow, layer 1 sky and sea with the boat, layer 2 rocks and lighthouse with real soft alpha |

CFG 2.5 means two passes a step, like the 2511 base; the official 640 px working size keeps it
to about a minute. The layers come back resampled to the canvas.

## 5c. Generative Edit and the automatic purge

`generate.edit` sends the whole 1024² lighthouse at 0.75 MP through the official 2511 edit
graph (no noise mask); the result is a layer, masked to the selection when there is one
(`bench/edit-*.png`):

| Case | time | result |
|---|---|---|
| "make the sky dark and stormy with heavy clouds", Lightning 8 | 16.0 s steady (22.6 s with the 2511 reload after a 2.1 run) | a storm with lightning, the lamp lit, the sea rough; the rocks and lighthouse kept |
| "turn the red boat blue" with a 200×120 selection on the boat | 16.1 s | only the boat changed on the canvas (the model re-rendered everything; the mask shows the boat) |
| "make it a sunny day with a blue sky" | 20.2 s after a model switch, 16 s steady | convincing |
| the same storm through the 40-step base (`qwen-edit-2511/edit`) | 145–148 s | two passes a step at CFG 4; the Lightning tier is the default for a reason |

**The purge.** Three edits in one process, Lightning → base → Lightning (`purge-live.ps1`):
16.0 s, 148.5 s (the base's own cost, purged first), **20.2 s** for the Lightning run after
the base. The same return to Lightning without a purge took 121 s earlier in the day. The
engine purges only when the model files change and never on a server's first run; separate
CLI invocations are separate processes, so the CLI only exercises it within one `--cmd` chain.

## 5e. Prompting: what the guides say, and the enhancer

The first user session (dev log, 2026-10-09 evening) turned a line drawing of a face into a plate
of food twice: "make this hyper realistic" gives Qwen-Image-Edit nothing to hold on to when the
picture is a doodle. What the model authors and ComfyUI recommend, checked 2026-10-09:

| Source | What it says | What we do |
|---|---|---|
| Qwen-Image-Edit model card and the `Qwen-Image` repo's `polish_edit_prompt` (Alibaba, 2025–2026) | run the instruction through a VLM that sees the picture and rewrites it: name the objects, one change per instruction, state what stays, quote literal text verbatim | `generate.enhancePrompt`: the same rewrite, on the Krea 2 text encoder (Qwen3-VL 4B) already on the server |
| ComfyUI's shipped Qwen-Image-2.1 "prompt enhancer" subgraph (`comfyui-workflow-templates` 0.11.76) | a 10 KB system prompt for an 8B VLM (`TextGenerate`, temperature 0.3, no thinking) | too long for the 4B model (it echoes or rambles); a 12-line prompt with the same rules works |
| ComfyUI's Krea 2 template | `TextGenerate` on the Krea 2 encoder expands a short idea into a paragraph before `CLIPTextEncode` | `enhance/image`: the same node, our short instruction, 60–120 words |
| Krea 2 prompting guide | detailed natural-language paragraphs, subject first, then materials, lighting, setting, style | the image system prompt orders the paragraph that way and keeps every subject, colour and medium the user gave |
| Qwen-Image-Edit-2511 settings in ComfyUI's templates | 40 steps / CFG 4 base, Lightning LoRA 4 steps / CFG 1 (shift 3), `ModelSamplingAuraFlow 3.1`, `CFGNorm`, `index_timestep_zero` | already what the templates ship (§2, §5c); the 8-step Lightning tier is the default |

**Settings check.** Our 2511 graphs match the official templates node for node (sampler euler /
simple, shift 3.1, CFG norm, the reference-latent method); the Lightning tier is the official
Lightning recipe with 8 steps instead of 4 (visibly cleaner edges at +4 s). Krea 2 Turbo: 8 steps,
CFG 1, `ConditioningZeroOut` negative, CLIPLoader type `krea2`, as shipped. Nothing was left on
the table there; the prompt was the lever.

**The enhancer, live** (`enhance-live.ps1`, the CLI against the server, 5090, ComfyUI 0.39):

| Case | time | the rewriter's sentence |
|---|---|---|
| the doodle, edit, "make this hyper realistic" | 0.9–1.3 s | Turn the doodle of a smiling face into a photograph of a real human face with the same expression and pose, replacing the black outline strokes with real skin, hair, and facial features; keep the white background unchanged. |
| the doodle, edit, "turn this drawing into a hyper realistic portrait that looks similar" | 0.9 s | Make the hand-drawn smiley face look like a hyper realistic portrait of a human face with the same expression and pose, replacing the black outline strokes with real skin, hair, and facial features, keeping the white background unchanged. |
| the doodle, edit, "make it a watercolor painting" | 1.1 s | Turn the hand-drawn smiley face into a watercolor painting of a smiling human face, replacing the black outline strokes with soft watercolor washes and brushstrokes, keeping the simple white background unchanged. |
| the lighthouse, edit, "make the sky stormy" | 1.0 s | Make the sky stormy with dark, swirling clouds and flashes of lightning, keeping the white lighthouse, red boat, and rocky shore unchanged. |
| the lighthouse, edit, "turn it blue" | 0.8 s | Turn the sky and sea blue, keeping the white lighthouse and red boat unchanged. |
| the lighthouse, edit, `put a sign on the lighthouse that says "OPEN"` | 1.1 s | Add a sign reading "OPEN" to the lighthouse's front door, keeping the white lighthouse, red boat, and rocky shoreline unchanged. |
| the lighthouse, fill with the upper part selected, "a hot air balloon" | 1.0 s | Add a hot air balloon floating above the lighthouse, blending with the soft pink sky, while keeping the white lighthouse, red boat, and rocky shoreline unchanged. |
| the same fill, "remove this" | 0.9 s | Remove the white lighthouse from the upper part of the image, keeping the pink sky and rocky shoreline intact. (the limit of the place hint: "this" was read as the lighthouse, not as the selected sky) |
| image idea, "a fox in snow" | 1.9 s | a 110-word paragraph: the fox alert in deep snow, fur, paws, soft diffused daylight, an open field, a photorealistic cool palette |
| image idea, already detailed (a fisherman mending a net, golden hour, 85mm) | 1.8 s | polished, keeping "85mm lens", "golden hour" and the shallow depth of field |

Same seed, same sentence every time (temperature 0.3); the seed changes little. The edit sentences
are what the Qwen guides ask for: the verb first, the subject named from the picture, what stays.

**The chat template.** The first live run answered nothing half the time: of 29 rewrites, 15 came
back empty and 9 began with a stray "assistant". ComfyUI's `TextGenerate` with `thinking: false`
appends an empty `<think>` block after the assistant turn (the Qwen3 convention), which the
non-thinking Qwen3-VL-4B-Instruct answers with an immediate end-of-turn or by restarting the
turn. `probe-enhance5.ps1`, 12 runs per variant: thinking=false 0 clean (7 empty, 5 echoes);
**thinking=true 12 of 12 clean**, with the rules in the user turn or as a real system turn
(`system_prompt`). The templates use thinking=true and the system turn.

**The doodle, through Generative Edit with `enhance: true`** (`bench/enhance-doodle-*.png`,
`doodle-sentence-*.png`): the first wording of the rules ("ends with what must stay unchanged")
made the rewriter write "keeping its simple outline and dot eyes unchanged", and 2511 drew a
loaf of bread with a smiley (as it had from the raw prompt). The sentence for "turn this drawing
into a … portrait" gave a human face with the doodle's lines still drawn over it. Three
hand-written sentences then showed what the model needs: "a photograph of a real human face …
replacing the black outline strokes with real skin, hair and facial features" (A) and "a hyper
realistic portrait photo of a real person … with no drawn lines left" (C) both gave a clean
photographic face in 19–20 s; "look like a hyper-realistic photograph of a real human face"
without the replacement clause (B) gave a face with the lines kept. The rules now say so for a
change of medium, and the rewriter's sentence for "make this hyper realistic" is the A form.

**Cost.** A rewrite is 0.8–1.3 s for a sentence, 1.8–2 s for a paragraph, after a 3 s first load
of the 4B encoder (5 GB staged). An enhanced Krea 2 image took 11.1 s against 6.2 s as typed
(the rewrite, a longer encode and the switch back from 2511). The encoder stays resident: with
2511 (19.6 GB), its own encoder (7.9 GB) and the 4B (5.0 GB) all loaded the card had 1.9 GB free
and 2511 edits ran at 19–25 s instead of 16 s (Krea 2 was resident too); with the enhancer and
2511 alone the steady state is 13.5–14.6 s (§5f).

Through the CLI, end to end (`enhance-doodle-final.ps1`, `bench/enhance-doodle-*-final.png`):
"make this hyper realistic" on the doodle now gives a photographic human face in 18.8 s (the
rewrite included), "turn this drawing into a hyper realistic portrait that looks similar" one in
17.2 s; both layers remember the typed prompt and the sentence the model got.

## 5f. NVFP4 against fp8 on the 5090

Comfy-Org publishes NVFP4 repacks of the 2511 text encoder (`qwen_2.5_vl_7b_nvfp4`, 5.7 GB
against 8.7 GB) and of Krea 2 Turbo (`krea2_turbo_nvfp4`, 7.2 GB against 12.2 GB); Blackwell
GPUs run the 4-bit weights through their own tensor cores (comfy-kitchen 0.2.37's
`scaled_mm_nvfp4`). Measured with the engine's own graphs posted to the server
(`nvfp4-bench.ps1`: a purge, then a cold run and two warm runs with their own seeds; ComfyUI
caches a repeated graph, which made a first attempt report 290 ms "runs"):

| Case (1024², 8 steps) | fp8 | NVFP4 |
|---|---|---|
| 2511 Lightning edit, the encoder swapped | cold 19.6 s, warm 14.1 / 14.0 s | cold 19.8 s, warm 14.1 / 14.2 s |
| Krea 2 Turbo image, the model swapped | cold 11.1 s, warm 7.1 / 7.0 s, 13.4 GB free afterwards | cold 9.2 s, **warm 5.7 / 6.0 s, 18.5 GB free** |
| 2511 edit with the enhancer's 4B encoder resident | 22.9 s with the load, then 13.5 / 13.6 s | 21.1 s, then 14.3 / 14.6 s |

The encoder runs once per prompt, so its precision does not move an edit's time; it saves
3 GB of weights (staged, in RAM and on disk). Krea 2 in NVFP4 is 18 % faster per image and
leaves 5 GB more free. Output: the lighthouse storm is the same picture with either encoder
(`bench/nvfp4-2511-te-*.png`), the fox equivalent (`bench/nvfp4-krea2-*.png`).

**Shipped as a policy, not a switch per file.** Template slots carry an `nvfp4` alternative
(the 2511 templates' encoder, Krea 2's model); Preferences › AI Integrations › **Model
precision** `auto` (the default) loads it when the server's GPU is a Blackwell part
(`is_blackwell` on `/system_stats`'s device name: GeForce RTX 50, RTX PRO Blackwell, B100–B300,
GB200/GB300, GB10) and the server lists the file; `default` never; `nvfp4` always (for
testing). The purge sees the switch (the model set is computed from the prepared request),
`generate.models` reports `device`, `blackwell` and each slot's `nvfp4` / `nvfp4Installed`, and
a caller's own `model` binding is never replaced. On Ampere and Ada there are no NVFP4 kernels
(ComfyUI would dequantize or fail), so `auto` leaves them on the default files. No official
NVFP4 repack of the 2511 diffusion model exists (a community one does); Qwen-Image-2.1 ships
int8 and w4a8 encoders instead, so neither has an alternative yet. Live through the CLI
(`nvfp4-live.ps1`): the server loaded `qwen_2.5_vl_7b_nvfp4` for an edit and
`krea2_turbo_nvfp4` for an image with nothing but the default preference.

## 5g. Reference images and the time estimate (PhotoshopEX, 2026-10-12)

Warm runs through the release CLI, one process, ComfyUI on the 5090 (NVFP4 text encoder by
the `auto` policy): `generate.edit` on the 1024² lighthouse picture (sent at 880×880 = 0.77 MP),
Lightning 8, the same prompt, with 0, 1 and 2 file references (1024² each, scaled to ≤ 1 MP):

| References | Run 1 | Run 2 |
|---|---|---|
| none | 11.9 s | 11.7 s |
| one | 19.9 s | 20.0 s |
| two | 30.1 s | 30.3 s |

Each reference image adds about 9 s (another megapixel of tokens through the encoder and the
reference latents). The templates' `estimatedSeconds` were fitted to these runs and the Generate
panel's fills: the Lightning-8 family is `7.5 s + 5.5 s/MP + 9 s per extra reference` (a fill at
~0.2 MP: 8.7 s measured, 8.6 s estimated; the edits above: 11.7 / 20.7 / 29.7 s estimated). A
reference-driven text to image (`qwen-edit-2511/image-ref`) counts its first reference as the
picture: 12.1 s measured for a 3:2 1K image from one reference, 11.3 s estimated. The 40-step
tier's per-reference cost is scaled by its steps and CFG passes (90 s, not measured).

## 5h. Generative Fill from a reference (PhotoshopEX, 2026-10-21)

`docs/comfy/bench/scripts/fill-ref-live.ps1`, ComfyUI on the 5090 (`--fast fp8_matrix_mult`),
the release CLI, one process per run (so each 2511 run below includes the model switch from
Krea 2 or the previous tier). Two Krea 2 pictures (a woman in a light grey cardigan; a red
cable-knit sweater on a person, 1024², 12.8 s and 6.0 s), the cardigan selected by text (SAM
3.1 on ComfyUI), then `generate.fill` with the sweater picture as a reference, `grow: 8`,
seed 7. The request: 928×816 (0.76 MP).

| Run | Template | Run time | What came back |
|---|---|---|---|
| typed "the red cable-knit sweater" | `fill-ref` (40 steps, CFG 4) | 150 s | the reference's cable knit, colour and ribbing on the woman, in the cardigan's silhouette (V neck: the selection decides the cut) |
| `enhance: true`, "the red sweater" | `fill-ref` | 150 s | the rewriter, shown the pair, wrote "red cable-knit sweater, crew neck, long sleeves, ribbed cuffs and hem, loose fit, textured pattern running down front and arms, worn over white top."; the picture differs from the typed run in 166 k pixels but reads the same |
| plain fill, "a red cable-knit wool sweater" | `fill` (40 steps) | 88 s | **the cardigan recoloured red**, its buttons and cut kept: the model copied what it saw under the mask, the failure the grey hole prevents |
| typed | `fill-ref-lightning-8` | 27.9 s | the same garment as the 40-step run (258 k pixels differ, mostly knit detail) |
| `enhance: true` | `fill-ref-lightning-8` | 29.2 s | the same |

Lessons: the grey hole is what makes a swap a swap; the selection's shape is the garment's
shape (select everything the new piece will cover, `grow` for the rest; the V stayed because
the white top inside it was not selected); the Lightning tier is the sensible default for a
swap (28 s against 150 s, the same garment); one reference on a fill costs about what it costs
on an edit (benchmarks §5g). Not yet judged over more seeds, and `erase: false` on the
reference template is untested live.

## 5i. Content-Aware Fill, Remove and Spot Healing through the model (PhotoshopEX, 2026-10-21)

The lighthouse picture's red boat (`final-fill-s11.png`, 1024², the 200×120 rectangle at
300,600 that `edit-live.ps1` edits), ComfyUI on the 5090, the release CLI, one process per
run. First the classical fills against the model (`caf-generative-live.ps1`, Quality `best`:
the 40-step 2511 fill):

| Run | Time | What came back |
|---|---|---|
| Content-Aware Fill, classical (PatchMatch, auto sampling) | 0.4 s | boat gone; a copied rock blob in the water and a ghost of the horizon |
| Content-Aware Fill, generative, the hole flat grey | 56 s | **the grey rectangle, painted back as a panel** |
| the same, pixels kept, "fill the flat grey area" wording | 44 s | the boat, untouched (there was no grey to fill) |
| Remove tool, classical (non-local patches) | 0.4 s | boat gone; rock copies at the stroke's end |
| Remove tool, generative, the stroke's hole grey | 47 s | boat gone, the sea continued cleanly |

Then the instruction matrix on the Lightning tier (`caf-matrix-live.ps1`, Quality
`balanced`, seed 7, the same rectangle):

| Variant | Time | Result |
|---|---|---|
| A: pixels kept, "Remove the object in the selected area and fill its place with the surrounding scene continued seamlessly…" | 11.6 s | boat gone, the sea and the horizon continued |
| B: the mask-guided template (`fill-guided`), pixels kept | 21.5 s | the same |
| C: pixels kept, the object named ("Remove the red boat…") | 15.9 s | the same |
| D: the hole grey, "Fill the flat grey area…" | 5.9 s | the grey panel |
| E: the guided template with the hole grey | 21.6 s | the grey panel |

So the removal path keeps the pixels and sends A's generic wording (`REMOVE_PROMPT`): 2511
reads the picture under the mask as truth and must be told what to do with it, and a flat
hole is "content" to it (the grey-hole trick serves a swap from a reference, where the
reference says what goes there; AnyPaint's model card says the same of Krea 2 on dark or
saturated pictures). Naming the object, which would cost a VLM pass, buys nothing here. One
picture, one seed: judge on more before trusting the margins.

**The stroke itself** (later the same day, `spot-generative-live.ps1`: the strokes through
`paint.spotHealing {type: generative}`, so what the app sends is what is measured; the boat is
145×37 at 308,649):

| Stroke | First version (hole = the stroke) | Now (object whole, grown 45, hole all-or-nothing) |
|---|---|---|
| S1: hard brush 56, along the whole boat | boat gone, a dark smudge of its reflection left under the hull | clean |
| S2: soft brush (hardness 0.3), the same | the smudge | clean |
| S3: hard brush 36, the middle of the boat only | **the boat redrawn, ribbed** (the user's report) | clean, the boat gone whole |

What each step was worth, measured on the way (`caf-prefill-live.ps1`, since dropped;
`caf-shape-live.ps1`; `caf-grow-live.ps1`; seeds 7 and 8, Lightning):

| Variant | Result |
|---|---|
| the hole pre-filled classically (PatchMatch), REMOVE_PROMPT or a "make it seamless" wording | **worse**: the model keeps the patch fill's copied rocks as a smear; a small hole on the bow comes back as a rock-coloured bow. 2511 treats what is under the mask as truth, so a pre-fill must be right or absent |
| the detector's silhouette of the boat (`select.byPoint` on the stroke's spots) grown 15 | the boat stays, both seeds |
| its bounding box grown 15 | the boat stays, both seeds: not the shape |
| the matrix's 200×120 rectangle (35–50 px past the boat) | gone, both seeds |
| the silhouette grown 30 | gone at one seed of two |
| the silhouette grown 45 or 60 | gone, both seeds |
| a soft brush's partial coverage in the noise mask | the boat stays: partial mask values keep that much of the rim |

So a removal (1) asks SAM 3.1 for the object under the stroke's spots (one point prompt, as
the Quick Selection tool) and takes it whole when the stroke covers at least a quarter of it
and it is under 40 % of the canvas, (2) makes the hole all-or-nothing wherever the brush
touched, (3) grows it by 30 % of its larger side (32 to 192 px), (4) runs the fill with the
pixels kept. The detector adds 0.7 s warm and about 4 s on the first ask (model load); a
whole stroke is 20–22 s wall on the Lightning tier with the server switching between the two
models. Content-Aware Fill's generative switch takes the same growth (the user's selection
is usually tight) but never asks the detector.

**A necklace** (the same night; `necklace.png`, a portrait the app's own text-to-image made in
7.4 s: a 6-px gold chain over the neck, collarbones and the neckline of a dark top, with a
small pendant; Select by Text finds it as 5296 px in a 409×297 box). The silhouette from
Select by Text under `generate.fill`, Lightning, seeds 7 and 8 (`necklace-grow-live.ps1`,
`necklace-name-live.ps1`):

| Instruction | grown 32 | grown 64 | grown 96 | grown 123 (30 %) |
|---|---|---|---|---|
| "Remove the object in the selected area…" | stays, stays | **stays, clean** | trace, stays | **clean, stays** |
| the same, rewritten by the vision-language model with the picture in view ("Remove the gold necklace and replace it with the surrounding skin, collarbone, and dark grey fabric…") | | clean, clean | | clean, clean |
| named by hand ("Remove the gold chain necklace and its pendant completely, leaving bare skin…") | | clean, clean | | clean, clean |

Unnamed, the necklace is a coin flip however far the hole is grown: the model's prior that this
portrait wears a necklace beats "the object". Named, every run is clean, and on the boat the
named instruction also removes it with the hole grown only 15 or 30 (`caf-grow-named-live.ps1`,
both seeds), where the generic one needed 45. So a removal now runs through the rewriter by
default (`enhance`, +3 s wall), and the growth went down to 15 % of the larger side (16–96 px).

Through the tools (`necklace-strokes-live.ps1`; the rewriter's words in each case):

| Stroke | Result |
|---|---|
| the Remove tool along the whole chain ("Remove the gold necklace and replace it with the woman's skin, collarbone, and dark grey shirt neckline…") | **clean**: chain and pendant gone, skin and neckline natural |
| Spot Healing along the left half of the chain ("Remove the gold pendant and chain…") | the left half and the pendant gone, the right half stays with a loose end: the detector's point prompt returns only the brushed half of a thin chain, so "the object whole" was half a necklace |
| Spot Healing, one 40-px dab on the pendant ("Remove the gold disc pendant… keeping the necklace chain intact") | the pendant gone, the chain joined across the gap. The first run left a lighter, smoother patch of fabric: a 108-px request sent at 512 (4.7×) gives the model no context; a removal's request is now at least 384 px a side |

Each tool stroke is 23–25 s wall on Lightning: the detector, the rewriter and the fill, with
the server loading each in turn.

**Asked again by name** (`remove-note-live.ps1`, `necklace-strokes-live.ps1` rerun). The
rewriter's sentence names the object ("Remove the gold necklace and replace it…" →
`named_object` → "gold necklace"), and the detector is asked a second time by that name (the
text template, every instance that the stroke covers a quarter of):

| Stroke | Before | Now |
|---|---|---|
| Spot Healing along the left half of the chain | the left half and the pendant gone, the right half left with a loose end | **the whole necklace gone** (the name ask returns the whole chain, 5296 px; the hole grows 64) |
| one dab on the pendant | the pendant gone, the chain joined | the same ("gold disc pendant": the dab covers all of it, the chain is not asked for) |
| the Remove tool along the whole chain | clean | clean |
| the boat's three strokes | clean | clean; one soft-brush run of seven put a rock where the boat was (the rewriter's "ocean and rocky shore… rock textures"), the other six are sea |

A removal note of its own for the rewriter ("name that object precisely, then only what lies
directly behind it") was tried against the rock and dropped: it named "the gold circular
pendant" where the fill note named "the gold necklace", so the name ask found less and the
half-necklace stroke regressed to half; the boat was clean either way. On a fresh server the
three asks come to 14–17 s wall a stroke on Lightning (the first stroke after a start 40 s).

**The robustness set** (the same day; `robust-make.ps1` makes fourteen scenes with the app's
own text-to-image, 1024², seed 31, and locates each object by Select by Text;
`robust-strokes.ps1` runs two strokes a scene through the tools' own commands, Spot Healing
along the first half of the object's long axis and the Remove tool in a serpentine over its
whole box; `robust-sheets.ps1` makes the sheets; every run's rewrite, growth and time go to
`bench\rb-log.txt`). Six rounds of refinement, the fixes in the dev log; what each round's
sheets showed:

| Round | Clean of 28 | What failed |
|---|---|---|
| 1: as committed before the set | (half strokes only) none whole | the quarter-of-the-object rule rejected every half stroke over a thick object; the rewriter named the part under the stroke ("the window", "the dog's head", "the hand") |
| 2: a tenth, the stroke outlined in green for the rewriter | 24 | "the green circle", "the green-painted face"; the Remove tool's serpentine on the logo removed the shirt, on the glasses the face (the point prompt's segment) |
| 3: two views side by side, name first, point prompt the fallback | 22 | the watch's place filled with the table, both strokes; a panel over the eyes (Remove); a lighter patch of sky where the crow was; the mug's rim without its coffee |
| 4: the name alone in the generic wording, tone matched at the edge, enclosed gaps filled | 25 | the watch clean (seed luck), the glasses half a wall panel, the crow's place blue on a grey sky, the cup's coffee named and emptied |
| 5: two seeds, the better edge match shown | 26 | a second wire drawn under the first; "the painted-over area" as a name |
| 6: the rewriter judges the two side by side, selection-names rejected | 26 (final run) | a faint circular seam where the mug's half stroke met the pattern; the crow's claws left on the wire |

The final run, all fourteen scenes clean at both strokes but those two: watch, glasses,
earring, tattoo, logo, car, person, lamp post, sign, crow, mug, ring, poster, dog, the
rewriter naming every one right ("silver watch", "black-framed glasses", "silver hoop
earring", "black rose tattoo", "red circular logo", "blue car", "man", "black lamppost", "blue
sign", "black bird", "white coffee cup", "gold ring", "white poster", "golden retriever").
On the glasses seed by seed (`glasses-seeds-live.ps1`), seeds 1–3 were clean and seed 4 a
smeared blob over one eye; the edge score chose the blob (its colour and smoothness matched the
cheeks), the judge chose seed 3. A stroke is 20–27 s wall on the Lightning tier (the name, the
detector, two fills, the judge; three outliers of 40–143 s were the server reloading).

**The stock set** (the same day; the generated set is made by a model of the same family the
fill model was trained alongside, so it looks the way that model expects). `stock-fetch.ps1`
takes one photograph per subject from Wikimedia Commons by its search API, only files whose
own metadata gives the licence as CC0 or Public domain, at 1600 px on the longer side, and
lists each file's Commons page, author and licence in `bench\stock\st-sources.txt`;
`stock-objects.ps1` locates the object by Select by Text (the search hits were not all on
subject: the "watch" is a man in a mask on a 1950s ship, the "necklace" a tiara in a
stereograph, the "cat" the Liddell sisters on a sofa, the "hydrant" a church with a small car,
the "bottle" a glass filling its frame); the same `robust-strokes.ps1` and `robust-sheets.ps1`
with `-Dir`, `-Prefix st`, `-Objects st-objects.json`. Twenty-six scenes: a Roman fresco, a
stereograph, a daguerreotype in its case, a sepia print, a night street, a kite festival, a
flock on a wire, a sunset silhouette, a museum hall, a subway hand.

Lightning tier, two seeds, as shipped for the generated set:

| | Scenes |
|---|---|
| clean at both strokes | the mask (a face invented in its place), the ring, the spider-web tattoo, the latte cup on wood, the dog on the lawn, the lamp post, the bicycle on cobbles, the traffic cone, the top hat (hair invented), the trash can beside the bench, the cow in the grass, the tiny bird in the flock |
| one of two | the boat silhouette (half a boat from the half stroke, the whole stroke clean but for a dot), the umbrella (the head missing under the half stroke, whole clean), the bear kite (gone from the half stroke, back on the whole) |
| neither | the transparent-framed glasses on the old man (redrawn both times), the tiara (named "the black frame"), the heart sign at night (Select by Text found a 66-px "poster" elsewhere), the two people on the beach (ghosts), the Liddell print ("the selected object"), the Wall St sign (gone, under a grey blur), the motorcycle (a grey placard in its place) |
| not a test | the glass that fills its frame; the church's small car (not judged) |

So 27 of 45 judged strokes, against 26 of 28 on the generated set. Two kinds of failure:
the rewriter and detector losing the object on old framed prints (the frame named instead of
the tiara; "selected object"; no name for the fresco's earring), and the eight-step fill model
itself leaving or redrawing an object, or blurring its place, on grainy and busy real scenes.
Names that are a frame, a border or "the object" are rejected now, as "the selected area"
was.

The nine failing scenes' half strokes again on the **40-step tier** (`-Quality best`,
`-Suffix -best`, 110–164 s a stroke against 20–28): the same result on every one, the
glasses, the tiara, the heart sign, the beach ghosts, the Liddell print, the Wall St blur and
the motorcycle's placard all as before, the kite gone as before. The step count is not the
lever on real photographs; the fill model's capacity at its working size is. The next lever is
a different fill model: the native Krea 2 port with AnyPaint's inpainting recipe (plan §9).

The stock photographs' sources, authors and licences are kept in
[`bench/stock-sources.txt`](bench/stock-sources.txt).

**What moved the stock set** (the same night, the nine failing scenes' half strokes under each
change, `-Suffix` runs of `robust-strokes.ps1`):

| Change | Result on the nine |
|---|---|
| the 40-step tier | the same nine outcomes at five times the time |
| Qwen-Image-2.1's fill (research licence) | the same, but the motorcycle removed cleanly |
| the whole-crop **edit** instruction, no noise mask (the user's suggestion: let an edit model edit) | the same nine outcomes |
| **the hole smoothed over** in the picture the model sees (`prefill`: a membrane from the hole's edge, `smooth_covered`) | the glasses gone, the tiara gone, the left walker gone, the motorcycle gone with the floor continued, the kite gone; the sign still a blur, the heart (a mislocated test) still there |

The three model-side changes did nothing because an edit model copies what its reference
shows, however it is asked; the smoothing takes the object out of the reference. (A flat grey
hole was painted back as a panel and a PatchMatch pre-fill's copied blobs were kept, both
measured earlier the same day: the model also copies what the hole is filled with, and a
smooth continuation is the one thing it has no reason to keep.) Both sets again with the
smoothing (`smooth-run.ps1`, the sheets `st-smooth-*.png`, `rb-smooth-*.png`):

| | Baseline | Smoothed |
|---|---|---|
| stock, half strokes (24 judged) | 13 | **16** |
| stock, whole strokes (24 judged) | 14 | **19** |
| generated, both strokes (28) | 26 | 26 |

New on the stock set: the transparent-framed glasses at both strokes, the tiara at both, the
mask with a consistent face, the Wall St sign on the whole stroke (the facade continued
instead of a blur), the left beach walker, the kite and the motorcycle on the whole stroke,
the bench itself. Still failing: a faint spider web left on the whole-stroke tattoo, a pale
blob under the bicycle, the fresco earring (repainted wider), the heart sign (the test's
66-px "poster"), and the half strokes that leave a boat's second figure, a headless figure
under the umbrella and the Liddell print untouched. So `prefill` is on by default for the
three removal commands.

## 6. Where the time goes, and what is left

- A 2511 fill is sampling-bound: ~1.3 s per Lightning step at a 512-px request, ~2 s at 1 MP
  on the 5090; the encoder 3–5 s when the pixels change; everything on the PhotoCraft side
  under 0.5 s.
- Next levers, in order of expected gain: a working FP8-tensor-core checkpoint (the scaled
  file gave noise here), SageAttention once a wheel exists for this torch build, a 20-step
  middle tier for the base (Comfy's note accepts 20 steps; CFG 4 is still two passes a step),
  TeaCache/EasyCache-style step caching for the 40-step base, detecting the memory-pressure
  state from `timings` to suggest a purge, and a prompt study for Expand over more seeds (§3:
  two seeds showed the wording matters and cannot rank it). The automatic purge on a model-set
  change shipped (§5c).
- On a 24 GB Ampere card (RTX 3090) the same tiers apply with roughly 2–3× the step times and
  no FP8 gain; the text encoder will live in RAM; the Lightning 8-step tier is the one to install.

## 8. The native-engine baseline (plan-native-engine §5, 2026-10-18)

The reference every engine of the native-engine plan is compared against: PhotoshopEX's
ComfyUI route as the product runs it (Quality `best`, the app-managed sidecar), through
`gn-bench run --engine comfy` in the `genai-native` repository (`crates/gn-harness/fixtures/`;
run folders under its ignored `references/2026-10-18-comfy-<suite>/`, 44 pictures, 52 MB).
ComfyUI 0.39.0, driver 617.42, RTX 5090 32 GB, one run per case, the server warm after the
first case.

| Suite | Cases | Template under `auto` / `best` | Engine time per case | Wall, whole suite |
|---|---:|---|---|---:|
| `t2i` | 28 | `krea2-turbo/image` (8 steps) | 4.0–4.4 s at 1024², 1344×768 and 768×1344 alike; 20.6 s for the first, cold | 142 s |
| `edit` | 8 | `qwen-edit-2511/edit` (40 steps) | 67.1–67.5 s; 119.7 s with one reference picture (two cases) | 652 s |
| `fill` | 8 | `qwen-edit-2511/fill` (40 steps) | 44.1–47.8 s; 59.5 s for the removal case (a 50 % rect) | 383 s |
| `sam` | 16 | `sam3.1/segment`, `-point`, `-box` | 0.2–0.7 s (text, point, negative point, box) | 8 s |

**sd.cpp, first rows (2026-10-19, prebuilt `sd-master-95b3090-bin-win-cuda12-x64`, arch 120,
flash attention on, `--rng cpu`, the fox prompt of `t2i-animal-s7` at 1024², 8 steps, CFG 1,
seed 7; `sd-cli` loads weights inside every call, which a resident worker would not):**

| Krea 2 Turbo through sd.cpp | Per step | Sampling (8) | Encode | Decode | `generate_image` | Weights in VRAM |
|---|---:|---:|---:|---:|---:|---:|
| GGUF Q8_0 DiT + Qwen3-VL-4B Q8_0 + Wan VAE (warm page cache) | 0.59 s (1.69 it/s) | 8.0 s | 1.1 s | 0.57 s (4.3 s the first time: graph build) | 9.7 s (+3.1 s DiT load inside the call) | 17.3 GB |
| The ComfyUI fp8 repack DiT + fp8 encoder (sd.cpp dequantises fp8; no tensor-core path) | ≈1.2 s | — | — | 0.7 s | 15.7–16.5 s | 16.8 GB |
| ComfyUI fp8, same prompt (row above, resident models) | ≈0.41 s | ≈3.3 s | ≈0.3 s | ≈0.3 s | 4.0–4.4 s | — |

Reading: sd.cpp's Q8 sampling step is **1.45×** ComfyUI's fp8 step on this card (the 1.3–2× band:
the Python-free lane stands, flagged for the kernel track — FP8/NVFP4 GEMMs in ggml's CUDA
backend are the lever); the whole-image gap (2.3×) is mostly per-call weight loading and a
slower text encoder, both of which the worker's residency removes. The picture is correct (the
fox, `_eval/sdcpp-out/krea2-gguf-q8-warm2.png`).

**The whole t2i suite through sd.cpp** (`gn-bench run --engine sdcpp`, same 28 cases, seeds and
sizes as the baseline; tables in `bench/sdcpp-t2i-summary-2026-10-19.md` and
`bench/sdcpp-t2i-compare-2026-10-19.md`): 28 of 28 ran, 311 s wall; per picture
`generate_image` 9.2–13.6 s (median 9.8), sampling 7.5–11.1 s (median 8.1), condition 1.1 s,
decode 0.58 s; **0.41× ComfyUI's wall per picture** (2.4×), the first picture apart (ComfyUI's
own cold start made that one 1.55× in sd.cpp's favour). **Parity at equal seed: 9 of 28** pass
the picture gate (PSNR ≥ 30 dB or SSIM ≥ 0.90; typography-s11 33.4 dB, animal-s7 30.9 dB,
product-s7 30.2 dB, landscape-s11 30.5 dB), 19 fall short at 14–29 dB. The misses are the same
scene and composition with detail drift (the street-wide pair: the same café window, umbrellas
and crossing), i.e. the noise path matches (`--rng cpu`) and the divergence is at the encoder
(the Q8 GGUF Qwen3-VL-4B and sd.cpp's own chat template against ComfyUI's fp8 repack and
template), amplified by 8 Turbo steps. Two consequences: the harness's seed-level parity gate
works across engines, and an engine that wants ComfyUI-identical output needs ComfyUI-identical
conditioning, which argues for our own encoder path in the native engine rather than borrowing
sd.cpp's.

**SAM 3.1 through ONNX Runtime (plan Phase 1, 2026-10-19; `gn-bench run --engine sam-ort`,
ORT 1.31.0 CUDA 13 provider, fp32 graphs exported from ComfyUI's own modules by
`genai-native/tools/export_sam31.py`; one process per case, so the wall includes 2 s of session
opening and the first-call autotune):** 16 of 16 at parity, bounds IoU 0.99–1.00
(`bench/sam-ort-comfy-compare-2026-10-19.md`); a first round through the community
`kishanstar2003` SAM 3 export reached 11 of 16 (text prompts only;
`bench/sam-ort-hf-compare-2026-10-19.md`). Engine times on the fox picture, warm:

| SAM 3.1 route | ComfyUI node (fp16, torch SDPA) | ORT fp32, first call | ORT fp32, warm | ORT fp16, warm | ORT fp16 + fused attention + device-resident features, warm (**the default set**) |
|---|---:|---:|---:|---:|---:|
| Picture embedding (vision trunk + both FPNs + CLS), once per picture | inside the node | 470–490 ms | 198 ms | 133 ms | **66 ms** (squash 8 + run 56) |
| Text (`select.byText`), one detection with 2 refinement passes, after the embedding | 154 ms whole node | 750 ms | 276 ms | 202 ms | **132 ms** (text 5 + decoder 36 + crop vision 66 + heads 2× 7) |
| Point (`select.byPoint`), 2 passes, after the embedding | 94 ms whole node | 140 ms | 21 ms | 22 ms | **10 ms** |
| Box, after the embedding | ≈ 90 ms | 137 ms | 21 ms | 22 ms | 10 ms |

Reading: the masks are ComfyUI's (fox: 344 133 vs 344 125 pixels by point, 344 151 vs 344 045 by
text, 142 239 vs 141 856 by box; every variant 16/16 at parity). The two steps that mattered
after fp16: **the feature maps stay on the device** (ORT IoBinding: the vision graph's 220 MB of
outputs no longer cross to the host and back for every prompt; "copy out" went from 25 ms to
0 and a heads call from 22 to 10 ms), and **the exporter's attention replaced by ORT's fused
`MultiHeadAttention`** (`export_sam31.py --mha`, a custom translation of
`scaled_dot_product_attention`; the trunk's run 71 → 56 ms). ORT's profile of the trunk shows
2 010 kernels per run and ≈ 37 ms of node time, so the remaining ≈ 20 ms is launch overhead; a
CUDA graph would remove it, but ORT 1.31's CUDA provider fails the capture on this model (the
path exists, off by default, `gn-sam --graph`). Against ComfyUI's node a fresh picture is
now **79 ms by point (the node: 94)** and 198 ms by text (the node: 154; the crop refinement
re-runs the trunk, as the node does). Tried and dropped: opset 23's `Attention` op (ORT 1.31's
CUDA provider ran it in 2.7 s); ORT's offline transformers optimizer (fused BiasGelu and
SkipLayerNorm only; RoPE and the window layout defeat its attention patterns).

**Qwen-Image-2.1 (plan Phase 2, 2026-10-19): the ComfyUI row and qwen-image-rs's first numbers
on this card** (`gn-bench run --engine comfy --template qwen-2.1/image` and `--engine qir`; the
`qwen-2.1/image` template: 25 steps, euler/simple, CFG 1, the INT8 ConvRot repack and the INT8
Qwen3-VL-8B; qwen-image-rs: the official bf16 snapshot, `--convrot` INT8 DiT, bf16 text encoder,
SageAttention2, one process per picture):

| Qwen-Image-2.1 at 1024², 25 steps | Per step | 25 steps | Text encoder | VAE decode | Per picture, resident | Per picture, cold process |
|---|---:|---:|---:|---:|---:|---:|
| ComfyUI (`best`, server warm) | ≈ 0.15 s | ≈ 3.7 s | inside | ≈ 0.2 s | 4.4–5.0 s engine time | 44 s the first (loads) |
| qwen-image-rs on the 5090 (4090-tuned tiles), first run | 0.14 s | 3.5 s | 17 s to load (bf16, 16 GB) | 2.1 s (tiled: 11 GB free) | — (no resident mode in its `generate`) | 45 s (16 GB encoder load + DiT quantise 20 s) |
| the same, page cache and INT8 cache warm (`genai-native/crates/qir`) | **0.116 s** | **2.9 s** | ≈ 5 s to load | 0.43 s | — | **11–13 s** (DiT cache 3 s + encoder 5 s + 2.9 + 0.4) |
| **PhotoshopEX's native worker** (`gn-diffusion` over `qir::pipeline`, `integrations.engine = native`, 2026-10-20; the app's `ms`, channel included) | 0.116 s | 2.9 s | 0.25 s (bf16, resident) | 0.43 s | **3.65–3.73 s** | 11.7 s the first picture (loads), then resident |

The native worker's picture is **0.8–1.3 s faster than ComfyUI's** at the same steps and size once
resident, and the first picture costs 11.7 s against ComfyUI's 44 s cold. Measured with ComfyUI
stopped: with its models still resident the same run took 11.9 s per picture (the card's 32 GB
oversubscribed), so the two engines are not to be kept warm together.

Reading: the port's kernels are correct on sm_120 at the authors' 4090 speed or a little better
(0.116–0.14 s/step against their 0.148), so the Python-free lane is at ComfyUI's speed (≈ 0.15),
not ahead; the gap to close is
tuning (tile plans for 170 SMs / 96 MB L2, Blackwell block-scaled GEMMs) and residency. Pictures
match ComfyUI's at scene level only (0 of 24 at seed parity, PSNR 5–17 dB): the noise source and
the schedule differ, which `gn-models` fixes by construction (ComfyUI's CPU noise and `simple`
sigmas).

**With ComfyUI's conventions (2026-10-20, `qwen-image-rs generate --comfy`: PyTorch's CPU
generator reproduced bit for bit, the `simple` schedule over the model's sigma table, its bf16
timestep rounding; against `references/2026-10-19-comfy-t2i-qwen21`, all 28 cases including
the non-square four):**

| Qwen-Image-2.1, 25 steps, 28 cases | PSNR | SSIM | At the 30 dB / 0.90 gate | Per picture (one process each) |
|---|---:|---:|---:|---:|
| qwen-image-rs, candle's RNG + diffusers' schedule (2026-10-19) | 5–17 dB | 0.1–0.7 | 0 of 24 | 26–40 s |
| `--comfy` with the wrong sigma table (`ModelSamplingDiscreteFlow`'s SNR shift, 1000 entries; the morning's runs) | 9–24 dB | 0.49–0.93 | 6 of 28 | 11.4–11.6 s |
| `--comfy` with the right one (`ModelSamplingFlux`'s exponential shift, 10 000 entries; `genai-native` `992231a`; `runs/2026-10-20-qir-t2i-comfy-flux`) | **16.7–41.3 dB** (the fox 39.9) | **0.71–1.00** | **24 of 28** | 11.3 s |
| the same against ComfyUI run with its bf16 encoder and DiT (`references/2026-10-20-comfy-t2i-qwen21-bf16`) | 16.9–41.3 dB | 0.72–1.00 | 25 of 28 | |
| for scale: ComfyUI's INT8 route against its own bf16 route | 6.1–42.4 dB | 0.75–1.00 | 26 of 28 | |

The four misses are the same cases for everyone: the brutalist facade at both seeds (ComfyUI
against itself 24.8 / 25.8 dB), the vehicle at seed 11 (23 dB; ComfyUI against itself 26), and
the BAKERY sign at seed 7, where the app's INT8 encoder flips the background to transparent
(ComfyUI's two routes 6 dB apart; ours matches the bf16 route at 37.9 dB).

**How the gap was found (2026-10-20).** With the wrong table the routes agreed after one step
(sigmas `[1, 0]` on both sides: 42 dB) and parted from the second (31.5 dB at 2 steps, 21 dB
at 25). Ruled out one by one on the fox before the cause showed: the attention kernel (exact
bf16 and SageAttention2 both 21.0 dB), this port's DiT quantisation (the bf16 DiT with exact
attention: 20.9 dB), the DiT weights (ComfyUI's INT8 ConvRot repack loaded into the port, its
256-column groups permuted into the port's Hadamard basis: 21.1 dB), the VAE (ComfyUI's own
final latent, normalised, through the port's decoder: 49 dB against ComfyUI's picture), and a
single DiT forward on identical inputs (1 % apart, as ComfyUI's INT8 and bf16 DiTs are from
each other). ComfyUI against itself, bf16 DiT instead of the INT8 repack: 43.9 dB on the fox,
30.6 on the facade, 26 of 28 cases at the gate. What was left was the sampler's own state, and
stepping ComfyUI's real `KSampler` path next to a plain Euler loop over the same model showed
the second sigma at 0.9795 where the SNR shift gives 0.9430: the Qwen-Image-2.1 model is a
`FLUX`-type model in ComfyUI, whose `ModelSamplingFlux` builds `e^0.69 / (e^0.69 + 1/t − 1)` over
10 000 timesteps. The script that had printed the "ground truth" sigmas had instantiated the
other class. Lesson for every later model: take the sigma table from the loaded model object,
never from a formula read off a class.

**The text encoders, measured on the way** (the fox prompt, 28 tokens after the system turn,
against transformers' bf16 Qwen3-VL-8B from the official snapshot):

| Text encoder | Error vs the oracle | Cosine, first token / mean |
|---|---:|---:|
| ComfyUI's `qwen3vl_8b_int8_convrot` (what the ComfyUI route conditions on) | 8.8 % | 0.902 / 0.9956 |
| the port, bf16 attention scores (as imported) | 4.1 % | 0.988 / 0.9992 |
| the port, attention scores and softmax in f32 (`genai-native` `9617518`, kept) | **2.7 %** | 0.9992 / 0.9997 |

The INT8 encoder's error costs the ComfyUI route little at picture level (its INT8 and bf16
routes agree at 24–42 dB), and the native route conditions the DiT closer to the model than
the ComfyUI route does. A second reference set, ComfyUI with the bf16 encoder and the bf16 DiT
(`references/2026-10-20-comfy-t2i-qwen21-bf16`, the model as published, run through ComfyUI's
API with the template's graph), sits next to the INT8 one the app's route produces.

**Qwen-Image-2.1 edit and fill natively (plan Phase 3, 2026-10-20).** Both routes driven by
the app's own CLI so the request is built the same way (the composite, the feathered mask, the
880 px working size, the fill's prompt wrapper): `gn-bench run --engine comfy` against ComfyUI
for the references, then with `--pref integrations.engine=native` for the native worker
(`references/2026-10-20-comfy-{edit,fill}-qwen21`, `runs/2026-10-20-native-{edit,fill}-qwen21`).
One CLI process per case, so every native case loads the models (about 8 s of its time):

| Suite, 8 cases each, 25 steps | ComfyUI warm, engine time | Native, per process (loads included) | PSNR / SSIM | At the gate |
|---|---:|---:|---:|---:|
| edit (one reference = the picture) | 5.4–5.9 s | 12.6–13.5 s | 36–40 dB on six; 21.2 dB / 0.94 and 23.8 dB / 0.97 on two | **8 of 8** |
| edit with a second reference | 8.2–8.6 s | 30 and 68 s (paging: see below) | 36.6 dB; 23.8 dB / 0.97 | (counted above) |
| fill (the picture and the mask) | 1.9–5.6 s | 9.6–12.1 s | 17–56 dB, SSIM 0.91–1.00 | **8 of 8** |

Resident (`qwen-image-rs edit`, the fox at 1024², one reference): the references' encoders
and the prefix pass 1.5 s, **sampling 3.8 s** (47 s before the prefix cache), decode 0.7 s; the
prefix cache holds every block's keys and values of the text and reference rows (4 GB for one
1024² reference, 8 GB for two). **Through the app, resident** (one CLI process, the worker
keeps the models): the first edit 12.3 s (loads), then **4.2 s per 880² edit against ComfyUI's
5.4–5.9 s warm**, a 608×464 fill **1.8 s** (ComfyUI 1.9–3.5 s). With two references the bf16
text encoder (16 GB), the INT8 DiT, the VAE, the vision tower and that cache oversubscribe the
32 GB card and the driver pages, hence the 30–68 s; `Options::evict_text_encoder` (on by
default) drops the encoder after the prompt when `free_vram` says the cache will not fit, and
loads it again for the next prompt (7 s): the fox-in-the-boat edit then takes 9.0 s (ComfyUI
8.2–8.6 s).
A direct `--engine qir` run of the edit suite is not comparable with the app's references (2 of
8): the app resizes a 1024² picture to 880² before the model sees it.

Also native the same evening (one CLI process each, loads included): Remove Background with
`qwen-2.1/matte` on the fox at 1024², 10.5 s (ComfyUI 13.8 s), the two routes' alphas at soft
IoU 0.992; a transparent Generate Image at 1024², 12.3 s; Generative Expand with
`qwen-2.1/expand` (the picture placed inside the canvas as the reference, the whole picture
along), 11.7–12.0 s for 256–384 px of new canvas on the fox, seamless on the four cases looked
at (two symmetric, two one-sided).

**Krea 2 Turbo natively (plan Phase 4, 2026-10-20 night).** The port (`genai-native`
`cc0ee18`…): ConvRot INT8 block linears, SageAttention2, bf16 Wan 2.1 VAE, ComfyUI's noise and
`simple` schedule (shift 1.15, 8 steps). The references are the app's ComfyUI route with the
fp8 repacks (`references/2026-10-18-comfy-t2i`) and the same graph with the bf16 files the
converter made (`references/2026-10-20-comfy-t2i-krea2-bf16`, through ComfyUI's API), 28 cases:

| Krea 2 Turbo, 1024² (4 at other aspects), 8 steps | Engine time per picture | At the 30 dB / 0.90 gate vs the fp8 route | vs the bf16 route |
|---|---:|---:|---:|
| ComfyUI, fp8 repacks (the app's route) | 4.0–4.7 s warm | — | 20 of 28 |
| ComfyUI, bf16 files | 5.5–6.7 s | 20 of 28 | — |
| native, INT8 ConvRot + SageAttention2 (`runs/2026-10-20-qir-krea2-t2i`) | **3.5–4.9 s** (one process per picture: 11 s to load, then the picture) | 16 of 28 | 17 of 28 |
| native, bf16 DiT + SageAttention2 (`runs/2026-10-20-qir-krea2-t2i-bf16`; candle's bf16 GEMMs, the encoder reloaded per prompt) | 8.6–15.0 s | 19 of 28 | **21 of 28** |

Reading: an 8-step Turbo at CFG 1 is sensitive to its arithmetic. ComfyUI's two precisions
disagree on 8 cases (street ×3, food ×2, architecture-s7, interior-s7, vehicle-s7), the native
route on 11–12, the same scenes plus macro ×2 and interior-s11, and the misses are the same
scene with details shifted (a menu card, a tree, an umbrella), not another picture. The fox:
35.6 dB against the fp8 route, 36.2 against bf16; ComfyUI's two routes 39.3 dB apart on it.
The single DiT forward on identical inputs is 1.6 % from ComfyUI's fp8 route (cosine 0.99988),
the conditioning 1 %, the VAE decode 0.24 %. With the bf16 DiT the native route agrees with
ComfyUI's bf16 route as well as ComfyUI's fp8 route does (21 against 20 of 28), so the INT8
ConvRot linears cost about four cases on this model; the fp8 GEMM of the kernel track (§11)
is the lever that keeps both. **Through the app, resident** (`native-krea2-live.bat`,
`integrations.engine = native`): the first picture 15.7 s (loads and quantises), then **4.85 s
and 4.31 s per 1024² picture** against ComfyUI's 4.0–4.7 s warm; swapping the worker to
Qwen-Image-2.1 and back costs the loads (18 s, 23 s). Routing: Krea 2 stays on ComfyUI under
`auto` (neither faster nor at the INT8 precision ComfyUI's own two routes reach); `native`
serves it.

Parity yardsticks and thresholds: `genai-native/docs/PARITY.md`. The SAM suite compares
instance bounds until an engine returns masks (Phase 1). The gates these numbers set for later
rows: sd.cpp within 1.3× of a row keeps that model on the Python-free lane without further work;
the native engine takes a model under `auto` only when it beats its row at parity.
