# Removal through Qwen-Image-2.1 as an edit by instruction (AusBoss's Cut Out + Remove recipe,
# research/notes/ausboss-catalogue-2026-10.md 2.2): `qwen-2.1/edit` as the removal template
# ("Remove the <name> …" on the crop, no mask in the model's view, the layer mask confining
# the change, the result realigned), on the native worker with the rewriter on the worker too,
# one candidate a stroke; without and with AusBoss's Consistency LoRA. Against the 2511
# one-candidate run (-one). -Tags picks scenes (default: the stock set's hard ones).
param([string]$Tags = "glasses,watch,tattoo,cup,dog,boat,bicycle,hat,umbrella,motorbike", [string]$Kinds = "half,whole")
Set-Location "C:\Users\5090\Documents\PhotoshopEX"
$scripts = "docs\comfy\bench\scripts"
$st = "C:\Users\5090\ComfyUI\photocraft-tests\bench\stock"
. "$scripts\native-worker.ps1"
if (-not $env:PHOTOCRAFT_NATIVE_ENGINE) { "no worker"; exit 1 }
try { $null = Invoke-RestMethod -Method Post -Uri http://127.0.0.1:8188/free -Body '{"unload_models":true,"free_memory":true}' -ContentType application/json -TimeoutSec 10 } catch { "(ComfyUI free: $_)" }
foreach ($variant in @(@("-e21", ""), @("-e21c", "qwen-image-2.1-consistency.safetensors:1.0"))) {
    $suffix, $loras = $variant
    "=== $suffix (loras: '$loras') $(Get-Date -Format HH:mm:ss)"
    # (an empty `-Loras ''` is dropped by the shell and leaves the parameter without its value:
    # the no-LoRA pass then never ran; pass it only when there is one)
    $more = if ($loras) { @("-Loras", $loras) } else { @() }
    & powershell -NoProfile -File "$scripts\robust-strokes.ps1" -Dir $st -Prefix st -Objects st-objects.json -Tags $Tags -Kinds $Kinds -Suffix $suffix -Extra "variations=1" -Purge heavy -Engine auto -Template qwen-2.1/edit @more 2>&1 | Where-Object { "$_" -match "ms grow|FAILED|restarted" } | ForEach-Object { "$_" -replace ': Remove the (.{0,40}).*', ': $1' }
}
& "$scripts\native-worker.ps1" -Stop
"=== done $(Get-Date -Format HH:mm:ss)"
