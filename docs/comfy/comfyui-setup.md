# ComfyUI: local server setup and API primer

Written 2026-10-08 for the development PC (Windows 10, RTX 5090 32 GB). ComfyUI is the generative
backend Comfy PhotoCraft talks to; PhotoCraft never imports PyTorch or any Python. The server runs
as a separate process on loopback and PhotoCraft is one of its API clients.

## 1. Which ComfyUI to install

| Option | When | Notes |
|---|---|---|
| **Portable build** (`ComfyUI_windows_portable`, from the GitHub releases) | recommended for development | Embedded Python + CUDA PyTorch, updated with the bundled scripts; you control the version. Blackwell (RTX 50-series) needs a CUDA 12.8+ PyTorch, which current portable builds ship. |
| **ComfyUI Desktop** | for end users | Installer, bundled runtime, auto-updates on the *stable* channel, which lags the Git release by days to weeks (Qwen-Image-2.1 and Krea 2 needed a fresh release when they landed). |
| **Git clone + venv** | if you already keep Python | Needs Python 3.12/3.13. This PC has **no Python** (only the Microsoft Store stub), so this is the least convenient option here. |
| **Pinokio** | one-click | Pinokio is installed on this PC (`C:\Users\5090\AppData\Local\Programs\Pinokio`) with no apps yet; its ComfyUI script installs a conda env and the server. Fine for trying things, less controllable for API work. |

Minimum versions: **ComfyUI ≥ 0.37.0** for Qwen-Image-2.1 (released 2026-09-20),
**≥ 0.26.0** for Krea 2 (2026-06-23). Always run a recent release; the official docs say to use
nightly if a workflow's nodes are missing.

## 2. Launch flags

```bash
python main.py --listen 127.0.0.1 --port 8188 --preview-method auto
```

- Keep the server on **loopback** (`127.0.0.1`, the default). PhotoCraft's own control channel is
  loopback-only with a bearer token for the same reason; ComfyUI has no authentication, so a
  `--listen 0.0.0.0` exposes every model on the machine to the LAN.
- `--preview-method auto` makes the server stream low-resolution previews over the WebSocket while
  sampling (Phase 2 shows them on the canvas).
- `--lowvram` / `--highvram` are not needed on 32 GB; leave memory management to the server.
- Base URL assumed throughout: `http://127.0.0.1:8188`. It becomes the preference
  `integrations.comfyServer` (Edit › Preferences › AI Integrations…) in Phase 1.

## 3. Model files

Paths are relative to the ComfyUI install (`ComfyUI/models/`). Every file is a plain
`.safetensors`; download them from the Hugging Face pages in [`models.md`](models.md). Gated repos
(`krea/Krea-2-*`) need a Hugging Face account that accepted the licence; the `Comfy-Org/Krea-2`
repack is not gated.

| Purpose | Folder | File |
|---|---|---|
| Krea 2 Turbo (FP8, recommended) | `diffusion_models/` | `krea2_turbo_fp8_scaled.safetensors` |
| Krea 2 Turbo for style reference | `diffusion_models/` | `krea2_turbo_int8_convrot.safetensors` |
| Krea 2 style reference LoRA | `loras/` | `krea2_style_reference.safetensors` |
| Krea 2 style LoRAs (optional) | `loras/` | e.g. `krea2_softwatercolor.safetensors` |
| Krea 2 text encoder | `text_encoders/` | `qwen3vl_4b_fp8_scaled.safetensors` |
| VAE (shared by Krea 2 and Qwen-Image) | `vae/` | `qwen_image_vae.safetensors` |
| Qwen-Image-Edit-2511 (Apache-2.0 editor) | `diffusion_models/` | `qwen_image_edit_2511_fp8mixed.safetensors` (or `_bf16`) |
| Qwen-Image text encoder (2511 line) | `text_encoders/` | the Qwen2.5-VL-7B encoder file from the 2511 template |
| Qwen-Image-2.1 (research licence) | `diffusion_models/` | the int8 file from the official 2.1 template |
| Qwen-Image-2.1 text encoder | `text_encoders/` | the Qwen3-VL-8B encoder file from the 2.1 template |
| Z-Image Turbo (Apache-2.0) | `diffusion_models/` | Comfy-Org BF16 release |
| SAM 3.1 (select by text, mattes; SAM License) | `checkpoints/` | `sam3.1_multiplex_fp16.safetensors` from `Comfy-Org/sam3.1` (≈ 1.75 GB; native nodes, templates "SAM3: Image Segmentation" under Utility) |
| Background removal | custom nodes' own folders | BiRefNet weights (MIT) |

