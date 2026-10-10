# ArtCraft's paid features, and how they do things: a study for PhotoshopEX

Written 2026-10-10 from getartcraft.com (home, pricing, apps, FAQ, support, tutorials), the
public `storytold/artcraft` repository at its 2026-10 head (a shallow clone read locally), the
team's Hacker News and Reddit posts, and the Omni API guide in the repo. A write-up only; nothing
in the app changed. The last section says what to adopt and in which order.

## 1. What ArtCraft is, in one paragraph

ArtCraft is the storytold team's "IDE for artists": a Tauri desktop app (Rust core, React
frontend, SQLite task database) and a web app that **aggregate cloud generation models** behind
one prompt box, plus a 2D canvas editor (draw, inpaint, background removal) and a 3D stage
(posing, blocking, image-to-mesh, Gaussian-splat worlds). It is open source (MIT or Apache-2.0),
free to run, and makes money from **credits** spent on the models ArtCraft hosts. PhotoCraft and
the other Crafting Apps are their offline, local editors; today they contain no ArtCraft
generation and no account, credits or cloud calls.

## 2. The paid features

### What costs money

Only **generation through ArtCraft's own provider** ("ArtCraft" in their provider enum). The
editing tools, the canvas, the 3D stage, the task queue and the gallery are free. Every
ArtCraft-provided model has a credit price; the Generate button shows it.

| Plan (2026-10) | Price | Credits a month | What the page says it buys |
|---|---|---|---|
| Free | $0 | daily free generations | "~10 GPT-Image-1 images, ~1 minute Kling video", limited tools |
| Basic | $8/mo ($96 a year) | 1,000 | ~250 Nano Banana, ~67 Nano Banana Pro, ~33 Pro 4K, ~200 GPT-Image-1.5, ~63 s Seedance 2.0, ~45 s Kling 3.0 Pro |
| Pro | $28/mo ($336 a year) | 3,750 | 3.75× Basic |
| Max | $48/mo ($576 a year) | 6,600 | 6.6× Basic |
| Enterprise | custom | bespoke | "secure models", SLAs, custom integrations |

Credit packs are one-off top-ups that never expire. **1 credit = $0.01** (their cost modal says
so). Implied per-model prices, from the plan maths: about 4 credits a Nano Banana image, 15 a
Nano Banana Pro, 30 at 4K, 5 a GPT-Image-1.5, about 16 a second of Seedance 2.0 and 22 a second
of Kling 3.0 Pro. **Credits are refunded when a generation fails**; the desktop app refetches the
balance two seconds after a failure event so the refund shows.

The team says the flagship models run at "zero-margin defaults" on the web app to build volume;
the Hacker News post claims a third of customers spend thousands a month. The roadmap's first
architecture goal is "remove dependence on ArtCraft-hosted services".

### What is free: bring your own

The desktop app lets a user attach **their own accounts and keys**, which then bypass credits:

- **API keys**: Fal, Replicate, Google and others, stored by the desktop credential cache
  (`provider_set_api_key_command`, `ProviderCredentialPayload::ApiKey`).
- **Logins** to consumer services through an embedded browser session with cookie access (their
  forked `tauri-plugin-http`): Midjourney and Grok today; Runway, Higgsfield, OpenArt, Magnific,
  Freepik, Leonardo, Picsart, PixVerse, Krea listed in the provider enum as coming.
- A Midjourney job costs 0 ArtCraft credits and is marked `is_rate_limited` instead: paid by the
  user's own subscription.

The 62-model catalogue (16 image, 25 video, 5 audio, 11 mesh, 5 splat; Nano Banana, GPT Image,
FLUX, Seedream, Seedance, Kling, Veo, Sora, Vidu, MiniMax, Suno, Hunyuan 3D, Tripo, Meshy, Rodin,
Marble…) is reachable through ArtCraft credits, through a user's Fal key for most image models,
through "Kinovi" for a few, and through the consumer logins. Local models: none yet; the team
"plans to make ArtCraft work with local models" and a variant, ArtCraft-X, is meant to drive
"other subscriptions (Runway, Higgs, OpenArt, Comfy…)". Their reasoning for cloud first: top
models "need more VRAM" than desktops have.

### The Omni API: ArtCraft as a provider for other apps

