# Overnight goal, 2026-10-27: the removal that never touches ComfyUI

> **Mission.** By morning, a Spot Healing dab or a Remove stroke on the native worker
> (Qwen-Image-2.1, no ComfyUI) should beat the shipped 2511 route on the hard bench scenes.
> It should take shadows with the object, and the worker should answer its own questions fast.
> Then make the LoRA engine something the owner can see and trust. Every step is measured,
> tested and committed locally. Anything that needs the owner goes in the morning report, not
> into the code.

You are an autonomous agent working alone overnight in `C:\Users\5090\Documents\PhotoshopEX`
(branch `photoshopex`) and `C:\Users\5090\Documents\genai-native`. Nobody will answer a
question until morning. Do not ask. Decide, record why in the progress log at the bottom of
this file, and keep going.

---

## 0. For the owner, before bed (2 minutes)

1. Open a **new** Code session with its folder set to `C:\Users\5090\Documents\PhotoshopEX`.
2. Set the permission mode to **Auto**. In Default mode the first permission prompt stops the
   night.
3. Close PhotoshopEX if it is open. The agent rebuilds `target\release\photocraft.exe` and
   will not kill a running app.
4. Optionally, tick any pre-approval below. An unticked box means the agent does not do it.
5. Send this one message:

   ```
   Read docs/comfy/goals/overnight-2026-10-27.md and execute it to its stop condition. Work the queue in order, update the file's progress log after every task, and finish with the morning report.
   ```

**Owner pre-approvals** (change `[ ]` to `[x]` to allow):

- [ ] Download **big-lama** as ONNX (Apache-2.0, about 200 MB) from a Hugging Face repo the
      agent names in the log, so Q7 can run.
- [ ] Download the official **Qwen-Image-2.1 Turbo** checkpoint (Comfy-Org, about 7 GB,
      research licence) so Q8 can run.
- [ ] Use the **Viggle Turbo** LoRA (already installed, research licence) as a measured fast
      tier on the 2.1 route. It is never a default.

---

## 1. Ground rules (they override anything below)

**Never:**

- Push, open a PR, or change a remote.
- Download anything that is not pre-approved above.
- Accept a licence, sign in anywhere, or run an installer.
- Edit the owner's real `preferences.json`.
- Change a default the owner decided: 2511 stays the removal model, research models are never
  defaults, and Krea 2 stays on ComfyUI under `auto` unless the plan's "faster at parity" rule
  is measured true.
- Kill `photocraft.exe` or a process you did not start. The session-0 tenants (`sd-server`, SSH
  pythons) are not yours.
- Commit `Cargo.lock`. It stays modified because of the local `[patch]`.
- Bump genai-native's rev in `Cargo.toml`.

**Always:**

- Follow `AGENTS.md`: no `unwrap`, `expect`, `panic` or `unsafe`; everything is a command;
  never break wasm.
- Follow `CLAUDE.md` and the fork conventions in `docs/comfy/`.
- Translate every new UI string into all 20 languages. Make sure every new pref is read by
  code. Keep edits to shared upstream files surgical.
- Make one commit per logical step, written with `git commit -F <file>`. In PowerShell 5.1,
  `-m @'…'@` splits on quotes.
- **Tests are the gate.** Before each commit:
  - Run `cargo test -p photocraft-engine` and `cargo test -p photocraft-genai`, plus the crate
    you touched, or `photocraft-ui-egui` for UI work.
  - Run clippy and allow no new warnings.
  - Run `cargo xtask wasm` and `cargo xtask layers`.
  - In genai-native, run the touched crate's tests through `native-env.cmd`.
- A live claim needs a live run. Contact sheets are judged by looking at them with Read. Write
  each per-stroke verdict in a table, not just a total.

**When stuck:**

- Each task has a time box. When it runs out, write what you learned, commit what is sound
  (tests green), and move to the next task.
- If the same failure happens three times in a row (build, GPU wedged, worker dead), stop
  everything and write the morning report.

---

## 2. Setup (first 15 minutes)