The exact encoder file names for the Qwen templates are in the template's "Download models"
panel inside ComfyUI (Workflow › Browse Templates); copy them from there rather than from this
page, which will go stale. Record what you installed in [`devlog.md`](devlog.md).

## 4. Smoke test

PowerShell, with the server running:

```powershell
Invoke-RestMethod http://127.0.0.1:8188/system_stats | ConvertTo-Json -Depth 5
```

Expect `system.comfyui_version`, `system.python_version` and a `devices[]` entry naming the RTX
5090 with `vram_total` / `vram_free`. Then:

```powershell
(Invoke-RestMethod http://127.0.0.1:8188/object_info).PSObject.Properties.Name | Select-String -Pattern "Krea|QwenImage"
```

lists the Krea 2 and Qwen-Image node classes, which proves the server version is new enough.

## 5. API primer

Everything PhotoCraft needs is in the self-hosted server's routes. Field names below come from the
official route and message docs where they are documented and from `server.py` conventions (widely
used by every client library) where they are not; both are stable across 2024–2026.

### HTTP

| Route | Use | Request | Response |
|---|---|---|---|
| `GET /system_stats` | health check, version, VRAM | — | `{"system": {"os", "python_version", "comfyui_version", …}, "devices": [{"name", "type", "vram_total", "vram_free", "torch_vram_total", "torch_vram_free"}]}` |
| `GET /object_info` · `GET /object_info/{class}` | which nodes exist, their inputs and value lists (model files appear as combo options of the loader nodes) | — | `{ "<ClassName>": {"input": {"required": {...}, "optional": {...}}, "output": [...], "name", "display_name", "category"} }` |
| `GET /models` · `GET /models/{folder}` | model files per folder | — | arrays of file names |
| `POST /upload/image` | send the source image | multipart form: `image` (file), `subfolder` (optional), `type` (`input`), `overwrite` (`true`/`false`) | `{"name", "subfolder", "type"}` |
| `POST /upload/mask` | send a mask for an uploaded image | multipart form: `image` (file), `original_ref` (JSON string `{"filename","subfolder","type"}`), `type` | as above |
| `POST /prompt` | queue a workflow | JSON `{"prompt": <API-format graph>, "client_id": "<uuid>", "extra_data": {...}}` | `{"prompt_id", "number", "node_errors"}`; on validation failure `{"error": {...}, "node_errors": {...}}` |
| `GET /prompt` · `GET /queue` | queue state | — | `{"queue_running": [...], "queue_pending": [...]}` |
| `POST /queue` | clear or delete queued prompts | `{"clear": true}` or `{"delete": ["<prompt_id>"]}` | — |
| `POST /interrupt` | stop the running prompt (our **cancel**) | — | — |
| `GET /history/{prompt_id}` | outputs of a finished prompt | — | `{"<prompt_id>": {"prompt": [...], "outputs": {"<node_id>": {"images": [{"filename", "subfolder", "type"}]}}, "status": {"status_str", "completed", "messages"}}}` |
| `GET /view?filename=…&subfolder=…&type=output` | fetch an output file (PNG bytes) | query params | image bytes |
| `POST /free` | unload models to free VRAM | `{"unload_models": true, "free_memory": true}` | — |

### WebSocket

Connect to `ws://127.0.0.1:8188/ws?clientId=<same uuid as client_id>`. Text frames are JSON
`{"type": "...", "data": {...}}`:

