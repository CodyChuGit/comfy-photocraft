# The Rust rebuild: one executable, no Python, no ONNX Runtime

> **Mission.** Every generative feature of PhotoshopEX runs in PhotoshopEX's own Rust code:
> the editor and its native worker (`photocraft.exe --engine`, genai-native's `qir`,
> `gn-diffusion`, `gn-perception`, `gn-engine`), on candle and our own CUDA kernels. No
> ComfyUI, no `python.exe` process, no ONNX Runtime, no Python step to prepare a model. The
> only libraries beside the executable are NVIDIA's (cuBLAS/cuBLASLt/cuRAND, loaded on first
> use). ComfyUI stays an optional engine for people who have it, never a requirement.

Owner's ask (2026-10-27): "the full rust rebuild of onnx and stuff so everything can be one
exe and rust native without having weird python instances that OOM", then "we need to get
started on the rust rebuild of this project, make this its own task; use Codex as much as you
need for parallelizing tasks".

This file is the task: its definition of done, the work queue, who does what, and the
progress log (newest last). The plan behind it is `docs/comfy/plan-native-engine.md`
(phases 5 and 6); the measurements are in `docs/comfy/benchmarks.md`.

---

## 1. Done when

1. On this PC with ComfyUI **not running and not configured** (`integrations.comfyPath` and
   `comfyServer` empty) and `integrations.engine = native`, every generative command in the
   feature table (§2) produces its result, and no `python.exe` starts (`walkthrough-live.ps1`
   extended to check the process list).
2. The worker loads no `onnxruntime*.dll` and no `cudnn*.dll` (checked with the module list of
   the running worker).
3. The models come from one folder the user points at, in ComfyUI's own layout
   (`diffusion_models/`, `text_encoders/`, `vae/`, `loras/`, `sam3/`), with no converted or
   exported copy: no `convert-fp8`, no `export_sam31.py`.
4. `packaging/windows/package.ps1 -Native` builds a portable zip whose runtime folder holds
   only NVIDIA's libraries (target under 700 MB; cuDNN's 542 MB and ONNX Runtime's 205 MB
   gone), and a fresh Windows user profile on this PC runs the walkthrough from it.
5. Each model moved to native is at parity with its ComfyUI route by the harness's rule
   (`plan-native-engine.md` §12.1) or beats it on a measured benchmark, and is at least as
   fast. Tests, clippy, wasm, layers, parity and scorecard gates stay green at every commit.

## 2. Where it stands (2026-10-27)

| Feature | Default model | Native today | Left to do |
|---|---|---|---|
| Generate Image | Krea 2 Turbo | yes (fp8, flash, fused; 4.0 s) | read ComfyUI's folder as is (R4) |
| Generative Fill (prompted) | Qwen-Image-2.1 fill / edit by instruction | yes (with the research opt-in) | the permissive default is still 2511 on ComfyUI (R6) |
| Remove, Content-Aware Fill, Spot Healing (generative) | 2.1 edit | yes | — |
| Generative Expand | 2.1 expand | yes | — |
| Generative Edit | 2.1 edit; Krea 2 edit (General Edit V2) | yes | — |
| Remove Background (matte) | 2.1 matte | yes | — |
| Select by text / subject / point, Object Selection | SAM 3.1 | **through ONNX Runtime + cuDNN**, ONNX graphs made by a Python export | SAM 3.1 on candle (R1) |
| Prompt rewriter, judge, naming | 2.1's Qwen3-VL 8B as a chat model | yes | — |
| Fill from a reference | 2511 fill-ref | no (ComfyUI) | on 2.1 edit with references or a Krea 2 edit LoRA (R2) |
| Image from references | 2511 image-ref | no (ComfyUI) | the same (R2) |
| Generative Angles | 2511 + Multiple Angles LoRA | no (ComfyUI) | Krea 2 + `krea2_anygles` (installed) or 2.1 (R2) |
| Krea 2 style reference | Krea 2 + style LoRA | no (ComfyUI) | the Krea 2 reference path exists now; the style LoRA (R2) |
| Split into layers | Qwen-Image-Layered | no (ComfyUI) | port (R3) |
| Generative Upscale | 2.1 edit tiles | yes | — |

## 3. The queue

Owners: **Claude** writes kernels, model code, integration and decides; **Codex** (`codex exec`,
read-only or in its own worktree) runs the parallel grind: audits, porting specs from the
reference code, parity dump scripts, test scaffolds, reviews of Claude's commits. Every Codex
result is checked by Claude before anything lands. Measurements run on the one GPU, one at a
time.

| # | Work | Owner | Gate |
|---|---|---|---|
| R0 | Inventory of every Python / ComfyUI / ONNX Runtime dependency at run, build and setup time | Codex (audit), Claude (check) | the table in §2 confirmed with file:line |
| R1 | **SAM 3.1 on candle** (`gn-perception` without `ort`): the vision backbone, text encoder, geometry encoder, transformer, heads and mask decoder, read from ComfyUI's checkpoint | Codex (porting spec, dump scripts), Claude (port) | masks at IoU ≥ 0.99 against the ONNX route on the harness's `sam` suite; select.byText as fast or faster; `ort` out of the worker |
| R2 | **The 2511 holdouts on native routes**: fill from a reference and image from references (2.1 edit takes nine references; Krea 2 edit LoRAs take two), Generative Angles (Krea 2 + `krea2_anygles`, or 2.1), the Krea 2 style reference | Claude; Codex reviews | each beats or matches its 2511 route on a small benchmark like the swap set; then `auto` sends them native |
| R3 | **Qwen-Image-Layered native** (`generate.split`) | Codex (porting spec), Claude (port) | the split's layers at the harness's rule against the ComfyUI route |
| R4 | **One models folder in ComfyUI's layout**: the worker resolves each model from `diffusion_models/`, `text_encoders/`, `vae/`, `loras/` by file name (Krea 2 already runs from ComfyUI's fp8 files; 2.1 still wants a diffusers snapshot), the first run defaults to ComfyUI's folder when one exists, a "Get models" page of links | Claude; Codex writes the resolver tests | the walkthrough with `integrations.modelsPath` = ComfyUI's `models` folder and nothing else |
| R5 | **Packaging**: the runtime folder without ONNX Runtime and cuDNN, the clean-profile install, the walkthrough from the zip | Claude; Codex scripts the clean-profile check | §1.2, §1.4 |
| R6 | **Native by default**: `integrations.engine` defaults to native when the worker has its folders; the ComfyUI sidecar only when asked; the permissive build's defaults decided with the owner (2.1 is research-only) | Claude, the owner decides the licence question | §1.1 |

Order: R0 → R1 and R3 specs in parallel (Codex) while Claude does R2 (no new models needed) →
R1 port → R4 → R3 port → R5 → R6.

## 4. Rules for this task

Everything in `CLAUDE.md` and `AGENTS.md` holds: never crash, no `unsafe` outside the kernel
FFI, tests are the gate, BYOM (no weights shipped), research-only models never a permissive
default, commits local, genai-native not pushed without the owner, one commit per logical
step, the dev log and roadmap updated.

## 5. Progress log

- 2026-10-27, evening: task opened. Native today: Krea 2 text to image and edit, every
  Qwen-Image-2.1 route (image, fill, edit, expand, matte, removals, upscale, the rewriter), the
  engine page and GPU detection. ComfyUI is still needed for the four 2511 holdouts, the
  Layered split and the Krea 2 style reference; ONNX Runtime and cuDNN for SAM 3.1. Codex runs
  R0, the R1 spec and the R3 spec in parallel (read-only).
