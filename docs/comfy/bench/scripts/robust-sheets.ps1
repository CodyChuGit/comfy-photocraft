# Contact sheets of the robustness set: a row per scene (original, half, whole), cropped
# around the object (its box doubled, at least 400 px), stacked -PerSheet rows per sheet.
# Output: bench\rb-sheet-<n>.png. -Tags a,b,c limits the scenes.
param([string]$Tags = "", [int]$PerSheet = 7)
Set-Location "C:\Users\5090\Documents\PhotoshopEX"
$out = "C:\Users\5090\ComfyUI\photocraft-tests\bench"
$sheet = (Resolve-Path "docs\comfy\bench\scripts\sheet.ps1").Path
$objects = Get-Content (Join-Path $out "rb-objects.json") -Raw | ConvertFrom-Json
$want = @($Tags -split ',' | Where-Object { $_ -ne "" })
Add-Type -AssemblyName System.Drawing
$strips = @()
foreach ($p in $objects.PSObject.Properties) {
    $tag = $p.Name; $o = $p.Value
    if ($null -eq $o) { continue }
    if ($want.Count -gt 0 -and $want -notcontains $tag) { continue }
    $x = $o.bounds[0]; $y = $o.bounds[1]; $w = $o.bounds[2]; $h = $o.bounds[3]
    $side = [Math]::Min(1024, [Math]::Max(400, [Math]::Max($w, $h) * 2))
    $cx = $x + $w / 2; $cy = $y + $h / 2
    $x0 = [int][Math]::Max(0, [Math]::Min(1024 - $side, $cx - $side / 2)); $y0 = [int][Math]::Max(0, [Math]::Min(1024 - $side, $cy - $side / 2))
    $strip = Join-Path $out "rb-$tag-strip.png"
    & powershell -NoProfile -File $sheet -Names "rb-$tag.png,rb-$tag-half.png,rb-$tag-whole.png" -Cols 3 -Crop "$x0,$y0,$side,$side" -Tile "400,400" -Out $strip | Out-Null
    $strips += $strip
}
$n = 0
for ($i = 0; $i -lt $strips.Count; $i += $PerSheet) {
    $n++
    $part = $strips[$i..([Math]::Min($i + $PerSheet, $strips.Count) - 1)]
    $imgs = $part | ForEach-Object { [System.Drawing.Bitmap]::FromFile($_) }
    $wmax = [int](($imgs | ForEach-Object { $_.Width } | Measure-Object -Maximum).Maximum)
    $htot = [int](($imgs | ForEach-Object { $_.Height } | Measure-Object -Sum).Sum + 6 * ($imgs.Count - 1))
    $bmp = New-Object System.Drawing.Bitmap -ArgumentList $wmax, $htot
    $g = [System.Drawing.Graphics]::FromImage($bmp); $g.Clear([System.Drawing.Color]::Black)
    $yy = 0
    foreach ($im in $imgs) { $g.DrawImage($im, 0, $yy, $im.Width, $im.Height); $yy += $im.Height + 6; $im.Dispose() }
    $g.Dispose()
    $file = Join-Path $out "rb-sheet-$n.png"
    $bmp.Save($file, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
    $file
}
