# Model catalogue

Researched 2026-10-08. Model facts move fast; every row carries its source. Licence summaries are
not legal advice: read the licence file on the model card before shipping a build that offers the
model by default.

Target hardware for the numbers below: one NVIDIA RTX 5090 (32 GB VRAM, Blackwell), Windows 10.
ComfyUI offloads text encoders and VAEs between runs, so "fits" means the diffusion transformer
plus working memory at 2K resolution.

Licence classes used in this document:

- **Permissive**: Apache-2.0 / MIT. Commercial use allowed. Safe as a default in an open-source app.
- **Community**: open weights with conditions (revenue caps, moderation duties, naming rules).
  Usable, but the app must tell the user the conditions.
- **Research-only**: non-commercial. Can be offered for personal use only, never as the default
  for a feature the user might use commercially.

## 1. Primary models

### Krea 2 Turbo (text-to-image, style reference)

| | |
|---|---|
| Weights | `krea/Krea-2-Turbo` (gated: accept the licence on Hugging Face); ComfyUI repack `Comfy-Org/Krea-2` |
| Released | 2026-06-22 (v1.0) |
| Architecture | Diffusion Transformer, 12 B parameters (HF metadata says 13 B); trained from scratch |
| Conditioning | Text encoder **Qwen3-VL-4B** (`qwen3vl_4b_fp8_scaled.safetensors`), VAE **`qwen_image_vae.safetensors`** (the Qwen-Image VAE) |
| Inference | 8 steps, CFG 0.0, shift/mu 1.15, 1K–2K output (set 2.0 megapixels for 2K) |
| ComfyUI | Native since **0.26.0 (2026-06-23)**. Official workflows: "Text to Image (Krea-2 Turbo)" and "Image Style Reference (Krea-2 Turbo)" subgraphs; style reference uses `krea2_turbo_int8_convrot.safetensors` + the `krea2_style_reference.safetensors` LoRA; style LoRAs such as `krea2_softwatercolor.safetensors` |
| Files (ComfyUI) | `diffusion_models/krea2_turbo_fp8_scaled.safetensors` (FP8, recommended), BF16 / NVFP4 / MXFP8 variants for high-end cards; GGUF community quants need a custom node because stock GGUF loaders do not yet know the `krea2` architecture tag |
| VRAM | FP8 transformer ≈ 12 GB; BF16 ≈ 24.8 GiB (community figures). The 5090 runs BF16 or NVFP4 comfortably |
| Licence | **Community: Krea 2 Community License.** Gated; deployers "are required to implement content filtering measures or equivalent review processes"; Krea claims no copyright over outputs. Third-party summaries report commercial use allowed below **US$1M annual revenue** (one source adds "under 50 seats"), derivatives must keep the licence and start their name with "Krea". Full text: krea.ai/krea-2-licensing. **Verify before relying on the thresholds.** |
| Role here | Default **Generate Image** model and the style-reference path. Best open text-to-image quality at 2K in 8 steps. |