`_docs/artcraft_omni_api.md` documents an API-key surface on `api.storyteller.ai`:
`POST /v1/omni_api/generate/image` and `/video` with `model`, `prompt`, `aspect_ratio`,
`resolution`, reference image/video/audio URLs or media tokens, a required `idempotency_token`
(UUID), then `GET /v1/omni_api/job_status/job/{token}` polled until a terminal state
(`complete_success`, `complete_failure`, `dead`, `cancelled_by_user`, `cancelled_by_system`),
with the result at `maybe_result.media_links.cdn_url`. `402` means no credits. Keys are prefixed
`artcraft_api_` and **API access is enabled per account by their staff**. This is the one way
PhotoshopEX could use a user's paid ArtCraft credits directly.

## 3. How they do things: the patterns

Read the source for these; each is a design decision we can copy without copying code.

### 3.1 Models are data, with capabilities, not code paths

`frontend/libs/model-list` holds a `Model` class (id, `tauriId`, full name, creator, selector
name/description/badges such as "10 sec.", tags, providers, `progressBarTime`, max prompt
length) and an `ImageModel` subclass of **capability flags the UI reads**: `canTextToImage`,
`canEditImages`, `usesInpaintingMask`, `editingIsInpainting`, `canUseImagePrompt` +
`maxImagePromptCount`, `canEditAngles`, `canChangeAspectRatio` + the list, `canChangeResolution`
+ the list (1K/2K/4K), `qualityOptions`, `maxGenerationCount` / `defaultGenerationCount` /
`predefinedGenerationCounts`. Tags: `instructiveEdit`, `maskedInpainting`,
`nonMaskedInpainting`, which decide **which editor pages a model appears on**.

