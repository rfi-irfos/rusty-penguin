#!/usr/bin/env bash
# make_meta_video.sh — turn a source clip into bin/meta.rpv for the bare-metal
# kernel video player (kernel/src/rpv.rs). Needs ffmpeg.
#
#   scripts/make_meta_video.sh <input.mp4|webm> [start] [duration] [out.rpv]
#
# Defaults target the Linus Torvalds OSS-EU-2024 quote (full video OM_8UOPFpqE):
#   start    = 16:59   (the "...start his own operating system in Rust..." line)
#   duration = 45      (seconds)
# If you already trimmed the clip, pass start=0 and duration=its length.
#
# Output: 640x360 (upscales 3x to 1920x1080 on the kernel), 12 fps, plus 44100 Hz
# stereo s16le audio. Writes <out.rpv> (default iso/assets/meta.rpv) and prints
# how to bundle it.
set -euo pipefail

IN="${1:?usage: make_meta_video.sh <input> [start] [duration] [out.rpv]}"
START="${2:-16:59}"
DUR="${3:-45}"
OUT="${4:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/iso/assets/meta.rpv}"
W=640; H=360; FPS=12

command -v ffmpeg >/dev/null || { echo "ffmpeg not found — install it first (sudo apt install ffmpeg)"; exit 1; }
SCRIPTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT

echo "[meta] extracting video → ${W}x${H} @ ${FPS}fps rawvideo (start=$START dur=${DUR}s)"
ffmpeg -nostdin -loglevel error -ss "$START" -t "$DUR" -i "$IN" \
    -vf "scale=${W}:${H}:flags=bicubic,fps=${FPS}" -pix_fmt rgb24 -f rawvideo "$TMP/video.rgb"

echo "[meta] extracting audio → 44100 Hz stereo s16le"
ffmpeg -nostdin -loglevel error -ss "$START" -t "$DUR" -i "$IN" \
    -vn -ar 44100 -ac 2 -f s16le "$TMP/audio.pcm"

echo "[meta] packing .rpv (delta + skip/literal codec)"
python3 "$SCRIPTS/pack_rpv.py" "$TMP/video.rgb" "$W" "$H" "$FPS" "$TMP/audio.pcm" "$OUT"

echo "[meta] done → $OUT"
echo "[meta] bundle it as bin/meta.rpv in the initrd, then boot with the"
echo "       'metavideo' kernel cmdline (or wire a desktop launcher)."
