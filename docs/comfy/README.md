# Comfy PhotoCraft

**A Firefly-class generative toolset for PhotoCraft, fully local and fully open source.**

Comfy PhotoCraft is a fork of [storytold/photocraft](https://github.com/storytold/photocraft) (the
clean-room, pure-Rust Photoshop reimplementation) whose purpose is to close the one gap upstream
has deliberately deferred: **AI / generative features** (`docs/roadmap.md` grades them "~0%,
deferred by decision (#41)"). The generative engine is a locally running
[ComfyUI](https://github.com/comfyanonymous/ComfyUI) server driving open-weight models such as
**Krea 2**, **Qwen-Image-2.1 / Qwen-Image-Edit** and friends, so that Generative Fill, Generative
Expand, text-to-image, instruction editing and background removal run on the user's own GPU with
no account, no cloud and no credits.

This folder is the fork's own documentation. Everything upstream wrote still applies: read
[`AGENTS.md`](../../AGENTS.md) first, then the upstream `docs/`, then this folder.

| Document | What it is for |
|---|---|
| [`codebase-orientation.md`](codebase-orientation.md) | The parts of the ~250k-line workspace a generative feature touches: command anatomy, pixel access, jobs, preferences, UI hooks, automation, tests. Cited to `file:line`. |
| [`architecture.md`](architecture.md) | The design: the `photocraft-genai` crate, the ComfyUI client, workflow templates, the `generate.*` commands, the UI, and how it stays inside upstream's rules. |
| [`models.md`](models.md) | The model catalogue: what each open model does, its licence, VRAM, ComfyUI support and file names. Includes the Firefly feature → model mapping. |
| [`comfyui-setup.md`](comfyui-setup.md) | Running ComfyUI locally on this machine (RTX 5090), the model files to download, and an API primer (HTTP + WebSocket) with exact field names. |
| [`roadmap.md`](roadmap.md) | Phases 0–6 with a definition of done each, ordered so every phase ships something usable. |
| [`dev-environment-windows.md`](dev-environment-windows.md) | What is installed on the development PC, how it was installed, build and test commands, timings and gotchas. |
| [`upstream.md`](upstream.md) | Fork hygiene: syncing with upstream, what to contribute back, brand-licence obligations, naming. |
| [`devlog.md`](devlog.md) | Dated log of what landed and what is open, so the next session (human or agent) can pick up. |

## Vision

Adobe's generative features are the reason many people cannot leave Photoshop. Every one of them
is a *workflow* over a diffusion model: select, describe, generate, pick a variation, keep it on
its own masked layer. PhotoCraft already has the hard parts of that workflow: a document model with
layers and masks, a selection system, a command engine where every action is scriptable, a
background-job system with progress and cancellation, and a thin UI that is generated from data.
What is missing is a backend that turns "prompt + pixels + mask" into pixels, and the handful of
commands and panels that wire it in.

ComfyUI is that backend. It is the de-facto standard runtime for open image models, it gets
day-one support for new models (Krea 2 landed in ComfyUI 0.26.0 the day after its release;
Qwen-Image-2.1 the day of), it runs on the user's own GPU, and it exposes a small, stable HTTP +
WebSocket API. Treating it as an external service keeps PhotoCraft pure Rust (no PyTorch, no CUDA
bindings, no C dependencies) and keeps the browser build intact.

## Principles (in addition to upstream's)

1. **Local and open by default.** No cloud, no accounts. Models the UI offers by default must be
   open weights with a licence that permits what the user is doing; the catalogue says which
   licence applies ([`models.md`](models.md)).
2. **Everything is still a command.** `generate.fill`, `generate.expand`, `generate.image`,
   `generate.edit`, `generate.removeBackground` are engine commands with params docs, `enabled`
   predicates, tests and `panic_hunt` coverage. The UI, CLI, control channel and MCP dispatch them
   like any other command. Agents can drive generative edits headlessly.
3. **Non-destructive.** A generation lands on a new raster layer with a layer mask cut from the
   selection, exactly like Photoshop's generative layers. The original pixels never change. Prompt,
   seed, model and workflow are recorded on the layer so a result can be reproduced or varied.
4. **The backend is a trait.** ComfyUI is the first `GenerativeBackend`. A native inference
   backend (candle/burn) or another server can come later without touching the commands or UI.
5. **Never block, never crash.** Generation runs as a background job (`crates/engine/src/jobs.rs`)
   with progress from ComfyUI's WebSocket and cancellation that interrupts the server. A server
   that is down, slow, misconfigured or returns garbage produces an error the user can act on.
6. **Upstream-compatible.** The fork tracks upstream `main`. Generic improvements go back upstream
   as PRs; the generative crate is designed so it could be upstreamed as a whole once #41 is
   decided.

## Firefly feature parity matrix

The target list is Photoshop 2026 (v27.x) as documented by Adobe and the community in 2026
(sources in [`models.md`](models.md) › Sources). "Phase" refers to [`roadmap.md`](roadmap.md).

| Photoshop feature | What it does | Comfy PhotoCraft plan | Default model (licence) | Phase |
|---|---|---|---|---|
| Generative Fill | Inpaint the selection from a prompt; result on a masked layer; several variations | `generate.fill`: composite + selection mask → edit/inpaint workflow → new layer + mask | Qwen-Image-Edit-2511 (Apache-2.0); Qwen-Image-2.1 (research) | 1–2 |
| Generative Expand | Outpaint when the canvas/crop grows | `generate.expand`: pad composite, mask the new area, same workflow | same as Fill | 3 |
| Generate Image | Text-to-image into a new document/layer, reference image optional | `generate.image` | Krea 2 Turbo (Krea 2 Community); Z-Image Turbo (Apache-2.0) | 3 |
| Generate Background | Replace the background behind the subject | `generate.removeBackground` + `generate.fill` of the inverse | Qwen-Image-2.1 RGBA / BiRefNet + editor | 3 |
| Remove tool / Distraction Removal | Remove an object, fill plausibly | `generate.edit` with "remove …" + mask; classical `edit.contentAwareFill` stays as fallback | Qwen edit models | 2–3 |
| Reference Image / Style reference | Steer a generation with an image | `generate.image` / `generate.edit` with `references[]` | Krea 2 style-reference LoRA; Qwen-Image-2.1 (up to 10 refs) | 4 |
| Harmonize | Match colour, light and shadows of a pasted layer to the scene | `generate.edit` preset ("harmonize") on the layer with the composite as context | Qwen edit models | 5 |
| Generate Similar | More variations of a chosen result | re-run with the stored prompt/seed family | any | 2 |
| Generative Upscale | Enlarge up to 4× with new detail | `generate.upscale` through an upscale-model workflow | SeedVR2 / ESRGAN-family (verify licence) | 5 |
| Partner models in the picker | Choose a model per generation | model picker in the task bar, fed by the catalogue + server `object_info` | all installed | 2 |
| Generative Workspace / AI Assistant | Ideation canvas, conversational panel | out of scope for now; MCP already lets any agent drive the app | — | — |

## Status

- 2026-10-08: Phase 0 (this documentation, the Windows environment, the `comfy-photocraft`
  branch). The release build and test suite were run on the development PC; see
  [`devlog.md`](devlog.md). No generative code exists yet.
