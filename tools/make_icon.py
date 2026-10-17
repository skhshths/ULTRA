"""Generate ULTRA's app icons for both platforms.

The mark: a dark rounded tile holding a swirl of mint ASCII characters, the same vortex the
browser draws as its background, spiralling into a glowing core. Each size is drawn on its
own (supersampled): from 96px up the arms are made of real glyphs, below that, where letters
can't be read, they become smooth tapered strokes so the swirl stays crisp at 16px.

Two variants: the normal icon, and an inverted one (light tile, dark swirl) for the
experimental build, matching its inverted colors.

Writes windows/assets/ultra.ico, ultra-inv.ico and mac/assets/ultra.icns, ultra-inv.icns,
plus a 256px preview (ultra.png) and a contact sheet (icon_sheet.png) in each folder.

Run from anywhere: python tools/make_icon.py   (needs Pillow)
"""

import io
import math
import struct
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

WIN_SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
MAC_SIZES = [16, 32, 64, 128, 256, 512, 1024]
GLYPHS = '0123456789abcdefghijklmnopqrstuvwxyz'
ARMS = 3
TURNS = 1.15  # how far each arm winds around the core

DARK = {
    'tile_top': (30, 30, 30), 'tile_bot': (12, 12, 12), 'edge': (159, 227, 208, 90),
    'arm_hi': (196, 255, 238), 'arm_lo': (63, 125, 110), 'core': (230, 255, 248), 'glow': (159, 227, 208),
}
LIGHT = {
    'tile_top': (242, 242, 242), 'tile_bot': (214, 214, 214), 'edge': (20, 80, 68, 110),
    'arm_hi': (10, 60, 50), 'arm_lo': (90, 150, 136), 'core': (6, 40, 33), 'glow': (40, 120, 102),
}


def font(px):
    for f in ['C:/Windows/Fonts/consolab.ttf', 'C:/Windows/Fonts/CascadiaMono.ttf',
              '/System/Library/Fonts/Menlo.ttc', '/Library/Fonts/Menlo.ttc']:
        try:
            return ImageFont.truetype(f, max(1, int(px)))
        except OSError:
            continue
    return ImageFont.load_default()


