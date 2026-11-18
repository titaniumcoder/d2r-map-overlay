"""Decode a DC6 (classic sprite) into a horizontal PNG strip, frame i at x = i * max width.

Usage: python3 -I research/dc6.py <in.dc6> <pal.dat> <out.png>  (needs Pillow; pal.dat from
data\\global\\palette\\act1 via research/mpq.py). invps.DC6 gives the 7 tomb symbols, frame i = level 66 + i.
"""
import struct, sys
from PIL import Image
data = open(sys.argv[1], 'rb').read()
pal = open(sys.argv[2], 'rb').read()
ver, flags, enc, term, dirs, fpd = struct.unpack_from('<iIIIII', data, 0)
n = dirs * fpd
ptrs = struct.unpack_from(f'<{n}I', data, 24)
frames = []
for p in ptrs:
    flip, w, h, ox, oy, unk, nxt, ln = struct.unpack_from('<IiiiiIII', data, p)
    px = {}
    body = data[p + 32: p + 32 + ln]
    x, y = 0, h - 1
    i = 0
    while i < len(body):
        c = body[i]; i += 1
        if c == 0x80:
            x = 0; y -= 1
        elif c & 0x80:
            x += c & 0x7F
        else:
            for k in range(c):
                px[(x, y)] = body[i]; i += 1; x += 1
    frames.append((w, h, ox, oy, px))
    print(f'frame w={w} h={h} off=({ox},{oy}) opaque={len(px)}')
mw = max(f[0] for f in frames); mh = max(f[1] for f in frames)
img = Image.new('RGBA', (mw * n, mh), (0, 0, 0, 0))
for k, (w, h, ox, oy, px) in enumerate(frames):
    for (x, y), v in px.items():
        b, g, r = pal[v*3], pal[v*3+1], pal[v*3+2]
        img.putpixel((k * mw + x, y), (r, g, b, 255))
img.save(sys.argv[3])
