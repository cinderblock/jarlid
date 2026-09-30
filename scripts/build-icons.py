#!/usr/bin/env python3
"""Regenerate every app icon in app/src-tauri/icons/ from the art in icons/source/.

First runs icons/source/jarlid-art.py, which draws the two SVGs (the icon is modelled in 3D
there, so the SVGs are generated output, not something to edit).

`tauri icon` renders one source at every size, which is fine from 40 px up but turns the jar
into a blur at taskbar sizes. So this renders the full set from jarlid.svg, then replaces the
16-32 px images with jarlid-small.svg (art redrawn for those sizes) and rebuilds icon.ico so
each size inside it comes from the right art.

Only files already present in icons/ are written: `tauri icon` also emits Android and iOS sets
this app does not ship.

Needs bun (for the Tauri CLI in app/node_modules) and Pillow. Run from anywhere:
    python scripts/build-icons.py
"""
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image

APP = Path(__file__).resolve().parent.parent / "app"
ICONS = APP / "src-tauri" / "icons"
ART = ICONS / "source" / "jarlid-art.py"
FULL = ICONS / "source" / "jarlid.svg"
SMALL = ICONS / "source" / "jarlid-small.svg"

# Largest size that uses the small art. Windows draws the taskbar at 24 px (100% scaling) to
# 32 px (150%); the full art holds up from 40 px.
SMALL_MAX = 32
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 256]
# Standalone PNGs in icons/ at or under SMALL_MAX, which get the small art too.
SMALL_PNGS = {"32x32.png": 32, "Square30x30Logo.png": 30}


def tauri_icon(source: Path, out: Path, *extra: str) -> None:
    bunx = shutil.which("bunx")
    if bunx is None:
        sys.exit("bunx not found on PATH")
    subprocess.run([bunx, "tauri", "icon", str(source), "-o", str(out), *extra], cwd=APP, check=True,
                   stdout=subprocess.DEVNULL)


def main() -> None:
    subprocess.run([sys.executable, str(ART)], check=True)
    with tempfile.TemporaryDirectory() as tmp:
        full_dir = Path(tmp) / "full"
        small_dir = Path(tmp) / "small"
        tauri_icon(FULL, full_dir)
        tauri_icon(FULL, full_dir / "px", "--png", ",".join(str(s) for s in ICO_SIZES))
        small_sizes = sorted({s for s in ICO_SIZES if s <= SMALL_MAX} | set(SMALL_PNGS.values()))
        tauri_icon(SMALL, small_dir, "--png", ",".join(str(s) for s in small_sizes))

        def art(size: int) -> Path:
            src = small_dir if size <= SMALL_MAX else full_dir / "px"
            return src / f"{size}x{size}.png"

        for existing in sorted(ICONS.iterdir()):
            if existing.is_file() and existing.name != "icon.ico":
                size = SMALL_PNGS.get(existing.name)
                shutil.copyfile(art(size) if size else full_dir / existing.name, existing)

        images = [Image.open(art(s)).convert("RGBA") for s in ICO_SIZES]
        images[-1].save(ICONS / "icon.ico", format="ICO", sizes=[(s, s) for s in ICO_SIZES],
                        append_images=images[:-1])

    print(f"icons regenerated in {ICONS}")


if __name__ == "__main__":
    main()
