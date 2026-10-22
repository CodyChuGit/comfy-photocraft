# The robustness set's strokes (robust-make.ps1's rb-objects.json), through the tools' own
# commands, Lightning (Quality balanced), random seeds:
#   half   Spot Healing (generative), one stroke along the object's long axis over its first
#          half: the hard case (the rest of the object stays in view; the detector and the
#          rewriter's name have to take it whole)
#   whole  the Remove tool (generative), a serpentine stroke over the object's whole box
# -Tags a,b,c runs those scenes (default: all). Output: bench\rb-<tag>-half.png,
# rb-<tag>-whole.png, and a line per run with the rewriter's words, the growth and the time
# (appended to bench\rb-log.txt).
# -Dir, -Prefix and -Objects point it at another set (the stock photographs: -Dir bench\stock
# -Prefix st -Objects st-objects.json); the image is <Prefix>-<tag>.<any extension>.
# -Quality best runs the 40-step tier; -Suffix names the outputs apart (<Prefix>-<tag>-half<Suffix>.png);
# -Kinds half,whole picks the strokes.
# -Extra adds fields to every stroke's params (e.g. ',\"prefill\":true'); -Template names the fill
# template (research-only ones allowed for the measurement).
param([string]$Tags = "", [string]$Dir = "C:\Users\5090\ComfyUI\photocraft-tests\bench", [string]$Prefix = "rb", [string]$Objects = "rb-objects.json", [string]$Quality = "balanced", [string]$Suffix = "", [string]$Kinds = "half,whole", [string]$Extra = "", [string]$Template = "")
Set-Location "C:\Users\5090\Documents\PhotoshopEX"
$cli = (Resolve-Path "target\release\photocraft-cli.exe").Path
$out = $Dir
$tpl = if ($Template -eq "") { "" } else { ',\"integrations.defaultFillTemplate\":\"' + $Template + '\",\"integrations.allowResearchModels\":true' }
# -Extra is key=value pairs separated by semicolons ("prefill=true;grow=20"): quotes do not
# survive the outer shell's argument passing, so they are added here.
$Extra = (($Extra -split ';' | Where-Object { $_ -match '^\s*([A-Za-z]\w*)\s*=\s*(.+?)\s*$' } | ForEach-Object { $k = $Matches[1]; $v = $Matches[2]; if ($v -match '^(true|false|-?\d+(\.\d+)?)$') { ',\"' + $k + '\":' + $v } else { ',\"' + $k + '\":\"' + $v + '\"' } }) -join '')
$env:CM_PREFS = '{\"values\":{\"integrations.engine\":\"comfy\",\"integrations.enhancePrompts\":false,\"integrations.quality\":\"' + $Quality + '\"' + $tpl + '}}'
# (not `$kinds`: that is the [string] parameter, case ignored, and would turn the list into "half whole")
$kindList = @($Kinds -split ',' | Where-Object { $_ -ne "" })
# (not `$objects`: PowerShell variable names ignore case, and the [string] parameter would
# turn the parsed set back into a string)
$set = Get-Content (Join-Path $out $Objects) -Raw | ConvertFrom-Json
$want = @($Tags -split ',' | Where-Object { $_ -ne "" })
$log = Join-Path $out "$Prefix-log.txt"
function Pts($list) { "[" + (($list | ForEach-Object { "[$($_[0]),$($_[1])]" }) -join ",") + "]" }
function Tool($tag, $kind, $cmd, $params) {
    $image = Get-ChildItem (Join-Path $out "$Prefix-$tag.*") | Where-Object { $_.Extension -match '^\.(png|jpg|jpeg)$' } | Select-Object -First 1
    $env:CM_IMAGE = $image.FullName; $env:CM_CMD = $cmd; $env:CM_PARAMS = $params; $env:CM_OUT = Join-Path $out "$Prefix-$tag-$kind$Suffix.png"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $lines = & $cli --% run %CM_IMAGE% --cmd prefs.set --params "%CM_PREFS%" --cmd %CM_CMD% --params "%CM_PARAMS%" --cmd generate.info --params "{}" --out %CM_OUT%
    $said = $null
    foreach ($l in $lines) { if ("$l" -match '"enhanced":"([^"]*)".*"grow":(\d+)') { $said = "{0,-9} {1,-5} {2,6} ms grow {3,3}: {4}" -f $tag, $kind, $sw.ElapsedMilliseconds, $Matches[2], $Matches[1] } }
    if ($null -eq $said) { $said = "{0,-9} {1,-5} FAILED: {2}" -f $tag, $kind, (($lines | Where-Object { $_ -notmatch "flattened" } | Select-Object -Last 1) -join " ") }
    $said; $said | Out-File $log -Append -Encoding utf8
}
foreach ($p in $set.PSObject.Properties) {
    $tag = $p.Name; $o = $p.Value
    if ($null -eq $o) { continue }
    if ($want.Count -gt 0 -and $want -notcontains $tag) { continue }
    $x = $o.bounds[0]; $y = $o.bounds[1]; $w = $o.bounds[2]; $h = $o.bounds[3]
    $cx = $x + [int]($w / 2); $cy = $y + [int]($h / 2)
    $s = [Math]::Round([Math]::Min([Math]::Max([Math]::Min($w, $h) / 2, 12), 64))
    # half: along the long axis, from 5 % to 50 % of it
    # (parenthesised sums: the comma binds tighter than + in PowerShell)
    if ($w -ge $h) { $half = @(@(($x + [int](0.05 * $w)), $cy), @(($x + [int](0.5 * $w)), $cy)) }
    else { $half = @(@($cx, ($y + [int](0.05 * $h))), @($cx, ($y + [int](0.5 * $h)))) }
    if ($kindList -contains "half") { Tool $tag "half" "paint.spotHealing" ('{\"type\":\"generative\",\"size\":' + $s + ',\"hardness\":1' + $Extra + ',\"points\":' + (Pts $half) + '}') }
    if ($kindList -notcontains "whole") { continue }
    # whole: a serpentine over the box, rows a brush apart
    $sw2 = [Math]::Round([Math]::Min([Math]::Max([Math]::Min($w, $h) / 2, 16), 80))
    $rows = New-Object System.Collections.ArrayList; $yy = $y + [int]($sw2 / 2); $i = 0
    $left = $x + [int]($sw2 / 2); $right = $x + $w - [int]($sw2 / 2)
    while ($yy -lt $y + $h -and $i -lt 14) {
        if ($i % 2 -eq 0) { [void]$rows.Add("[$left,$yy]"); [void]$rows.Add("[$right,$yy]") }
        else { [void]$rows.Add("[$right,$yy]"); [void]$rows.Add("[$left,$yy]") }
        $yy += [int](0.8 * $sw2); $i++
    }
    if ($rows.Count -lt 2) { [void]$rows.Add("[$x,$cy]"); [void]$rows.Add("[$($x + $w),$cy]") }
    Tool $tag "whole" "paint.remove" ('{\"generative\":true,\"size\":' + $sw2 + $Extra + ',\"points\":[' + ($rows -join ",") + ']}')
}
