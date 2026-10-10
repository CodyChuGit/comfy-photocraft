"""Render the PhotoshopEX app icon: a Photoshop-style tile (dark navy rounded square, light-blue
letters) with the letters "Px", into every file the app and its packaging read.

Usage: python -I icon-px.py <repo-root>

Writes, under <repo-root>/assets/app-icon/: photocraft.svg and photocraft-small.svg (the SVG
masters, text-based), photocraft-1024.png (Apple's padded grid), hicolor/<s>x<s>/apps/
ai.storyteller.photocraft.png for 16..512 and hicolor/scalable/apps/...svg, photocraft.icns, and
the PNGs for the .ico into <repo-root>/target/icon-px/ico-<s>.png (pack them with
`cargo xtask ico`). The geometry matches packaging/icons.sh: a 512-unit tile with rx=112, cropped
22 units a side for the Windows and Linux renders, padded to 636 units for macOS.
"""
import os
import sys

from PIL import Image, ImageDraw, ImageFont

ROOT = sys.argv[1] if len(sys.argv) > 1 else "."
OUT = os.path.join(ROOT, "assets", "app-icon")
NAVY = (0, 30, 54, 255)        # #001E36
BLUE = (49, 168, 255, 255)     # #31A8FF
LETTERS = "Px"
FONT = r"C:\Windows\Fonts\segoeuib.ttf"
SCALE = 8  # render the 512 tile at 4096 px, then downsample


def tile(size_units=512, margin_units=0):
    """The full tile rendered at SCALE× into a square of (size_units) units, the tile itself
    centred with `margin_units` of transparency around it (0 = full bleed)."""
    px = size_units * SCALE
    img = Image.new("RGBA", (px, px), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    m = margin_units * SCALE
    tile_px = px - 2 * m
    r = int(112 * SCALE * tile_px / (512 * SCALE))
    d.rounded_rectangle([m, m, m + tile_px - 1, m + tile_px - 1], radius=r, fill=NAVY)
    # Letters: cap height about 46 % of the tile, baseline a little below centre, like the
    # Adobe tiles' lettering.
    font_px = int(tile_px * 0.56)
    font = ImageFont.truetype(FONT, font_px)
    bbox = d.textbbox((0, 0), LETTERS, font=font)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    x = m + (tile_px - tw) / 2 - bbox[0]
    y = m + (tile_px - th) / 2 - bbox[1] - tile_px * 0.01
    d.text((x, y), LETTERS, font=font, fill=BLUE)
    return img


def save(img, path, size):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    img.resize((size, size), Image.LANCZOS).save(path, "PNG", optimize=True)


def main():
    full = tile()                       # 512 units, full bleed
    # Windows / Linux: crop 22 units a side (into the rounded corners), as icons.sh does.
    c = 22 * SCALE
    tight = full.crop((c, c, full.width - c, full.height - c))
    # macOS: the tile on the 636-unit padded grid (62 units of margin a side).
    mac = tile(636, 62)

    save(mac, os.path.join(OUT, "photocraft-1024.png"), 1024)
    for s in (16, 24, 32, 48, 64, 128, 256, 512):
        save(tight, os.path.join(OUT, "hicolor", f"{s}x{s}", "apps", "ai.storyteller.photocraft.png"), s)
    ico_dir = os.path.join(ROOT, "target", "icon-px")
    os.makedirs(ico_dir, exist_ok=True)
    for s in (16, 20, 24, 32, 40, 48, 64, 128, 256):
        save(tight, os.path.join(ico_dir, f"ico-{s}.png"), s)
    # macOS .icns from the padded renders (Pillow writes ICNS).
    icns_sizes = [16, 32, 64, 128, 256, 512, 1024]
    base = mac.resize((1024, 1024), Image.LANCZOS)
    base.save(os.path.join(OUT, "photocraft.icns"), format="ICNS", sizes=[(s, s) for s in icns_sizes])

    svg = (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512">\n'
        '  <!-- PhotoshopEX app icon: a Photoshop-style tile with the letters "Px". The PNGs are\n'
        '       rendered by packaging/icon-px.py (Segoe UI Bold); this SVG is the editable master. -->\n'
        '  <rect width="512" height="512" rx="112" fill="#001E36"/>\n'
        '  <text x="256" y="356" text-anchor="middle" font-family="Segoe UI, Helvetica Neue, Arial, sans-serif"\n'
        '        font-weight="700" font-size="287" fill="#31A8FF">Px</text>\n'
        '</svg>\n'
    )
    for name in ("photocraft.svg", "photocraft-small.svg"):
        with open(os.path.join(OUT, name), "w", encoding="utf-8", newline="\n") as f:
            f.write(svg)
    scalable = os.path.join(OUT, "hicolor", "scalable", "apps")
    os.makedirs(scalable, exist_ok=True)
    with open(os.path.join(scalable, "ai.storyteller.photocraft.svg"), "w", encoding="utf-8", newline="\n") as f:
        f.write(svg)
    print("icons written to", OUT, "and", ico_dir)


if __name__ == "__main__":
    main()
