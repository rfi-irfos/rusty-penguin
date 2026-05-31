# The meta video — playing the founding clip on the kernel it triggered

The most meta shot we can take: the Linus Torvalds keynote line that started this
OS ("...start his own operating system in Rust...", OSS EU 2024 — see
[ORIGIN.md](ORIGIN.md)) playing **on the bare-metal pure-Rust kernel itself**. No
browser, no Linux, no external codec, no web stack — just our framebuffer and our
HDA audio.

![The keynote playing on the Rusty Penguin kernel](meta-video-on-rusty-penguin.png)

*Done. The real keynote (Torvalds + Hohndel on stage) decoded by our from-scratch
`.rpv` codec and blitted to our own framebuffer — captured via QEMU `screendump`,
540 frames at 640×360, zero faults.*

## How it works

The clip is pre-decoded offline into a tiny from-scratch container, `.rpv`
("Rusty Penguin Video"), and bundled in the initrd as `bin/meta.rpv`. The kernel
player (`kernel/src/rpv.rs`) parses it, decodes each frame, blits to the
framebuffer (640×360 nearest-neighbour upscaled 3× to 1920×1080), paces off the
PIT timer, and streams PCM into the HDA audio ring.

`.rpv` codec: each frame is a delta vs the previous one (frame 0 vs black),
encoded as `{ skip, litlen, litlen×pixels }` segments — a talking-head clip is
mostly static, so the background costs one `skip` and only the moving regions
carry literal pixels. Decoding is a few dozen lines, no dependencies.

## Make the real clip (needs ffmpeg)

```bash
# 1. drop the source video somewhere, e.g. ~/torvalds.mp4
# 2. extract + pack (defaults target the quote at 16:59, 45 s):
scripts/make_meta_video.sh ~/torvalds.mp4            # → iso/assets/meta.rpv
#    or pass start/duration/out:
scripts/make_meta_video.sh ~/torvalds.mp4 16:59 45 iso/assets/meta.rpv
# 3. bundle iso/assets/meta.rpv as bin/meta.rpv in the initrd, then boot with the
#    'metavideo' kernel cmdline (or wire a desktop launcher).
```

## Verify without the real clip

```bash
python3 scripts/gen_synth_rpv.py /tmp/meta.rpv   # synthetic test clip, no ffmpeg
# build a tiny ISO with bin/meta.rpv + the 'metavideo' cmdline and boot it;
# serial shows "[rpv] ... playback complete" with no exceptions.
```

The player is proven end-to-end on the synthetic clip (30 frames decode + blit +
timer pacing, zero faults). When the real clip is dropped, it's a single
`make_meta_video.sh` run away from the meta shot.
