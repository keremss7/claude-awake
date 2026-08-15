#!/usr/bin/env python3
"""Generates every icon the bundle needs from one vector definition.

Kept as a script rather than checked-in binaries so the mark can be adjusted in
one place. Needs `rsvg-convert` (brew install librsvg) and Pillow.

The burst is drawn from scratch. If you ever publish this app, that matters: the
Claude/Anthropic mark is a trademark and shipping it in a third-party product is
not something a personal build gets to do.
"""

import subprocess
import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
ICONS = ROOT / "src-tauri" / "icons"

ARMS = 12
INNER = 9.0
CLAY = "#D97757"
INK_TOP = "#2B2825"
INK_BOTTOM = "#141312"


def burst(color: str, cx: float, cy: float, scale: float, opacity: str = "1") -> str:
    """Twelve tapered spokes, alternating length so it reads as a mark."""
    out = []
    for i in range(ARMS):
        angle = i * (360 / ARMS)
        long = i % 2 == 0
        outer = 47.0 if long else 32.0
        width = 7.0 if long else 5.0
        x = (50 - width / 2) * scale + cx - 50 * scale
        y = (50 - outer) * scale + cy - 50 * scale
        out.append(
            f'<rect x="{x:.3f}" y="{y:.3f}" width="{width * scale:.3f}" '
            f'height="{(outer - INNER) * scale:.3f}" rx="{width * scale / 2:.3f}" '
            f'fill="{color}" opacity="{opacity}" '
            f'transform="rotate({angle} {cx:.3f} {cy:.3f})"/>'
        )
    return "\n".join(out)


def app_svg(size: int = 1024) -> str:
    inset = size * 0.0625
    side = size - inset * 2
    radius = side * 0.235
    glow = size * 0.30
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 {size} {size}">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{INK_TOP}"/>
      <stop offset="1" stop-color="{INK_BOTTOM}"/>
    </linearGradient>
    <radialGradient id="glow" cx="0.5" cy="0.5" r="0.5">
      <stop offset="0" stop-color="{CLAY}" stop-opacity="0.42"/>
      <stop offset="1" stop-color="{CLAY}" stop-opacity="0"/>
    </radialGradient>
  </defs>
  <rect x="{inset}" y="{inset}" width="{side}" height="{side}" rx="{radius}" fill="url(#bg)"/>
  <rect x="{inset}" y="{inset}" width="{side}" height="{side}" rx="{radius}"
        fill="none" stroke="#FFFFFF" stroke-opacity="0.07" stroke-width="{size * 0.004}"/>
  <circle cx="{size / 2}" cy="{size / 2}" r="{glow}" fill="url(#glow)"/>
  {burst(CLAY, size / 2, size / 2, size * 0.0068)}
</svg>"""


def tray_svg(size: int = 44) -> str:
    """Template image: pure black plus alpha. macOS recolours it per menu bar."""
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" '
        f'viewBox="0 0 {size} {size}">\n'
        f"{burst('#000000', size / 2, size / 2, size / 100 * 0.92)}\n</svg>"
    )


def render(svg: str, out: Path, size: int) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["rsvg-convert", "-w", str(size), "-h", str(size), "-o", str(out)],
        input=svg.encode(),
        check=True,
    )


def main() -> int:
    tmp = ICONS / "_app.png"
    render(app_svg(), tmp, 1024)
    master = Image.open(tmp).convert("RGBA")

    for name, size in [
        ("32x32.png", 32),
        ("128x128.png", 128),
        ("128x128@2x.png", 256),
        ("icon.png", 512),
        ("Square30x30Logo.png", 30),
        ("Square44x44Logo.png", 44),
        ("Square71x71Logo.png", 71),
        ("Square89x89Logo.png", 89),
        ("Square107x107Logo.png", 107),
        ("Square142x142Logo.png", 142),
        ("Square150x150Logo.png", 150),
        ("Square284x284Logo.png", 284),
        ("Square310x310Logo.png", 310),
        ("StoreLogo.png", 50),
    ]:
        master.resize((size, size), Image.LANCZOS).save(ICONS / name)

    # Windows .ico — a handful of sizes is plenty and keeps the file small.
    master.resize((256, 256), Image.LANCZOS).save(
        ICONS / "icon.ico",
        sizes=[(16, 16), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )

    # macOS .icns via iconutil, which insists on a strictly named iconset.
    iconset = ICONS / "icon.iconset"
    iconset.mkdir(exist_ok=True)
    for name, size in [
        ("icon_16x16.png", 16),
        ("icon_16x16@2x.png", 32),
        ("icon_32x32.png", 32),
        ("icon_32x32@2x.png", 64),
        ("icon_128x128.png", 128),
        ("icon_128x128@2x.png", 256),
        ("icon_256x256.png", 256),
        ("icon_256x256@2x.png", 512),
        ("icon_512x512.png", 512),
        ("icon_512x512@2x.png", 1024),
    ]:
        master.resize((size, size), Image.LANCZOS).save(iconset / name)
    subprocess.run(
        ["iconutil", "-c", "icns", str(iconset), "-o", str(ICONS / "icon.icns")],
        check=True,
    )
    for leftover in iconset.iterdir():
        leftover.unlink()
    iconset.rmdir()
    tmp.unlink()

    render(tray_svg(), ICONS / "tray.png", 44)
    print(f"icons written to {ICONS}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