def mix(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def spiral(arm, t, R):
    """Point on arm `arm` at 0..1 along it (0 = outer end, 1 = core). Returns x, y, radius."""
    theta = arm * 2 * math.pi / ARMS + t * TURNS * 2 * math.pi
    r = R * (1 - t) ** 1.35  # tightens toward the core
    return 0.5 + r * math.cos(theta), 0.5 + r * math.sin(theta), r


def render(size, pal):
    ss = 8 if size <= 64 else 4 if size <= 256 else 2
    S = size * ss
    img = Image.new('RGBA', (S, S), (0, 0, 0, 0))

    # Tile: rounded square with a soft vertical gradient and a faint accent edge.
    m = 0.04 if size <= 32 else 0.07
    box = [m * S, m * S, (1 - m) * S, (1 - m) * S]
    rad = 0.22 * (1 - 2 * m) * S
    tile = Image.new('L', (S, S), 0)
    ImageDraw.Draw(tile).rounded_rectangle(box, rad, fill=255)
    grad = Image.new('RGBA', (1, S))
    for y in range(S):
        grad.putpixel((0, y), mix(pal['tile_top'], pal['tile_bot'], y / (S - 1)) + (255,))
    img.paste(grad.resize((S, S)), (0, 0), tile)
    edge_w = max(ss * 0.9, S * 0.008)
    edge = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(edge).rounded_rectangle(box, rad, outline=pal['edge'], width=int(edge_w))
    img.alpha_composite(edge)

    R = 0.365 * (1 - 2 * m) / (1 - 2 * 0.07)  # swirl radius (fraction of S)
    layer = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    if size >= 96:
        # Arms of glyphs: big and dim on the outside, small and bright near the core.
        seed = 7
        for arm in range(ARMS):
            t = 0.0
            while t < 0.93:
                x, y, r = spiral(arm, t, R)
                px = S * (0.020 + 0.13 * r)
                f = font(px)
                seed = (seed * 1103515245 + 12345) & 0x7fffffff
                ch = GLYPHS[seed % len(GLYPHS)]
                col = mix(pal['arm_lo'], pal['arm_hi'], t ** 0.8)
                alpha = int(110 + 145 * t)
                d.text((x * S, y * S), ch, font=f, fill=col + (alpha,), anchor='mm')
                # step so neighbouring glyphs don't overlap: proportional to glyph size
                t += (px * 0.62) / (S * max(0.02, 2 * math.pi * TURNS * r * 0.55 + 0.25 * R))
        # faint dust between the arms, like the background's scattered glyphs
        for i in range(70):
            seed = (seed * 1103515245 + 12345) & 0x7fffffff
            a = (seed % 3600) / 3600 * 2 * math.pi
            r = R * (0.25 + 0.75 * ((seed >> 12) % 1000) / 1000)
            x, y = 0.5 + r * math.cos(a), 0.5 + r * math.sin(a)
            d.text((x * S, y * S), GLYPHS[seed % len(GLYPHS)], font=font(S * 0.028),
                   fill=pal['arm_lo'] + (55,), anchor='mm')
    else:
        # Small sizes: each arm is one tapered stroke, thick outside, thin at the core.
        for arm in range(ARMS):
            steps = 60
            for i in range(steps):
                t0, t1 = i / steps * 0.92, (i + 1) / steps * 0.92
                x0, y0, r0 = spiral(arm, t0, R)
                x1, y1, _ = spiral(arm, t1, R)
                w = S * (0.026 + 0.15 * r0)
                col = mix(pal['arm_lo'], pal['arm_hi'], t0 ** 0.7)
                d.line([(x0 * S, y0 * S), (x1 * S, y1 * S)], fill=col + (255,), width=max(1, int(w)))
                d.ellipse([x1 * S - w / 2, y1 * S - w / 2, x1 * S + w / 2, y1 * S + w / 2], fill=col + (255,))
    # keep the swirl inside the tile
    layer.putalpha(ImageChops.multiply(layer.getchannel('A'), tile))
    img.alpha_composite(layer)

    # Core: a soft glow with a bright centre.
    glow = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    gr = S * 0.11
    ImageDraw.Draw(glow).ellipse([S / 2 - gr, S / 2 - gr, S / 2 + gr, S / 2 + gr], fill=pal['glow'] + (150,))
    glow = glow.filter(ImageFilter.GaussianBlur(S * 0.05))
    glow.putalpha(ImageChops.multiply(glow.getchannel('A'), tile))
    img.alpha_composite(glow)
    cr = S * (0.055 if size <= 32 else 0.038)
    ImageDraw.Draw(img).ellipse([S / 2 - cr, S / 2 - cr, S / 2 + cr, S / 2 + cr], fill=pal['core'] + (255,))
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


# ICNS entry types holding PNG data: (OSType, pixel size). Retina variants reuse a size.
ICNS_TYPES = [(b'icp4', 16), (b'icp5', 32), (b'icp6', 64), (b'ic07', 128), (b'ic08', 256),
              (b'ic09', 512), (b'ic10', 1024), (b'ic11', 32), (b'ic12', 64), (b'ic13', 256), (b'ic14', 512)]


def write_icns(images, path):
    """ICNS with one PNG per entry (the format every macOS since 10.7 reads)."""
    by_size = {im.width: im for im in images}
    body = b''
    for kind, size in ICNS_TYPES:
        buf = io.BytesIO()
        by_size[size].save(buf, 'PNG', optimize=True)
        blob = buf.getvalue()
        body += kind + struct.pack('>I', len(blob) + 8) + blob
    path.write_bytes(b'icns' + struct.pack('>I', len(body) + 8) + body)


def sheet(rows, path):
    """Every size of each variant, on dark and light backgrounds, to check legibility."""
    width = max(sum(im.width + 12 for im in r) for r in rows) + 12
    out = Image.new('RGBA', (width, 280 * 2 * len(rows)), (0, 0, 0, 0))
    y = 0
    for r in rows:
        for bg in [(20, 20, 20, 255), (238, 238, 238, 255)]:
            ImageDraw.Draw(out).rectangle([0, y, width, y + 279], fill=bg)
            x = 12
            for im in r:
                out.alpha_composite(im, (x, y + (280 - im.height) // 2))
                x += im.width + 12
            y += 280
    out.save(path)


if __name__ == '__main__':
    root = Path(__file__).resolve().parent.parent
    win, mac = root / 'windows' / 'assets', root / 'mac' / 'assets'
    rows = []
    for pal, suffix in [(DARK, ''), (LIGHT, '-inv')]:
        wi = [render(s, pal) for s in WIN_SIZES]
        write_ico(wi, win / f'ultra{suffix}.ico')
        mi = [render(s, pal) if s not in WIN_SIZES else wi[WIN_SIZES.index(s)] for s in MAC_SIZES]
        write_icns(mi, mac / f'ultra{suffix}.icns')
        if not suffix:
            wi[-1].save(win / 'ultra.png')
            wi[-1].save(mac / 'ultra.png')
        rows.append(wi)
    sheet(rows, win / 'icon_sheet.png')
    sheet(rows, mac / 'icon_sheet.png')
    print('wrote icons to', win, 'and', mac)
