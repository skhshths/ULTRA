"""Generate assets/ultra.ico (and a 256px preview PNG) for the ULTRA browser.

A forward-leaning "U" with a lightning bolt cut through its hollow and speed
streaks trailing behind it, on a transparent background. Every size is drawn
separately (supersampled) so small sizes stay crisp: 16px drops the streaks and
bolt, 24-32px keep the bolt, 48px+ get the full mark.

Run: python tools/make_icon.py
"""

import io
import struct
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
SS = 8  # supersampling factor
SHEAR = 0.2  # forward lean
TOP, BOTTOM = 0.16, 0.86
LEFT, RIGHT, STROKE = 0.30, 0.78, 0.15
MINT_TOP, MINT_BOT = (196, 255, 238), (44, 178, 152)
OUTLINE = (6, 18, 15)


def shear(img, yc):
    """Lean the shape forward around the line y = yc (pixels)."""
    return img.transform(img.size, Image.AFFINE, (1, SHEAR, -SHEAR * yc, 0, 1, 0), resample=Image.BICUBIC)


def u_mask(S):
    m = Image.new('L', (S, S), 0)
    d = ImageDraw.Draw(m)
    R = (RIGHT - LEFT) / 2
    cx, cy = (LEFT + RIGHT) / 2, BOTTOM - R
    d.rectangle([LEFT * S, TOP * S, RIGHT * S, cy * S], fill=255)
    d.ellipse([(cx - R) * S, (cy - R) * S, (cx + R) * S, (cy + R) * S], fill=255)
    r = R - STROKE
    d.rectangle([(cx - r) * S, (TOP - 0.05) * S, (cx + r) * S, cy * S], fill=0)
    d.ellipse([(cx - r) * S, (cy - r) * S, (cx + r) * S, (cy + r) * S], fill=0)
    return m, cy


def bolt_mask(S):
    m = Image.new('L', (S, S), 0)
    pts = [(0.555, 0.17), (0.640, 0.17), (0.578, 0.395), (0.645, 0.395),
           (0.500, 0.735), (0.535, 0.47), (0.468, 0.47)]
    ImageDraw.Draw(m).polygon([(x * S, y * S) for x, y in pts], fill=255)
    return m


def streak_mask(S, yc, count):
    """Tapered speed streaks ending just behind the U's (sheared) left edge."""
    m = Image.new('L', (S, S), 0)
    d = ImageDraw.Draw(m)
    rows = [(0.30, 0.22), (0.46, 0.17), (0.62, 0.12)][:count]
    th = 0.055
    for y, length in rows:
        edge = LEFT + SHEAR * (yc / S - y) - 0.045
        x0 = edge - length
        # taper: thin tail on the left, full height at the head
        d.polygon([(x0 * S, (y + th * 0.35) * S), (edge * S, y * S), (edge * S, (y + th) * S),
                   (x0 * S, (y + th * 0.65) * S)], fill=255)
    return m


def dilate(mask, px):
    """Approximate dilation: blur then a soft threshold (keeps antialiasing)."""
    b = mask.filter(ImageFilter.GaussianBlur(px))
    return b.point(lambda v: 0 if v < 14 else 255 if v > 24 else int((v - 14) * 255 / 10))


def gradient(S, top, bot):
    g = Image.new('RGBA', (1, S))
    for y in range(S):
        t = y / (S - 1)
        g.putpixel((0, y), tuple(int(top[i] + (bot[i] - top[i]) * t) for i in range(3)) + (255,))
    return g.resize((S, S))


def render(size):
    S = size * SS
    u, yc_unit = u_mask(S)
    yc = yc_unit * S
    detail_bolt = size >= 24
    streaks = 0 if size < 32 else 1 if size < 48 else 3

    u = shear(u, yc)
    bolt = shear(bolt_mask(S), yc) if detail_bolt else Image.new('L', (S, S), 0)
    lines = streak_mask(S, yc, streaks) if streaks else Image.new('L', (S, S), 0)

    # Outline weight scales so it's ~1px at 16px and proportionally thinner at large sizes.
    ow = max(SS * 1.0, S * 0.012)
    gap = dilate(bolt, ow * 0.9) if detail_bolt else bolt
    u = ImageChops.subtract(u, gap)
    union = ImageChops.lighter(ImageChops.lighter(u, bolt), lines)
    outline = dilate(union, ow)

    img = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    img.paste(Image.new('RGBA', (S, S), OUTLINE + (255,)), (0, 0), outline)
    img.paste(gradient(S, MINT_TOP, MINT_BOT), (0, 0), u)
    img.paste(gradient(S, (255, 255, 255), (214, 255, 244)), (0, 0), bolt)
    # streaks fade out toward their tails
    fade = Image.linear_gradient('L').rotate(90, expand=True).resize((S, S)).point(lambda v: int(60 + v * 0.75))
    img.paste(gradient(S, MINT_TOP, MINT_BOT), (0, 0), ImageChops.multiply(lines, fade.transpose(Image.FLIP_LEFT_RIGHT)))
    return img.resize((size, size), Image.LANCZOS)


def write_ico(images, path):
    """ICO with one PNG-compressed image per size (supported since Windows Vista)."""
    blobs = []
    for im in images:
        buf = io.BytesIO()
        im.save(buf, 'PNG', optimize=True)
        blobs.append(buf.getvalue())
    out = struct.pack('<HHH', 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for im, blob in zip(images, blobs):
        w = im.width if im.width < 256 else 0
        out += struct.pack('<BBBBHHII', w, w, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
    path.write_bytes(out + b''.join(blobs))


if __name__ == '__main__':
    root = Path(__file__).resolve().parent.parent
    assets = root / 'assets'
    assets.mkdir(exist_ok=True)
    images = [render(s) for s in SIZES]
    write_ico(images, assets / 'ultra.ico')
    images[-1].save(assets / 'ultra.png')
    # contact sheet on light and dark backgrounds, for checking legibility
    sheet = Image.new('RGBA', (sum(SIZES) + 12 * len(SIZES) + 12, 2 * 280), (0, 0, 0, 0))
    for row, bg in enumerate([(20, 20, 20, 255), (238, 238, 238, 255)]):
        ImageDraw.Draw(sheet).rectangle([0, row * 280, sheet.width, row * 280 + 279], fill=bg)
        x = 12
        for im in images:
            sheet.alpha_composite(im, (x, row * 280 + (280 - im.height) // 2))
            x += im.width + 12
    sheet.save(assets / 'icon_sheet.png')
    print('wrote', assets / 'ultra.ico')
