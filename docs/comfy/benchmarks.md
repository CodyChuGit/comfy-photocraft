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
request mask is feathered outward by 2 % of the longer side (4–24 px) so the latent noise mask
blends the edge; the layer mask stays the selection. `fill-guided` remains in the picker for
the cases where placement matters more than time. Qwen-Image-2.1 put the boat on the water at
every seed tried; it is the research-licensed option.

## 4. Native-size sampling (shipped)

The official 2511 graph encodes the `FluxKontextImageScale` output, so every request is sampled
at ~1 MP whatever its size. The shipped templates now VAE-encode the uploaded image itself: the
text encoder still sees the 1 MP reference, the sampling latent has the request's own size
(the engine grows the request rectangle to the 16-px grid, caps it at 1 MP and floors it at
512 px on the longer side). Same three cases, Lightning 8, same seeds:

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
with 2511 fp8 is ~1.6–2.5 GB free on a 32 GB card (UNET 19.6 GB + text encoder 7.9 GB + VAE),
which is fine as long as nothing else is resident.

## 6. Where the time goes, and what is left

- A 2511 fill is sampling-bound: ~1.3 s per Lightning step at a 512-px request, ~2 s at 1 MP
  on the 5090; the encoder 3–5 s when the pixels change; everything on the PhotoCraft side
  under 0.5 s.
- Next levers, in order of expected gain: a working FP8-tensor-core checkpoint (the scaled
  file gave noise here), SageAttention once a wheel exists for this torch build,
  TeaCache/EasyCache-style step caching for the 40-step base, and detecting the
  memory-pressure state from `timings` to suggest the purge.
- On a 24 GB Ampere card (RTX 3090) the same tiers apply with roughly 2–3× the step times and
  no FP8 gain; the text encoder will live in RAM; the Lightning 8-step tier is the one to install.
