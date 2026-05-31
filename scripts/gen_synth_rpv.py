#!/usr/bin/env python3
"""gen_synth_rpv.py — generate a tiny synthetic .rpv (no ffmpeg) to verify the
bare-metal kernel video player before the real clip is available.

Produces moving colour bars + a sliding white block over a gradient, so the
delta codec sees both large static regions (skips) and changing regions
(literals). Writes /tmp/meta.rpv by default.
"""
import sys, struct
sys.path.insert(0, __file__.rsplit('/', 1)[0])
from pack_rpv import pack

W, H, FPS, N = 320, 180, 15, 30

def frame(t):
    px = [0] * (W * H)
    bar = (t * 7) % W
    for y in range(H):
        for x in range(W):
            # gradient background (static-ish vertical) + moving colour bar
            r = (x * 255) // W
            g = (y * 255) // H
            b = 64
            if abs(((x + bar) % W) - W // 2) < 8:
                r, g, b = 255, 230, 40       # moving yellow bar
            # sliding white block
            bx = (t * 9) % (W - 40)
            if bx <= x < bx + 40 and H // 2 - 20 <= y < H // 2 + 20:
                r, g, b = 255, 255, 255
            px[y * W + x] = (r << 16) | (g << 8) | b
    return px

def main():
    out = sys.argv[1] if len(sys.argv) > 1 else '/tmp/meta.rpv'
    frames = [frame(t) for t in range(N)]
    data = pack(frames, W, H, FPS, pcm=b'')   # no audio in the synthetic test
    open(out, 'wb').write(data)
    print("wrote %s: %dx%d %d frames @ %dfps, %d bytes" % (out, W, H, N, FPS, len(data)))

if __name__ == '__main__':
    main()
