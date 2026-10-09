# Generative Fill benchmark against the local ComfyUI (comfy-photocraft).
#
#   powershell -File docs\comfy\bench\bench-fill.ps1 [-Cli target\release\photocraft-cli.exe]
#       [-Image C:\path\in.png] [-Out C:\path\bench] [-Repeat 2] [-Tag flags-default]
#
# Runs every fill template -Repeat times with the same prompt and prints a table with the
# client's own timing breakdown (encode / upload / queue / run / download, from the command's
# `timings`). Run 1 of a template ("cold") uses a selection shifted by 16 px per template, so
# the server's node cache (keyed on upload content) cannot serve it the previous template's
# text-encoder and VAE work: that is a first fill on new pixels, model load included when the
# family changed. Later runs ("warm") reuse the template's selection with the next seed: the
# variation / re-roll case, where only sampling runs. Outputs land in -Out as PNGs for a visual
# comparison. Nothing is downloaded or installed.
#
# PowerShell 5.1 mangles JSON arguments that contain spaces, so the CLI is called through the
# stop-parsing token with the dynamic parts in environment variables (expanded as %NAME%).
param(
    [string]$Cli = "target\release\photocraft-cli.exe",
    [string]$Image = "C:\Users\5090\ComfyUI\photocraft-tests\q21-image.png",
    [string]$Out = "C:\Users\5090\ComfyUI\photocraft-tests\bench",
    [int]$Repeat = 2,
    [string]$Tag = "default",
    [string]$Prompt = "a small red wooden rowing boat floating on the water",
    [int]$Seed = 7,
    # The lighthouse's water/rock edge: a 384x320 selection, 25 % margin = a 576x512 request.
    [int]$RectX = 320,
    [int]$RectY = 560,
    [int]$RectW = 384,
    [int]$RectH = 320
)
$ErrorActionPreference = "Continue"
New-Item -ItemType Directory -Force $Out | Out-Null
if (-not (Test-Path $Cli)) { throw "no CLI at $Cli (cargo build --release -p photocraft-cli)" }
$Cli = (Resolve-Path $Cli).Path

$templates = @(
    @{ id = "qwen-edit-2511/fill";             name = "2511 base, 40 steps" },
    @{ id = "qwen-edit-2511/fill-lightning-8"; name = "2511 Lightning, 8 steps" },
    @{ id = "qwen-edit-2511/fill-lightning-4"; name = "2511 Lightning, 4 steps" },
    @{ id = "qwen-2.1/fill";                   name = "Qwen-Image-2.1, 25 steps" }
)
$env:BENCH_IMAGE = $Image
$env:BENCH_PREFS = '{\"values\":{\"integrations.allowResearchModels\":true}}'
$rows = @()
$n = 0
foreach ($t in $templates) {
    # A selection of the same size, shifted per template: new pixels for the server's cache.
    $x = $RectX - 16 * $n
    $env:BENCH_RECT = '{\"x\":' + $x + ',\"y\":' + $RectY + ',\"width\":' + $RectW + ',\"height\":' + $RectH + '}'
    $n++
    for ($i = 1; $i -le $Repeat; $i++) {
        $kind = if ($i -eq 1) { "cold" } else { "warm$i" }
        # (PowerShell variable names are case-insensitive: `$seed` would be `$Seed`.)
        $runSeed = $Seed + $i - 1
        $slug = ($t.id -replace '[/.]', '-')
        $env:BENCH_OUT = Join-Path $Out "$Tag-$slug-$kind.png"
        $env:BENCH_FILL = '{\"prompt\":\"' + $Prompt + '\",\"template\":\"' + $t.id + '\",\"seed\":' + $runSeed + ',\"margin\":0.25}'
        $t0 = Get-Date
        $lines = & $Cli --% run %BENCH_IMAGE% --cmd prefs.set --params "%BENCH_PREFS%" --cmd select.rect --params "%BENCH_RECT%" --cmd generate.fill --params "%BENCH_FILL%" --out %BENCH_OUT%
        $wall = [int](((Get-Date) - $t0).TotalMilliseconds)
        $result = $null
        foreach ($l in $lines) { if ("$l" -match '"timings"') { $result = "$l" } }
        if ($null -eq $result) {
            $rows += [pscustomobject]@{ template = $t.name; run = $kind; ms = "FAILED"; encode = ""; upload = ""; queue = ""; sampling = ""; download = ""; wall = $wall; note = (($lines | Select-Object -Last 1) -join " ") }
            continue
        }
        $j = ($result | ConvertFrom-Json).result
        $tm = $j.timings[0]
        $rows += [pscustomobject]@{
            template = $t.name; run = $kind; ms = $j.ms; encode = $tm.encodeMs; upload = $tm.uploadMs; queue = $tm.queueMs
            sampling = $tm.runMs; download = $tm.downloadMs; wall = $wall; note = "$($j.requestWidth)x$($j.requestHeight) via $($j.template)"
        }
    }
}
"Tag: $Tag  image: $Image  seeds: $Seed.. (cold = new pixels for the server's cache; warm = same pixels, next seed)"
$rows | Format-Table -AutoSize | Out-String -Width 200
$rows | Export-Csv -NoTypeInformation -Encoding UTF8 (Join-Path $Out "bench-$Tag.csv")
