"""Draws the app's logo: assets/app.ico (16..256 px, for the exe) and
assets/icon-256.png (window and taskbar icon, read by main.rs), plus
assets/icon-preview.png (every size side by side, for checking by eye).

Design (original artwork, no brand marks): a blue rounded square, split by a
ground line. Below it, brown soil with an underground fuel tank (a horizontal
capsule, amber fuel level) buried in it; a fill riser rises from the tank to a
flat manhole cover sitting on the ground line, and a vent pipe rises above the
ground and ends in a T-cap. Above the ground, at 32 px and up, three white
"log" lines and a magnifying glass. Simpler at small sizes: 24 px drops the log
lines; 16 px is the soil, the tank and one riser stub crossing the line.

Run:  python -I tools/make_icon.py   (needs Pillow)
"""

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets"

SKY_TOP = (22, 92, 134)  # close to the app's selection blue
SKY_BOTTOM = (16, 70, 108)
SOIL_TOP = (92, 66, 44)  # earth, not water
SOIL_BOTTOM = (60, 42, 28)
GROUND_LINE = (150, 118, 84)
WHITE = (255, 255, 255)
AMBER = (247, 176, 50)
TANK_INSIDE = (214, 228, 240)
SS = 4  # supersampling


def gradient(s, top, bottom, y0, y1):
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    for y in range(int(y0), int(y1)):
        t = (y - y0) / max(1, (y1 - y0 - 1))
        c = tuple(round(top[i] + (bottom[i] - top[i]) * t) for i in range(3))
        d.line([(0, y), (s, y)], fill=c + (255,))
    return img


def draw(size: int) -> Image.Image:
    s = size * SS
    u = s / 100.0
    small = size <= 16
    detail = size >= 32

    # Ground line height: lower when there is a log above it.
    gy = (40 if small else 50 if not detail else 54) * u

    # Badge: sky above the ground line, soil below, clipped to a rounded square.
    mask = Image.new("L", (s, s), 0)
    m = round(s * 0.03)
    ImageDraw.Draw(mask).rounded_rectangle([m, m, s - 1 - m, s - 1 - m], radius=round(s * 0.2), fill=255)
    art = gradient(s, SKY_TOP, SKY_BOTTOM, 0, gy)
    art = Image.alpha_composite(art, gradient(s, SOIL_TOP, SOIL_BOTTOM, gy, s))
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    img.paste(art, (0, 0), mask)
    d = ImageDraw.Draw(img)

    # The buried tank: a long, flat horizontal capsule just below the surface
    # (a short riser; a deep tank with a tall riser reads as a lab flask).
    if small:
        left, top, right, bot, outline = 8 * u, 52 * u, 92 * u, 86 * u, 8 * u
    elif detail:
        left, top, right, bot, outline = 10 * u, gy + 15 * u, 90 * u, gy + 37 * u, 4 * u
    else:
        left, top, right, bot, outline = 8 * u, gy + 13 * u, 92 * u, gy + 40 * u, 6 * u
    r = (bot - top) / 2
    w = right - left

    # Fill riser: from the tank up to a flat manhole cover on the ground line.
    rw = w * (0.12 if not small else 0.2)
    rx = left + w * (0.55 if not small else 0.38)
    d.rectangle([rx, gy, rx + rw, top + outline], fill=WHITE)
    cover_h = (3 if detail else 5) * u

    # Vent pipe at the left end: a thin straight pipe up through the ground,
    # finished with a T-cap (a hooked end reads as a faucet over a bathtub).
    if not small:
        vw = (3 if detail else 5) * u
        px = left + w * 0.2  # pipe center
        vt = gy - (11 if detail else 13) * u  # top of the pipe
        d.rectangle([px - vw / 2, vt, px + vw / 2, top + outline], fill=WHITE)
        cap = (5 if detail else 6) * u
        d.rounded_rectangle([px - cap, vt - vw * 0.9, px + cap, vt + vw * 0.4], radius=vw * 0.4, fill=WHITE)

    # Ground line over the soil edge (thicker when small, so it survives scaling).
    lw = (1.5 if size >= 64 else 3.0) * u
    d.line([(6 * u, gy), (94 * u, gy)], fill=GROUND_LINE, width=round(lw))
    # Redraw the cover on top of the line.
    d.rounded_rectangle([rx - 3 * u, gy - cover_h, rx + rw + 3 * u, gy + 1 * u], radius=1.5 * u, fill=WHITE)

    # Tank body with the amber fuel level.
    d.rounded_rectangle([left, top, right, bot], radius=r, fill=TANK_INSIDE)
    fuel = Image.new("L", (s, s), 0)
    ImageDraw.Draw(fuel).rounded_rectangle([left, top, right, bot], radius=r, fill=255)
    clip = Image.new("L", (s, s), 0)
    ImageDraw.Draw(clip).rectangle([0, top + (bot - top) * 0.42, s, s], fill=255)
    fuel = Image.composite(fuel, Image.new("L", (s, s), 0), clip)
    img.paste(Image.new("RGBA", (s, s), AMBER + (255,)), (0, 0), fuel)
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([left, top, right, bot], radius=r, outline=WHITE, width=round(outline))

    # Above ground: the log lines and the magnifier.
    if detail:
        for i, w in enumerate((40, 30, 36)):
            y = (14 + i * 8.5) * u
            d.rounded_rectangle([14 * u, y, (14 + w) * u, y + 4.5 * u], radius=2.2 * u, fill=WHITE)
    if not small:
        cx, cy, rr = (70, 27, 12) if detail else (66, 24, 14)
        ring = 4.5 if detail else 6.5
        glass = Image.new("RGBA", (s, s), (0, 0, 0, 0))
        g = ImageDraw.Draw(glass)
        cx, cy, rr = cx * u, cy * u, rr * u
        g.ellipse([cx - rr, cy - rr, cx + rr, cy + rr], fill=(255, 255, 255, 60))
        g.ellipse([cx - rr, cy - rr, cx + rr, cy + rr], outline=WHITE + (255,), width=round(ring * u))
        hx, hy = cx + rr * 0.72, cy + rr * 0.72
        g.line([(hx, hy), (hx + 8 * u, hy + 8 * u)], fill=WHITE + (255,), width=round(ring * 1.5 * u))
        img = Image.alpha_composite(img, glass)

    return img.resize((size, size), Image.LANCZOS)


def main() -> None:
    OUT.mkdir(exist_ok=True)
    sizes = [16, 24, 32, 48, 64, 128, 256]
    images = [draw(n) for n in sizes]
    # One source for both files: the 256 px drawing.
    images[-1].save(OUT / "icon-256.png")
    images[-1].save(OUT / "app.ico", sizes=[(n, n) for n in sizes], append_images=images[:-1])
    strip = Image.new("RGBA", (sum(sizes) + 10 * len(sizes), 256), (40, 40, 40, 255))
    x = 0
    for n, im in zip(sizes, images):
        strip.paste(im, (x, 256 - n), im)
        x += n + 10
    strip.save(OUT / "icon-preview.png")
    print("wrote", OUT / "app.ico", "and", OUT / "icon-256.png")


if __name__ == "__main__":
    main()