The backend's listing is the source of truth for capabilities; the static list only adds
presentation and page flags (`buildModelsFromListing.ts`: "API capability fields ALWAYS win
over the overlay"). Membership in the picker comes from what providers currently *offer*
(the backend's publish switch), so a model can be catalogued yet hidden, and models the app has
never heard of still appear. Admin-only variants are filtered client-side.

### 3.2 One request shape per modality, provider chosen late

`OmniRequest` is `{provider?, frontend_caller?, frontend_subscriber_id?,
frontend_subscriber_payload?, …fields}`: the desktop keeps only its own metadata typed and
passes every API field through untouched ("including model IDs and option strings introduced
after this version of the app"), with a small rename layer per modality (`batch_size` →
`image_batch_count`). Provider dispatch happens in the Rust command: ArtCraft → the Omni
endpoint, Midjourney → the login client, everything else → `artcraft_router`, which has one
module per **(provider, model)** pair (`providers/fal/nano_banana_pro`, `providers/artcraft/…`).
A persisted **provider priority** list (`[Artcraft, Sora, Fal]` by default) is the fallback
order; a missing credential yields a typed error (`NeedsFalApiKey`, `NeedsMidjourneyCredentials`)
that the frontend turns into the "Set up Midjourney" modal.

### 3.3 Everything is a task in a local queue

Every enqueue inserts a row in a desktop SQLite `tasks` table: status (`pending`, `started`,
`complete_success`, `attempt_failed`, `dead`, `cancelled_by_user`, `cancelled_by_provider`,
`cancelled_by_us`), task type, model, provider, provider job id, the prompt token, the caller
page and an opaque **subscriber id/payload** the frontend set, dismissal, and on completion the
batch token, primary media token/class, CDN URL and thumbnail template; on failure a typed
reason and message. Background threads poll providers and emit Tauri events
(`generation-complete-event`, `generation-failed-event`, `credits_balance_changed`). Pages
**filter completions by their own subscriber id**, so a canvas resolves only the placeholders it
enqueued.

The **Task Queue** popover in the top bar (badge = in-progress + unread completed) lists In
Progress / Failed / Completed cards with thumbnails (the first reference image dimmed while
running), a fake progress bar driven by the model's `progressBarTime`, "~ 45s left", the prompt
in a marquee with a copy button, typed failure labels ("Images with faces are not allowed",
"Text prompt violates content policy"…), dismiss, clear stale/failed/completed, nuke all.

### 3.4 The prompt box

One component per modality (`PromptBoxImage`, `PromptBoxEdit`, `PromptBoxVideo`,
`PromptBox3D`, `PromptBoxAudio`), each built from the selected model's capabilities: a growing
textarea with a character counter against the model's limit and a fullscreen mode; a
**reference deck** (drag-and-drop, paste, upload or "pick from library", reorder, cap from
`maxImagePromptCount`, hidden when the model cannot take references); a model pill (creator
icon + name) opening a rich picker grouped by family with badges, descriptions and a provider
submenu; aspect ratio, resolution and quality pickers shown only when the model supports them;
a generation count picker limited by the model; Clear all; and a **Generate button that carries
the credit cost** (coin icon + number, a tooltip "15 credits cost", 0 for a user-paid
Midjourney). Enter to generate is a preference. Cost comes from a cost-estimate command run
with the same request fields (`cost_in_credits`, `cost_in_usd_cents`, `is_free`,
`is_unlimited`, `is_rate_limited`, `has_watermark`). A Cost Breakdown modal converts credits to
the user's currency.

### 3.5 The canvas editor and history

The 2D editor (`pagedraw`) is a Konva scene store: draw nodes (shapes and strokes), a separate
**inpaint mask layer** of strokes with Add/Erase and a size slider, a base image, and a
`historyImageNodeMap` that remembers the drawing state per generated image. A generation returns
an **image bundle** (one tile per batch image) that goes into a **History Stack** beside the
canvas: pick a tile to make it the new base image, remove tiles, remove pending placeholders.
The edit prompt box adds a mode switch (edit vs masked inpaint, from the model's tags), "Fit",
undo/redo and a system-prompt toggle. Background removal is a separate enqueue whose
completion event replaces the node's image.

### 3.6 Credits, billing and providers in the UI

A `credits` store holds free, monthly and banked credits and a status badge for slow or failed
fetches; the balance refreshes on enqueue and on failure. Billing lives in a settings modal pane
with Stripe checkout and a customer portal; the pricing modal is the plan table above. Provider
setup is a modal reached from the typed credential errors.

### 3.7 Engineering conventions that show

Two-space Rust, `maybe_` prefixes for optionals, one Tauri command per file, constants → types →
Request/Response/Error → impls, callers above callees; enums that are stored as strings get
`to_str`/`from_str` with round-trip and length tests; a NOTICE for third-party material; a short
ROADMAP ("better than model aggregation websites", "remove dependence on ArtCraft-hosted
services", "don't be evil").

## 4. Where PhotoshopEX stands against that

| ArtCraft pattern | PhotoshopEX today | Gap |
|---|---|---|
| Model catalogue as data with capability flags | Templates are JSON with task, licence, `needsImage`/`needsMask`, model slots, defaults, prompt formats; `generate.models` lists them | No capability flags for references, aspect ratios, resolutions, counts; no "which pages" tags; no provider field |
| One request shape, provider chosen late | `Request {template, prompt, negative, seed, steps, guidance, image, mask, models, size, params}` to one `GenerativeBackend` (ComfyUI) | No provider concept; everything is the local server |
| Local task queue with typed statuses and failure reasons | Engine background jobs (one per command), the status bar, the task bar's progress; nothing persists | No queue panel, no history of runs, no typed failures, nothing survives a restart |
| Prompt box from capabilities, cost on the button | The task bar: Fill/Edit mode, prompt, template picker with installed badges, variations, Enhance, Generate | No references, no aspect/size pickers, no time or cost estimate on the button |
| History stack of candidates beside the canvas | Variations become hidden layers with a 1/3 switcher; `GenerativeInfo` on layers; Generate Similar | No persistent gallery of past results; candidates are layers, which is right for an editor |
| Inpaint mask with Add/Erase | Photoshop's selection tools (far richer) feed Fill; soft dithered edges | None here; we are ahead |
| BYO accounts and keys, credits | Nothing; BYOB means bring your own ComfyUI | No cloud path at all |
| Enhance prompts | Shipped yesterday (Qwen3-VL 4B rewriter) | ArtCraft has a system-prompt toggle but no visible rewriter in the desktop app |

## 5. What to adopt, in order

The aim the owner stated: "the implementation more like how they do things overall". The pieces
below keep the fork's rules (no shipped weights, local first, research models gated) and add
ArtCraft's shape. Numbers are rough effort in sessions of this size.

1. **A model catalogue with capabilities** (1 session). Extend each template's `meta` with the
   flags the UI needs: `references {max}`, `aspectRatios`, `resolutions`, `counts {max,
   default}`, `pages` (fill, edit, image, expand, matte, split), `estimatedSeconds` (their
   `progressBarTime`), `creator`, `badges`, `description`. `generate.models` returns them; the
   task bar and the dialogs read them instead of hard-coding. This is the foundation for
   everything else and matches the fork's "models as data" roadmap phase.
2. **A persistent task queue and history** (2 sessions). An engine `generate.jobs` store (SQLite
   or a JSON file under the app data dir, as upstream's prefs are stored) with ArtCraft's
   statuses and failure categories, the prompt, the template, the provider, timings and the
   result layer ids; a Task Queue popover in the title bar with in-progress, failed and
   completed, dismiss and clear; "Recreate" on any row (we already have Generate Similar). The
   job events exist; this records them.
3. **The prompt box, ArtCraft-shaped** (1–2 sessions). Grow the task bar into a prompt panel:
   a multi-line prompt with the model's character limit, a reference deck (drag in images or
   pick layers: Qwen-Image-2.1 takes up to 10 references, Krea 2's style reference LoRA one),
   aspect/size pickers for Generate Image, count, and a Generate button showing the estimated
   time from the template's `estimatedSeconds` and the request size (we measured these). Local
   generation has no credit cost; show seconds where they show coins.
4. **Providers** (2–3 sessions). Introduce `GenerationProvider` in genai with the local ComfyUI
   as the first and default, then optional cloud providers behind **the user's own keys**: a
   Fal provider first (their router covers most image models through Fal: Nano Banana, GPT
   Image, FLUX Pro, Seedream), then the **ArtCraft Omni API** as a provider so a user with an
   ArtCraft subscription spends their credits from inside PhotoshopEX (needs API access enabled
   on their account; the job-status protocol is documented and simple). Each provider has a
   capability-flagged catalogue entry per model, a credential store (the OS keychain, never the
   prefs file), a typed "needs key" error that opens a setup dialog, and the cost estimate on
   the button when a provider charges. Provider priority with fallback, as theirs. This is the
   "paid features" part: nothing is paid to us, the user pays whom they already pay.
5. **Failure categories and refunds** (part of 2 and 4). Content-policy and provider failures
   mapped to plain labels; for cloud providers, refetch the balance after a failure.
6. **A results gallery** (1 session, later). A panel of past generations (thumbnails from the
   job store, click to re-open the layer or to place the image again), ArtCraft's grid/list
   views with "recreate", "download", "copy prompt".

Not worth copying: the fake progress bar (we have real progress from ComfyUI), the 3D stage
(FilmCraft/EffectCraft territory), consumer-login scraping of Midjourney and Grok (fragile, and
against those services' terms), telemetry.

## 6. Open questions for the owner

- Cloud providers at all? The fork's principle so far was "fully local". Fal keys and the
  ArtCraft Omni API are both BYO-key and optional; the default stays the local server.
- Which first: the task queue (visible every day) or the provider layer (the paid part)?
- ArtCraft API access is staff-gated per account; ask them on Discord whether a PhotoCraft
  fork can get it, since the Crafting Apps are their own family.

## Sources

- getartcraft.com: home, `/pricing`, `/apps`, `/apps/photocraft`, `/faq`, `/support`,
  `/tutorials` (read 2026-10-10)
- github.com/storytold/artcraft: README (62-model catalogue), ROADMAP.md, AGENTS.md,
  crates/AGENTS.md, `_docs/artcraft_omni_api.md`, `_docs/dev_setup.md`,
  `_database/sql/artcraft_migrations/…create_tasks_table.sql`,
  `crates/schema/public/enums/src/common/generation_provider.rs`,
  `crates/desktop/artcraft/src/core/commands/{generate,cost_estimate,providers,task_queue,app_preferences}`,
  `crates/desktop/artcraft/src/core/state/provider_priority.rs`,
  `frontend/libs/model-list` (`Model.ts`, `ImageModel.ts`, `ModelTag.ts`, `ImageModels.ts`,
  `buildModelsFromListing.ts`), `frontend/libs/components/{promptbox,button,generation-list,
  pagedraw,model-selector,provider-setup-modal}`, `frontend/libs/state/{credits,subscription}`,
  `frontend/apps/artcraft/app/src/components/signaled/TopBar/TaskQueue.tsx`,
  `frontend/apps/artcraft/app/src/components/reusable/CostModal.tsx`
- Hacker News thread 49500019 (the founder on the open-source server, OpenSourceRouter, local
  models) and the r/… launch post of 2026-05 (BYOK: "You can add Fal, Replicate, Google, and
  other API keys"; Midjourney and Grok logins; cheapest-provider routing in progress)
