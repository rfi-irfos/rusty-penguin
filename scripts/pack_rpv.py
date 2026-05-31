#!/usr/bin/env python3
"""pack_rpv.py — pack raw RGB frames (+ optional PCM) into the .rpv container the
bare-metal kernel player (kernel/src/rpv.rs) decodes. numpy-accelerated.

  * pack(frames, w, h, fps, pcm, arate, ach) -> bytes   (library)
  * CLI: pack a single rgb24 rawvideo blob + an s16le PCM file.

Frame codec: each frame is a delta vs the previous (frame 0 vs black), encoded as
repeated segments { u32 skip; u32 litlen; litlen x u32 pixels }. Pixels are stored
as 0x00RRGGBB (u32 LE), matching fb::pixel. Static regions cost one skip; only the
changed pixels are stored as literals.
"""
import struct, sys
import numpy as np

MAGIC = b"RPV1"

def encode_frame(cur, prev):
    """cur, prev: np.uint32 arrays of npix pixels (0x00RRGGBB)."""
    changed = cur != prev
    if not changed.any():
        return b''
    c = changed.astype(np.int8)
    d = np.diff(np.concatenate(([np.int8(0)], c, [np.int8(0)])))
    starts = np.flatnonzero(d == 1)
    ends   = np.flatnonzero(d == -1)
    seg = bytearray()
    prev_end = 0
    for s, e in zip(starts.tolist(), ends.tolist()):
        skip = s - prev_end
        litlen = e - s
        seg += struct.pack('<II', skip, litlen)
        seg += cur[s:e].astype('<u4').tobytes()
        prev_end = e
    return bytes(seg)

def pack(frames, w, h, fps, pcm=b'', arate=44100, ach=2):
    """frames: 2D np array (nframes x npix) or iterable of npix-length arrays;
    each pixel 0x00RRGGBB. pcm: s16le bytes."""
    npix = w * h
    frames = [np.asarray(fr, dtype=np.uint32).reshape(npix) for fr in frames]
    asamps = len(pcm) // (ach * 2) if ach else 0
    out = bytearray()
    out += MAGIC
    out += struct.pack('<7I', w, h, fps, len(frames), arate, ach, asamps)
    out += pcm
    prev = np.zeros(npix, dtype=np.uint32)
    for fr in frames:
        seg = encode_frame(fr, prev)
        out += struct.pack('<I', len(seg))
        out += seg
        prev = fr
    return bytes(out)

def frames_from_rawvideo(blob, w, h):
    """Split an rgb24 rawvideo blob (ffmpeg -f rawvideo -pix_fmt rgb24) into a 2D
    np.uint32 array, nframes x (w*h), each pixel 0x00RRGGBB."""
    arr = np.frombuffer(blob, dtype=np.uint8)
    total = (arr.size // 3) * 3
    rgb = arr[:total].reshape(-1, 3).astype(np.uint32)
    u32 = (rgb[:, 0] << 16) | (rgb[:, 1] << 8) | rgb[:, 2]
    nframes = u32.size // (w * h)
    return u32[:nframes * w * h].reshape(nframes, w * h)

def main():
    # pack_rpv.py <rawvideo.rgb> <w> <h> <fps> <pcm|-> <out.rpv>
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