```powershell
cd C:\Users\5090\Documents\PhotoshopEX
git branch --show-current            # must be photoshopex
git status --short                   # only Cargo.lock (and this file) may be dirty
git -C C:\Users\5090\Documents\genai-native log --oneline -3   # da60b3c or later
nvidia-smi                           # note the other tenants' memory before timing anything
Invoke-RestMethod http://127.0.0.1:8188/system_stats   # ComfyUI up? If not, start it as a
# background command (it never returns): powershell -NoProfile -File C:\Users\5090\ComfyUI\start-comfyui-fast.ps1
```

**Read first:**

- `CLAUDE.md`
- The top three entries of `docs/comfy/devlog.md`
- `docs/comfy/benchmarks.md` §5i, the removal measurements and "The rewriter's views per model"
- `docs/comfy/research/notes/ausboss-catalogue-2026-10.md` §2.2, §2.5, §2.6 and §4
- `docs/comfy/plan-native-engine.md` §9, for the LoRA and AnyPaint design

**The tools you will use:**

| Need | How |
|---|---|
| Build the CLI or app with the native engine | `docs\comfy\bench\scripts\build-native.cmd photocraft-cli` (or `photocraft`). Stop the worker first: it locks the exe. |
| Run genai-native's tests or tools | `docs\comfy\bench\scripts\native-env.cmd cargo test --release -p qir`, run from the genai-native folder |
| Shared worker for many CLI runs | `. docs\comfy\bench\scripts\native-worker.ps1` sets `$env:PHOTOCRAFT_NATIVE_ENGINE`. Stop it with `-Stop`. It takes one client at a time. |
| Rewriter naming in seconds | `naming-probe.ps1 -System docs\comfy\bench\scripts\removal-system.txt -Tags …` (the `chat_probe` example: `native-env.cmd cargo build --release -p photocraft-genai --example chat_probe` from PhotoshopEX) |
| Stroke benches | `robust-strokes.ps1` (stock set: `-Dir …\bench\stock -Prefix st -Objects st-objects.json`; robustness set: the defaults), `robust-sheets.ps1` for the contact sheets, `views-run.ps1` as the template of a whole A/B |
| Free ComfyUI's VRAM before native timing | `POST http://127.0.0.1:8188/free {"unload_models":true,"free_memory":true}` |

**Gotchas that cost hours before:**

- PowerShell variable names are case-insensitive: `$W` is `$w`.
- `--%` swallows the rest of its line.
- A long-lived child started from a piped call hangs the call. Redirect to a file.
- ComfyUI resident beside the worker makes native timings 3× slower.
- The 8B flips on a few pixels of crop difference, so confirm probe findings with real strokes.
- 2511 runs use random seeds. Run twice before claiming a one-point gain.

**The score:** a **clean removal** means the object is gone whole, with no smudge, ghost, new
object or blurred face, and the background is continued plausibly. The baseline is the ten
hard stock scenes, half and whole strokes (`glasses, watch, tattoo, cup, dog, boat, bicycle,
hat, umbrella, motorbike`):

| Route | Score of 20 |
|---|---|
| 2.1 on the worker, two judged (`-p21v`) | 15 |
| 2.1 on the worker, one candidate (`-e21v`) | 14 |
| 2511 shipped (`-auto6`) | 16 |

The robustness set (`rb`) is the no-regression check. A change ships only if it does not lose a
clean result on either set.

---

## 3. The queue (in order)

Each task lists its goal, a time box, hints and what "done" means. When a task ends, append a
dev-log entry (newest first), add a `benchmarks.md` row if anything was measured, update the
`roadmap.md` markers, and fill in its line in the progress log below.

### Q1. The mask's whole stroke on the 8B (90 min)

The worker's 8B names "the man wearing the cap and mask" on the real crop of the mask's whole
stroke, while the probe's crop gets "white face mask" 16 times of 16.