| `type` | When | `data` fields |
|---|---|---|
| `status` | queue changes | `status.exec_info.queue_remaining` |
| `execution_start` | a prompt starts | `prompt_id` |
| `execution_cached` | nodes skipped because their outputs are cached | `prompt_id`, `nodes` |
| `executing` | a node starts; `node: null` means the prompt finished | `node`, `prompt_id` |
| `progress` | per sampling step | `node`, `prompt_id`, `value`, `max` |
| `executed` | a node produced UI output (e.g. SaveImage) | `node`, `prompt_id`, `output` |
| `execution_error` | failure | `prompt_id` plus error details (`node_id`, `node_type`, `exception_message`, `traceback`, …) |
| `execution_interrupted` | after `/interrupt` | `prompt_id`, `node_id`, `node_type`, `executed` |
| `execution_success` | all nodes done | `prompt_id`, `timestamp` |

Binary frames carry previews: a 4-byte big-endian event type (`1` = preview image), a 4-byte
image type (`1` = JPEG, `2` = PNG), then the image bytes. Previews arrive only when a preview
method is enabled.

### The flow PhotoCraft implements

1. Create a `client_id` (UUID v4), open the WebSocket with it.
2. `POST /upload/image` for the source composite crop and, for inpainting, `POST /upload/mask` for
   the selection mask (or upload the mask as a second image and reference it with `LoadImage`).
3. Fill a workflow template: set the `LoadImage` file names, the prompt text, seed, steps, model
   file names, output prefix. `POST /prompt` with the API-format graph and the `client_id`.
4. Read `progress` and `executing` messages to drive the job's progress bar; `jobs.cancel` calls
   `POST /interrupt`; a dropped socket falls back to polling `/history/{prompt_id}`.
5. On `execution_success` (or `executing` with `node: null`), `GET /history/{prompt_id}`, then
   `GET /view` for each `images[]` entry of the SaveImage node.
6. Decode the PNG with `photocraft-codecs`, build the new layer and mask, apply as one undo step.

### API-format workflows

ComfyUI stores workflows in two shapes. The **UI format** (what Save writes) carries node
positions and widgets; the **API format** is the flat `{"<node_id>": {"class_type", "inputs": {...}}}`
map that `/prompt` accepts. Export it from the app with **Workflow › Export (API)** (older
frontends: enable *Dev mode* in Settings first). PhotoCraft ships API-format templates with
placeholder values and never sends the UI format.

### Client libraries, for reference

Rust: `comfyui-client` (HTTP + `/ws` event stream, MulanPSL-2.0) and `comfyui-rs` (async, typed
REST, WebSocket progress with polling fallback). Neither is wasm-capable or in our dependency
policy; they are useful as reference implementations. The official docs' "API examples" page and
the Krita plugin (`Acly/krita-ai-diffusion`, Python) show the same upload → prompt → ws → history
→ view loop. See [`architecture.md`](architecture.md) for why we write our own thin client.

## Sources

- [ComfyUI server routes (official)](https://docs.comfy.org/development/comfyui-server/comms_routes) · [WebSocket messages (official)](https://docs.comfy.org/development/comfyui-server/comms_messages) · [API examples (official)](https://docs.comfy.org/development/comfyui-server/api-examples)
- [Krea-2 workflow (official)](https://docs.comfy.org/tutorials/image/krea/krea-2) · [Qwen-Image-Edit-2511 workflow (official)](https://docs.comfy.org/tutorials/image/qwen/qwen-image-edit-2511) · [Qwen-Image workflow (official)](https://docs.comfy.org/tutorials/image/qwen/qwen-image)
- [Hosting a ComfyUI workflow via API (9elements)](https://9elements.com/blog/hosting-a-comfyui-workflow-via-api/) · [ComfyUI API endpoints (runflow)](https://www.runflow.io/blog/comfyui-api-endpoints)
- [comfyui-client (docs.rs)](https://docs.rs/comfyui-client) · [comfyui-rs (docs.rs)](https://docs.rs/comfyui-rs)
- [Krita AI Diffusion (Acly)](https://github.com/Acly/krita-ai-diffusion)
