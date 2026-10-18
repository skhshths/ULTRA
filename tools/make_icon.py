"""Generate ULTRA's app icons for both platforms.

The mark: a bold, round-bottomed U in ULTRA's mint on a dark tile, with a soft glow. Big and
flat on purpose, so it stays recognizable down to 16px in the taskbar.

Two variants: the normal icon (mint U on a dark tile), and an inverted one (dark U on a light
tile) for the experimental build.

Writes windows/assets/ultra.ico, ultra-inv.ico and mac/assets/ultra.icns, ultra-inv.icns,
plus 256px PNGs (ultra.png; on the Mac also ultra-inv.png, for the Dock) and a contact sheet
(icon_sheet.png) in each folder.

Run from anywhere: python tools/make_icon.py   (needs Pillow)
"""

import io
import math
import struct
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

WIN_SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
MAC_SIZES = [16, 32, 64, 128, 256, 512, 1024]

DARK = {
    'tile_top': (34, 34, 34), 'tile_bot': (12, 12, 12), 'edge': (159, 227, 208, 70),
    'u_top': (196, 255, 238), 'u_bot': (110, 200, 176), 'glow': (159, 227, 208),
}
LIGHT = {
    'tile_top': (246, 246, 246), 'tile_bot': (216, 216, 216), 'edge': (20, 80, 68, 90),
    'u_top': (20, 92, 78), 'u_bot': (6, 48, 40), 'glow': (40, 120, 102),
}


def mix(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def gradient(S, top, bot):
    g = Image.new('RGBA', (1, S))
    for y in range(S):
        g.putpixel((0, y), mix(top, bot, y / (S - 1)) + (255,))
    return g.resize((S, S))


def render(size, pal):
    ss = 8 if size <= 64 else 4 if size <= 256 else 2
    S = size * ss
    img = Image.new('RGBA', (S, S), (0, 0, 0, 0))

    # Tile: rounded square, soft vertical gradient, faint accent edge.
    m = 0.03 if size <= 32 else 0.07
    box = [m * S, m * S, (1 - m) * S, (1 - m) * S]
    rad = 0.22 * (1 - 2 * m) * S
    tile = Image.new('L', (S, S), 0)
    ImageDraw.Draw(tile).rounded_rectangle(box, rad, fill=255)
    img.paste(gradient(S, pal['tile_top'], pal['tile_bot']), (0, 0), tile)
    edge = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(edge).rounded_rectangle(box, rad, outline=pal['edge'], width=int(max(ss * 0.9, S * 0.008)))
    img.alpha_composite(edge)

    # The U: thick, with a round bottom; bolder at small sizes.
    k = 1 - 2 * m
    small = size <= 32
    w = (0.60 if small else 0.52) * k             # overall width
    h = (0.62 if small else 0.56) * k             # overall height
    st = (0.205 if small else 0.155) * k          # stroke
    x0, y0 = 0.5 - w / 2, 0.5 - h / 2
    x1, y1 = x0 + w, y0 + h
    P = lambda *v: [c * S for c in v]
    mask = Image.new('L', (S, S), 0)
    d = ImageDraw.Draw(mask)
    d.rectangle(P(x0, y0 + st / 2, x1, y1 - w / 2), fill=255)
    d.ellipse(P(x0, y1 - w, x1, y1), fill=255)
    d.rectangle(P(x0 + st, y0 - 0.01, x1 - st, y1 - w / 2), fill=0)
    d.ellipse(P(x0 + st, y1 - w + st, x1 - st, y1 - st), fill=0)
    # rounded tops on both arms
    d.ellipse(P(x0, y0, x0 + st, y0 + st), fill=255)
    d.ellipse(P(x1 - st, y0, x1, y0 + st), fill=255)

    # soft glow behind the U (not at tiny sizes, where it would just blur)
    if size >= 32:
        glow = Image.new('RGBA', (S, S), pal['glow'] + (0,))
        glow.putalpha(mask.filter(ImageFilter.GaussianBlur(S * 0.045)).point(lambda a: int(a * 0.45)))
        glow.putalpha(ImageChops.multiply(glow.getchannel('A'), tile))
        img.alpha_composite(glow)
    u = gradient(S, pal['u_top'], pal['u_bot'])
    u.putalpha(mask)
    img.alpha_composite(u)
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
        wi[-1].save(mac / f'ultra{suffix}.png')  # the Mac sets its Dock icon from this at runtime
        rows.append(wi)
    sheet(rows, win / 'icon_sheet.png')
    sheet(rows, mac / 'icon_sheet.png')
    print('wrote icons to', win, 'and', mac)