- First, make the probe see what the job sees. Add a bench-only env hook,
  `PHOTOCRAFT_DUMP_VIEWS=<dir>`, that saves the exact side-by-side picture the rewriter gets.
  The pattern is `PHOTOCRAFT_THINKER`: documented in benchmarks, read by code. Then probe
  wordings and margins against that exact picture.
- Candidate fixes, cheapest first:
  - A parser rule in `named_object`: `<person> wearing|holding|carrying <X>` → `<X>`, with
    tests.
  - Ask twice, at seeds s and s+1, and keep the answer that is not a person.
  - A margin of 0.05 for strokes under N px.
- **Done when:** the mask's whole stroke is clean on `-p21v` and the probe set still names all
  20 views correctly. Then run the full `-p21` A/B on both sets.
- **Target:** 2.1 two judged at ≥ 16/20, with no rb regressions.

### Q2. Remove it with its shadow: the differential composite (2.5 h)

This is catalogue §2.2. The dog's cast shadow stays on every route.

- **The 2.1 whole-picture route.** After the model returns, grow the layer mask by a change
  band:
  - Take |result − original| in luma and chroma, after realign-to-source.
  - Threshold it. Keep only the parts connected to the hole, within a search band of k × the
    hole's size. Close, fill holes, feather.
  - Add a `shadow: true|false` param on the removal commands (default true when the route
    supports it), tests on synthetic images (a disc and its offset dark ellipse), and
    `generate.info` recording the band.
- **The 2511 crop route.** Add a rewriter clause, or a second detector query
  (`select.byText "the <name>'s shadow"`), and union the result into the hole when the shadow
  touches the grown stroke. Measure whether SAM 3.1 finds shadows on the dog, cone and hydrant
  scenes (rb and stock).
- **Done when:** the dog's shadow is gone on at least one route and nothing regresses on either
  set. The cup's dark patch is the model continuing the table's shade (see the dev log), so do
  not chase it with this.

### Q3. The worker's chat KV cache (2 h, genai-native)

`Pipeline::chat` re-runs the whole sequence for every token.

- Add an incremental decode with a per-layer K/V cache. The positions follow Qwen3-VL's
  M-RoPE: image tokens get their 3-axis positions, and text after the image continues from
  the max position + 1, as in HF's `get_rope_index`.
- **Done when:**
  - Greedy output is token-for-token identical with and without the cache, on three prompts
    with a picture and one without (a test in `qir`).
  - A 30+ token answer is at least 3× faster per token.
  - `chat-smoke.ps1` shows the new times in the dev log.
- Commit in genai-native locally. PhotoshopEX picks the change up through the `[patch]`.

### Q4. LoRA parity against ComfyUI (90 min)

The LoRA engine landed on 2026-10-26 unmeasured.

- Run the same seed, size and prompt through both routes:
  - `qwen-image-2.1-consistency` on a 2.1 edit
  - `qwen-image-2.1-restore` on a 2.1 edit
  - `bfs_head_v1.1_qwen_2.1` on t2i
  - (if pre-approved) Viggle Turbo at 6 steps
- On the ComfyUI side, use a template with a `LoraLoaderModelOnly` node verified against
  `/object_info`, or the harness's API graphs. Compare with the parity harness's metrics.
- The reference point is the no-LoRA agreement of the same cases (the 2.1 rows in benchmarks
  §7–8).
- **Done when:** the LoRA agreement is within that baseline, or the gap is root-caused and
  fixed. Suspects are `txt_in` taking no adapter, the alpha/rank scale, and the gate-up split
  order. Check the worker's "unmatched module" log first.

### Q5. LoRA rows in the picker (2 h, UI)

Users cannot see the LoRAs yet.

- The Generate panel and the fill/edit bars get a LoRAs section: the files under
  `<models>/loras` that match the selected model's family, each with a switch and a strength
  slider from 0 to 1.5. The list comes from a command (`generate.loras {template}`) that reads
  the worker's index, or ComfyUI's `/object_info` `LoraLoaderModelOnly` list on that route.
  The rows feed `loras: [{file, strength}]`.
