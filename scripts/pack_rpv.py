#!/usr/bin/env python3
"""pack_rpv.py — pack raw RGB frames (+ optional PCM) into the .rpv container the
bare-metal kernel player (kernel/src/rpv.rs) decodes.

Two entry points:
  * pack(frames, w, h, fps, pcm, arate, ach) -> bytes   (library)
  * CLI: pack a directory of WxH raw RGB24 frames + a raw s16le PCM file.

Frame codec: each frame is a delta vs the previous (frame 0 vs black), encoded as
repeated segments { u32 skip; u32 litlen; litlen x u32 pixels }. Pixels are stored
as 0x00RRGGBB (u32 LE), matching fb::pixel. Static regions cost one skip; only the
changed pixels are stored as literals.
"""
import struct, sys, os

MAGIC = b"RPV1"

def encode_frame(cur, prev):
    seg = bytearray()
    n = len(cur)
    i = 0
    while i < n:
        skip = 0
        while i < n and cur[i] == prev[i]:
            skip += 1; i += 1
        if i == n:
            break  # trailing unchanged — nothing to emit
        lit_start = i
        while i < n and cur[i] != prev[i]:
            i += 1
        litlen = i - lit_start
        seg += struct.pack('<II', skip, litlen)
        seg += struct.pack('<%dI' % litlen, *cur[lit_start:lit_start + litlen])
    return bytes(seg)

def pack(frames, w, h, fps, pcm=b'', arate=44100, ach=2):
    """frames: list of lists, each w*h ints (0x00RRGGBB). pcm: s16le bytes."""
    npix = w * h
    asamps = len(pcm) // (ach * 2) if ach else 0
    out = bytearray()
    out += MAGIC
    out += struct.pack('<7I', w, h, fps, len(frames), arate, ach, asamps)
    out += pcm
    prev = [0] * npix
    for fr in frames:
        assert len(fr) == npix, "frame size mismatch"
        seg = encode_frame(fr, prev)
        out += struct.pack('<I', len(seg))
        out += seg
        prev = fr
    return bytes(out)

def frames_from_rawvideo(blob, w, h):
    """Split a concatenated rgb24 rawvideo blob (ffmpeg -f rawvideo -pix_fmt
    rgb24) into a list of frames, each w*h ints (0x00RRGGBB)."""
    npix = w * h
    fbytes = npix * 3
    nframes = len(blob) // fbytes
    frames = []
    for f in range(nframes):
        base = f * fbytes
        fr = [0] * npix
        for i in range(npix):
            o = base + i * 3
            fr[i] = (blob[o] << 16) | (blob[o + 1] << 8) | blob[o + 2]
        frames.append(fr)
    return frames

def main():
    # pack_rpv.py <rawvideo.rgb> <w> <h> <fps> <pcm|-> <out.rpv>
    # rawvideo = concatenated rgb24 frames (ffmpeg -f rawvideo -pix_fmt rgb24).
    if len(sys.argv) != 7:
        print("usage: pack_rpv.py <rawvideo.rgb> <w> <h> <fps> <pcm|-> <out.rpv>")
        sys.exit(1)
    blobpath, w, h, fps, pcmpath, out = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), sys.argv[5], sys.argv[6]
    blob = open(blobpath, 'rb').read()
    frames = frames_from_rawvideo(blob, w, h)
    pcm = b'' if pcmpath == '-' else open(pcmpath, 'rb').read()
    data = pack(frames, w, h, fps, pcm)
    open(out, 'wb').write(data)
    print("wrote %s: %dx%d %d frames, %d bytes (pcm %d)" % (out, w, h, len(frames), len(data), len(pcm)))

if __name__ == '__main__':
    main()
