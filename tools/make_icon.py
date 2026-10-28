"""Genera assets/icon.ico y assets/icon.png: isolíneas plateadas sobre negro,
la misma idea visual que "Isoline Void". Requiere Pillow.

    python tools/make_icon.py
"""
import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
SS = 1024  # supersampling; se reduce a 256 al final


def field(x: float, y: float) -> float:
    v = math.sin(x * 2.6 + 0.4) * 0.40
    v += math.sin(y * 3.1 - 0.9) * 0.35
    v += math.sin((x + y) * 2.1 + 1.3) * 0.25
    for cx, cy, amp in ((0.18, -0.12, 1.25), (-0.42, 0.35, 0.55), (0.45, 0.42, 0.35)):
        dx, dy = x - cx, y - cy
        v += amp * math.exp(-(dx * dx + dy * dy) * 4.5)
    return v


def render() -> Image.Image:
    img = Image.new("RGBA", (SS, SS), (0, 0, 0, 0))
    px = img.load()
    spacing = 0.15
    half = SS / 2
    for j in range(SS):
        y = (j - half) / half * 1.1
        for i in range(SS):
            x = (i - half) / half * 1.1
            r = math.hypot(x, y)
            v = field(x, y) / spacing
            d = abs(v - math.floor(v) - 0.5)  # 0.5 en la línea
            line = max(0.0, (d - 0.34) / 0.16)  # grosor de línea
            fade = max(0.0, min(1.0, (1.02 - r) / 0.35))
            g = int(235 * line * fade)
            px[i, j] = (g, g, min(255, g + 8), 255)
    return img


def rounded_mask(size: int, radius: int) -> Image.Image:
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, size - 1, size - 1), radius, fill=255)
    return mask


def main() -> None:
    art = render()
    base = Image.new("RGBA", (SS, SS), (6, 6, 8, 255))
    base.alpha_composite(art)
    glow = art.filter(ImageFilter.GaussianBlur(SS / 90))
    base = Image.blend(base, Image.alpha_composite(base, glow), 0.35)
    base.putalpha(rounded_mask(SS, SS // 5))
    icon = base.resize((256, 256), Image.LANCZOS)

    assets = ROOT / "assets"
    icon.save(assets / "icon.png")
    icon.save(assets / "icon.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
    print("assets/icon.png, assets/icon.ico")


if __name__ == "__main__":
    main()