- The family comes from the file's tensor names, the way `gn-weights` identifies models. Do not
  go by the file name.
- Add the strings in 20 languages, a kittest UI test, and a command test against the fake
  server.
- **Done when:** a live `ui.inspect` shows the rows, and a run with one switched on lands in the
  Generations library with the LoRA recorded in `GenerativeInfo`.

### Q6. Generative Upscale (2.5 h)

This is catalogue §2.5 and roadmap Phase 5. The command is `generate.upscale {scale, tile?,
restore?}`:

- Resize to about 2 MP per tile, sides in multiples of 32.
- Tiles overlap by a shared strip and are joined with a feather.
- Each tile runs the edit route with the restore prompt from catalogue §2.5.
- The Restore LoRA is applied when installed, at strength 0.5 for blurry input.
- Each tile is matched to the source's colour statistics (mean and std per channel in Lab)
  before the join.
- It runs on 2.1 native, with 2511 as the fallback.
- Add an Image-menu entry, 20 languages, fake-server tests (tile count, seam weights, colour
  match), and one live run on a bench picture at 2×. Look at the seams at 200 % before
  claiming.

### Q7. LaMa as Spot Healing's first tier (2 h, needs its pre-approval)

This is catalogue §2.8. Without the tick, skip it and write the exact files and commands needed.

- Add a `heal/lama` route in `gn-perception` through `ort`. Use it for holes under N px; above
  that, the diffusion fill. Add a pref with a default, read by code, and a fake-engine test.
- **Done when:** a dust/wire set (make five strokes on the stock pictures) runs under 200 ms a
  dab, cleaner than PatchMatch on the sheet.

### Q8. The rest, if the night allows

Take these in order, each with its own time box of 90 minutes:

- **Tone match at the seam** for Expand and Fill (catalogue §2.4).
- **The prompting rules of catalogue §2.6** as rewriter A/Bs. Start with "what they hold goes
  too" on person removals.
- **The Krea 2 in-context reference pair**: the AnyPaint first step from plan §9, needing no
  model change.
- **2.1 Turbo as a second weight set**, only if pre-approved.
- **A `denoise` strength on fills.**

**Owner-only (never start; list them in the report):**

- Pushing genai-native and bumping its rev
- Making 2.1 the removal default (give the numbers and a recommendation)
- The fp8 GEMM kernel track (plan §11, a multi-day effort; write a design note at most)
- SeedVR2 and FLUX 3 Klein downloads
- Civitai workflow JSONs
- The VS Build Tools rerun

---

## 4. Stop condition

Stop when any one of these holds:

- The queue is done or time-boxed out.
- The three-strikes rule fires.
- It is 07:30 local time. Stop starting new GPU runs at 06:45 so the last one finishes.

Then do the following:

1. Rebuild `target\release\photocraft.exe` and `photocraft-cli.exe` with
   `build-native.cmd` so the pinned app has the night's work. If the app is running, build into
   `CARGO_TARGET_DIR=target-overnight` and say so.
2. Stop the worker. Leave ComfyUI running.
3. Run the full gate once: `cargo test --workspace --no-fail-fast`. The baseline is 9
   GPU-driver failures on this PC; list any others. Also run clippy, `xtask wasm`,
   `xtask layers` and `xtask scorecard --check`.
4. Write **the morning report** at the top of the progress log:
   - Five lines at most on what changed for the owner.
   - The score table before and after.
   - The commits in both repos, with one line each.
   - What failed and why.
   - The decisions waiting for the owner, each with a recommendation.
5. Commit this file with the log filled in.

---

## 5. Progress log (the agent appends; newest first)

### Morning report

_(written at the end)_

| Task | Status | Commit(s) | Result |
|---|---|---|---|
| Q1 mask naming | | | |
| Q2 shadow composite | | | |
| Q3 chat KV cache | | | |
| Q4 LoRA parity | | | |
| Q5 LoRA picker rows | | | |
| Q6 Generative Upscale | | | |
| Q7 LaMa tier | | | |
| Q8 extras | | | |

### Notes as they happen
