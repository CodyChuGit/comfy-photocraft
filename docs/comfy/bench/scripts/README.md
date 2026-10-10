# Bench and verification scripts

The PowerShell scripts the 2026-10-09 sessions used against the local ComfyUI server, kept as
they ran (they hard-code this machine's paths: the repo under `C:\Users\5090\Projects\comfy-photocraft`,
the server at `127.0.0.1:8188`, the bench images under `C:\Users\5090\ComfyUI\photocraft-tests\bench`,
copied here into `../images/`, which git ignores). Edit the paths at the top of a script before
running it elsewhere. The numbers they produced are in [`../../benchmarks.md`](../../benchmarks.md).

| Script | What it measures or does |
|---|---|
| `restart-comfyui.ps1` | Restart the server with logging (`-u`), the measured flags, a known log file |
| `thrash-probe.ps1`, `repro-slow.ps1` | Fresh loads of the 2511 edit; the slow-run investigation |
| `edge-live.ps1`, `after-edge.ps1`, `expand-ab*.ps1`, `small-ab.ps1`, `guided-ab.ps1`, `ab-run.ps1` | Soft-edge and Expand A/B runs (§3) |
| `matte-live.ps1`, `matte-route-live.ps1`, `probe-rgba.ps1`, `probe-t2i-alpha.ps1` | Remove Background and transparent generation (§5b) |
| `edit-live.ps1`, `purge-live.ps1`, `similar-live.ps1`, `split-live.ps1`, `probe-sam-point.ps1` | Generative Edit, the purge, Generate Similar, Split into Layers, `select.byPoint` |
| `user-drawing-live.ps1`, `user-drawing-variants.ps1`, `doodle-sentences.ps1` | The doodle session: which prompts give a face |
| `probe-enhance*.ps1`, `enhance-live.ps1`, `enhance-doodle-final.ps1` | The prompt enhancer: the chat-template finding (5: thinking on/off), the final rules (6), end to end (§5e) |
| `nvfp4-bench.ps1`, `nvfp4-live.ps1` | NVFP4 against fp8, and the policy live (§5f) |
| `sheet.ps1`, `pixdiff.ps1` | Contact sheets and pixel differences of bench images |
| `verify-*.ps1`, `snapshots*.ps1` | UI verification runs through the control protocol |
| `append-translations.ps1` + `pref-translations.tsv` | Append `lang<TAB>source<TAB>translation` rows to the 13 TSVs |
| `enhance-edit-system.txt`, `enhance-t2i-system.txt` | ComfyUI's shipped enhancer prompts, for reference (too long for the 4B model) |
