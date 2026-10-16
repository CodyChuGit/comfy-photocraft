# Fork app icon: the "Ps" tile

The Comfy PhotoCraft fork shows a Photoshop-style tile instead of upstream's kitsune, for a
personal local build only. It is not part of any published build (see the brand checklist in
`docs/comfy/upstream.md`); the kitsune files beside this folder stay untouched so upstream merges
are clean, and the code points here through three one-line edits (`crates/ui-egui/src/brand.rs`,
`apps/photocraft/src/app_icon.rs`, `apps/photocraft/build.rs`).

## Palette

| Colour | Hex | Used for |
|---|---|---|
| Navy | `#001E36` | the rounded tile |
| Light blue | `#31A8FF` | the "Ps" |

## Geometry

A rounded square with the same corner radius as the kitsune tile (112/512), "Ps" in Segoe UI
Bold at 46% of the tile height, centred.

## Files

| File | Used by |
|---|---|
| `ps-128.png` | `brand.rs`: the title-bar mark (mipmapped texture) |
| `ps-256.png` | `app_icon.rs`: the window and taskbar icon on Windows and Linux |
| `ps.ico` | `build.rs`: the `photocraft.exe` resource icon (PNG entries at 16, 24, 32, 48, 64, 128, 256 px) |

## Regenerating

Drawn with .NET `System.Drawing` from a short PowerShell script (rounded path, `DrawString`,
PNG entries packed into an ICONDIR). Any image tool that writes PNGs and a PNG-entry `.ico`
will do; `app_icon.rs`'s `exe_icon_has_every_shell_size` test checks the result.