Sources: [Krea 2 open-source page](https://www.krea.ai/krea-2-open-source) · [krea/Krea-2-Turbo model card](https://huggingface.co/krea/Krea-2-Turbo) · [ComfyUI Krea-2 workflow docs](https://docs.comfy.org/tutorials/image/krea/krea-2) · [krea-ai/krea-2 inference code](https://github.com/krea-ai/krea-2) · [Krea 2 local guide (third party)](https://localaimaster.com/blog/krea-2-local-guide) · [Krea 2 open weights write-up](https://www.computeleap.com/blog/krea-2-open-weights-image-model-frontier-2026/).

### Krea 2 Raw (base model for fine-tuning)

Undistilled pretrained checkpoint (`krea/Krea-2-Raw`, gated), 52 steps, "diverse, malleable, built
for fine-tuning and LoRA training"; Krea does not recommend it for plain generation. Same licence
and conditioning as Turbo. Relevant to us only as the base of editing LoRAs (next entry) and for
users who train their own styles.

### "Krea 2 Edit" (community LoRA, not an official Krea release)

**There is no official Krea 2 editing checkpoint.** What the community calls "Krea 2 Edit" is
`conradlocke/krea2-identity-edit`, an unofficial LoRA fine-tune of Krea 2 Raw (v1.2 recommended
file `krea2_identity_edit_v1_2.safetensors`): instruction-based, identity-preserving editing with
head / face / person swap, inpainting, outpainting, virtual try-on and character reference sheets.
It needs its own ComfyUI node pack because it uses dual conditioning (in-context VAE tokens plus an
image-grounded Qwen3-VL encoding) that stock nodes do not provide. Licence: Krea 2 Community
License (inherited). Status for us: **experimental lane**, behind the Apache-licensed editors
below, re-evaluated when Krea ships an official editor.

Sources: [conradlocke/krea2-identity-edit](https://huggingface.co/conradlocke/krea2-identity-edit) · [community demo](https://krea2edit.github.io/) · [Civitai Krea 2 ecosystem](https://civitai.com/ecosystems/krea2).

### Qwen-Image-2.1 (generation + editing, RGBA output)

| | |
|---|---|
| Weights | `Qwen/Qwen-Image-2.1` (BF16 safetensors); ComfyUI repack `Comfy-Org/Qwen-Image-2.1`: `diffusion_models/qwen_image_2.1_int8_convrot.safetensors` (or `_bf16`), `text_encoders/qwen3vl_8b_int8_convrot.safetensors` (or `_bf16`), **its own VAE** `vae/qwen_image_2.1_vae_bf16.safetensors`, optional prompt-enhancer encoders `qwen3.5_9b_qwen_image_2.1_pe_t2i` / `_pe_i2i` |
| Released | 2026-09-20 |
| Architecture | 7 B visual generator, 32 single-stream DiT layers, mixed-granularity attention, prefix KV-cache reuse; text encoder Qwen3-VL 8B |
| Capabilities | One model for text-to-image **and** editing; up to **10 reference images**; local edits from circles, painted annotations or masks; identity preservation; **native RGBA / transparent output** (subject extraction without a matting model); native 2K (e.g. 2048², 2752×1536) |
| Inference | 40 steps in the Diffusers examples; ComfyUI templates ship their own settings |
| ComfyUI | Native since **0.37.0 (2026-09-20)** with official text-to-image, image-edit and background-removal templates (`image_qwen_image_2_1_t2i.json`, `image_qwen_image_2_1_image_edit.json`, `image_qwen_image_2_1_background_removal.json`). Nodes: `TextEncodeQwenImage21` (prompt, negative_prompt, resolution 0 = keep the input size, a grouped `images` input whose **API-format keys are `images.image_1`…`images.image_16`**; outputs positive, negative and an **empty** latent sized to image_1: the references enter the conditioning as latents, so 2.1 regenerates the whole picture in context and latent masking does not inpaint), `QwenImage21Cache` on the model, `CLIPLoader` type `qwen_image`, KSampler euler/simple **25 steps, CFG 1**. PhotoCraft templates: `qwen-2.1/fill` (the selection mask goes in as `images.image_2` with a local-edit instruction wrapped around the prompt; the layer mask confines the visible change) and `qwen-2.1/image`. |
| VRAM | int8 transformer plus an 8 B encoder: comfortable on 32 GB; community GGUF quants exist (`AlperKTS/Qwen-Image-2.1-GGUF`) |
| Licence | **Research-only: Qwen Research License Agreement** (research or evaluation purposes; commercial use needs a separate agreement from Alibaba). This is a change from every earlier Qwen-Image release, which was Apache-2.0. Output ownership is not spelled out in the licence; Alibaba's official account has said images belong to the user. |
| Role here | The model the user asked for ("qwen 2.1 edit"). Offered for **personal / research use**, with the licence shown in the picker. Its RGBA output is the best local **Remove Background** available. |

Sources: [Qwen/Qwen-Image-2.1 model card](https://huggingface.co/Qwen/Qwen-Image-2.1) · [QwenLM/Qwen-Image-2.1](https://github.com/QwenLM/Qwen-Image-2.1) · [ComfyUI v0.37.0 notes (wiki)](https://comfyui-wiki.com/en/news/2026-09-20-comfyui-v0-37-0) · [GIGAZINE hands-on](https://gigazine.net/gsc_news/en/20260924-qwen-image-2-1/) · [licence analysis (datanorth)](https://datanorth.ai/news/qwen-releases-qwen-image-2-1) · [licence analysis (latenode)](https://latenode.com/ai-trends/qwen-image-2-1-commercial-licence).

### Qwen-Image-Edit-2511 (instruction editing, permissive)

| | |
|---|---|
| Weights | `Qwen/Qwen-Image-Edit-2511`; ComfyUI files `qwen_image_edit_2511_fp8mixed.safetensors` (24 GB cards) and `qwen_image_edit_2511_bf16.safetensors` |
| Released | 2025-12-23 (the Edit line ships dated snapshots: 2509 on 2025-09-22, then 2511) |
| Capabilities | Single- and multi-image instruction editing, strong text rendering, pose/identity consistency; Lightning LoRAs give 4–8-step edits |
| ComfyUI | Native since December 2025 (`TextEncodeQwenImageEditPlus` family of nodes; official templates on docs.comfy.org). The official template (2026-10) chains UNETLoader → ModelSamplingAuraFlow 3.1 → CFGNorm 1.0 → (Lightning LoRA) → KSampler euler/simple, 40 steps CFG 4 (Comfy's note: 20 steps CFG 4 is fine) or 4 steps CFG 1 with the LoRA, and puts `FluxKontextMultiReferenceLatentMethod index_timestep_zero` on both conditionings |
| Speed tiers | **Lightning LoRAs by lightx2v, Apache-2.0** (checked 2026-10-09): `Qwen-Image-Edit-2511-Lightning-4steps-V1.0-bf16.safetensors` and `…-8steps-V1.0-bf16.safetensors`, 850 MB each, in `ComfyUI/models/loras`. PhotoCraft templates: `qwen-edit-2511/fill` (40 steps), `fill-lightning-8` (the `auto` tier when its LoRA is installed), `fill-lightning-4`, and `fill-guided` (mask as a second reference image: best placement, 1.5–3× slower). The same repository's `qwen_image_edit_2511_fp8_e4m3fn_scaled.safetensors` (19 GB) gave noise with our graph under `--fast fp8_matrix_mult` on 2026-10-09: not used. Numbers in [`benchmarks.md`](benchmarks.md) |
| Licence | **Permissive: Apache-2.0** (model and Lightning LoRAs) |
| Role here | **Default editor** for Generative Fill, Expand, Remove and Harmonize in any build that must stay commercially usable. |

Sources: [Qwen/Qwen-Image-Edit-2511](https://huggingface.co/Qwen/Qwen-Image-Edit-2511) · [ComfyUI blog: Qwen Image Edit 2511](https://blog.comfy.org/p/qwen-image-edit-2511-and-qwen-image) · [ComfyUI docs: Qwen-Image-Edit-2511 workflow](https://docs.comfy.org/tutorials/image/qwen/qwen-image-edit-2511) · [official template JSON](https://github.com/Comfy-Org/workflow_templates/blob/main/templates/image_qwen_image_edit_2511.json) · [lightx2v/Qwen-Image-Edit-2511-Lightning](https://huggingface.co/lightx2v/Qwen-Image-Edit-2511-Lightning) (Apache-2.0) · [local guide (third party)](https://localaimaster.com/blog/qwen-image-edit-local-guide).

Qwen-Image-2.1's few-step option, `Viggle/Qwen-Image-2.1-viggle-turbo` (6 steps, v0.3, 2026-09), is
under the same **Qwen Research License** as the base model and needs the author's custom nodes (a
sigma schedule and an unmerged LoRA loader) rather than a stock `LoraLoaderModelOnly`; it is not
shipped as a template. `QwenImage21Cache` (in our 2.1 templates) is the other 2.1 speed lever.

Note: there is **no** "Qwen-Image-Edit-2512". `Qwen-Image-2512` (2025-12-31, Apache-2.0) is the
text-to-image base of that generation; its editing counterpart is Edit-2511.

### Z-Image Turbo (fast permissive text-to-image)

6 B distilled model from Alibaba Tongyi Lab, released 2025-11-27, about 8 sampling steps,
**Apache-2.0** (base Z-Image released 2026-01-27, also Apache-2.0). ComfyUI BF16 weights from
Comfy-Org. VRAM: BF16 ≈ 16 GB, FP8 ≈ 8 GB, GGUF ≈ 6 GB. Role: the permissive, fast default when
Krea 2's community licence is not acceptable, and the model for live/low-latency previews.

Sources: [Z-Image Turbo ComfyUI guide (thundercompute)](https://www.thundercompute.com/blog/z-image-turbo-comfyui) · [open-model survey (bentoml)](https://www.bentoml.com/blog/a-guide-to-open-source-image-generation-models).

## 2. Other models worth knowing

| Model | What | Licence | Notes |
|---|---|---|---|
| FLUX.2 [dev] | 32 B generation + editing (Black Forest Labs, Nov 2025) | FLUX non-commercial | Comfy-Org FP8 builds; [pro]/[flex] are API-only. Licence reportedly mandates safety filtering. |
| FLUX.2 [klein] 4B | small generation model (Jan 2026) | Apache-2.0 | 8–13 GB VRAM (sources disagree). The 9 B klein is non-commercial. |
| FLUX.1 Kontext [dev] | the 2025 instruction editor | FLUX non-commercial | Adobe offers Kontext *pro* as a partner model; dev is the local equivalent. |
| FLUX.1 Krea [dev] | 2025 BFL × Krea aesthetic fine-tune | FLUX non-commercial | superseded by Krea 2 for our purposes. |
| Qwen-Image-2512 | 2025-12-31 text-to-image base | Apache-2.0 | good LoRA-training base; permissive T2I alternative. |
| Qwen-Image-Edit-2509 | the previous editor | Apache-2.0 | keep for compatibility with existing LoRAs. |
| BiRefNet | matting / background removal | MIT | ComfyUI custom nodes; fast; no diffusion needed. |
| RMBG-2.0 (BRIA) | background removal | non-commercial | do not make default. |
| **SAM 3.1** (Meta, 2026-03-27) | open-vocabulary segmentation: text prompts ("red car", `eye:2, window panels:4`, ≤ 32 tokens, comma-separated terms), point/box prompts, video tracking (Object Multiplex) | **SAM License** (custom, see below) | **Native in ComfyUI** (PR #13408 by kijai, a dependency-free re-implementation); templates "SAM3: Image Segmentation" / "SAM3: Video Segmentation" under Utility; one checkpoint `checkpoints/sam3.1_multiplex_fp16.safetensors` from `Comfy-Org/sam3.1` (≈ 1.75 GB); the official `facebook/sam3` weights are gated behind accepting the licence. **This is the model for Select by text, ML Select Subject and object masks that feed Generative Fill.** |
| SAM 2 | promptable segmentation (point/box) | Apache-2.0 | superseded by SAM 3.1 for us; keep as the permissive fallback. |

**SAM License in one paragraph** (read 2026-10-08 from `facebookresearch/sam3/LICENSE`; not legal
advice): a non-exclusive, worldwide, royalty-free licence to use, reproduce, distribute and modify
the "SAM Materials" (code, weights, docs) for research **and** commercial use, with no user or
revenue thresholds. Conditions: pass the licence on with any redistribution, acknowledge SAM in
publications, comply with trade controls; no reverse engineering; **no ITAR, military/warfare,
nuclear, espionage or weapons end uses**; suing Meta over the materials terminates the licence;
Meta may change the terms with immediate effect. Outputs (masks) are not restricted. For an
open-source editor this is workable: the app ships no weights, points the user at the gated
download, and shows the licence summary once. Sources: [Meta SAM 3 / 3.1 repo](https://github.com/facebookresearch/sam3) · [SAM License](https://github.com/facebookresearch/sam3/blob/main/LICENSE) · [ComfyUI SAM 3.1 guide (official)](https://docs.comfy.org/tutorials/utility/video-segment-sam3) · [ComfyUI PR #13408](https://github.com/Comfy-Org/ComfyUI/pull/13408) · [SAM 3.1 release note (third party)](https://the-agent-report.com/2026/05/meta-sam-3-1-video-detection-multiplexing-may21/) · [Ultralytics SAM 3 page](https://docs.ultralytics.com/models/sam-3).
| SeedVR2, 4x-UltraSharp, SUPIR | upscalers | **verify** (SeedVR2 is reported Apache-2.0; 4x-UltraSharp CC BY-NC-SA; SUPIR non-commercial) | Generative Upscale lane; licences must be checked per model before listing. |

Sources: [FLUX.2 / Z-Image on AWS (hands-on)](https://builder.aws.com/content/363GAtT4hAB7stB8mbva5H1h1Ab/running-flux-2-and-z-image-diffusion-models-on-aws-a-hands-on-implementation-guide) · [best ComfyUI models 2026](https://iimagined.ai/blog/best-comfyui-models-2026) · [open-model comparison](https://www.pixazo.ai/blog/top-open-source-image-generation-models).

## 3. Feature → model mapping

| Feature (command) | Default (permissive build) | Best quality (user opts in) | Workflow shape |
|---|---|---|---|
| Generative Fill (`generate.fill`) | Qwen-Image-Edit-2511, `auto` = the Lightning 8-step tier when its LoRA is installed, else 40 steps | Qwen-Image-2.1 | composite crop (selection bounds + context margin, sent at ≤ 1 MP) + mask → edit model with inpaint conditioning → resampled back (Lanczos) under the selection's full-resolution mask |
| Generative Expand (`generate.expand`) | Qwen-Image-Edit-2511 | Qwen-Image-2.1 | pad composite to the new canvas, mask = padding (feathered inward) → same as Fill |
| Generate Image (`generate.image`) | Z-Image Turbo | Krea 2 Turbo | prompt (+ optional reference / style LoRA) → new layer or new document |
| Instruction edit (`generate.edit`) | Qwen-Image-Edit-2511 | Qwen-Image-2.1 (multi-ref) | layer or composite + instruction (+ optional mask, references) → new layer |
| Select by text (`select.byText`) | SAM 3.1 | SAM 3.1 | prompt → instance masks → a selection (union, or one instance by index); the selection then drives Fill, Remove, masks, anything |
| Select Subject / Sky / Hair (ML) (`select.subjectML`) | SAM 3.1 ("person", "sky"…) or BiRefNet for soft hair mattes | SAM 3.1 + BiRefNet refine | replaces the classical `select.subject` path when the server is up |
| Remove Background (`generate.removeBackground`) | SAM 3.1 or BiRefNet | Qwen-Image-2.1 RGBA | layer → alpha matte → layer mask (never destroys pixels) |
| Harmonize (`generate.edit` preset) | Qwen-Image-Edit-2511 | Qwen-Image-2.1 | pasted layer + composite context + "match lighting and colour" → new layer |
| Generative Upscale (`generate.upscale`) | ESRGAN-class (licence-checked) | SeedVR2 | layer → upscale-model node → new layer / Image Size |
| Generate Similar | same model as the source layer | — | stored prompt + new seed |

## 4. VRAM budget on the RTX 5090 (32 GB)

| Setup | Approximate resident VRAM | Fits? |
|---|---|---|
| Krea 2 Turbo FP8 + Qwen3-VL-4B FP8 + VAE | ≈ 12 + 4–5 + <1 GB | yes, with headroom for 2K |
| Krea 2 Turbo BF16 | ≈ 25 GB transformer | yes, encoder offloaded to RAM between runs |
| Qwen-Image-2.1 int8 + Qwen3-VL-8B FP8 + VAE | ≈ 8–9 + 8–9 + <1 GB | yes |
| Qwen-Image-Edit-2511 fp8mixed | sized for 24 GB cards | yes |
| Z-Image Turbo BF16 | ≈ 16 GB | yes |

These are community figures (see sources above), not measurements on this machine. Phase 1 of the
roadmap includes measuring them with `GET /system_stats` before and after a run and recording the
numbers in [`devlog.md`](devlog.md).

## 5. What the picker must show

For each installed model the UI shows: name, task (generate / edit / matte / upscale), licence
class with a one-line summary and a link, resolution range, steps, and whether it is loaded on the
server right now (`/system_stats`, `/object_info`). Research-only models are never pre-selected.
Community-licence models show their conditions once per session. The catalogue is data
(`docs/comfy/models.toml` in Phase 4), not code.

## Sources (feature list to match)

- Adobe help: [Generative Fill on desktop](https://helpx.adobe.com/photoshop/desktop/create-open-import-images/create-images/edit-images-with-generative-fill.html) (Adobe models Firefly Fill & Expand, Firefly Image 5, Firefly Image 1; partner models FLUX.2 pro, FLUX Kontext pro, Gemini 3.1 / Nano Banana 2, Gemini 3 / Nano Banana Pro).
- [Adobe Design: partner models in Photoshop](https://medium.com/@Adobe_Design/behind-the-design-partner-models-in-adobe-photoshop-91b54e1aaca9).
- [What's new in Photoshop 2026 (PhotoshopCAFE)](https://photoshopcafe.com/whats-new-in-photoshop-2026-full-release-overview/) · [Generative Upscale 2026](https://www.photoshopessentials.com/photo-editing/how-to-use-generative-upscale-in-photoshop-2026/) · [Harmonize / Generative Upscale announcement](https://alternativeto.net/news/2025/7/photoshop-expands-ai-tools-with-harmonize-generative-upscale-and-project-collaboration) · [AI features field report](https://weandthecolor.com/ai-features-in-adobe-photoshop-that-actually-changed-how-i-work-a-designers-field-report/208072).
