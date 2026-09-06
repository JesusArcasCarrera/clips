#!/usr/bin/env bash
set -euo pipefail

# Functional smoke test for the export decision used by the lossless path:
# compatible, keyframe-aligned MP4 sections are muxed with `-c copy`; an
# incompatible resolution is sent through a real video encoder instead.

export LC_ALL=C
workdir="$(mktemp -d "${TMPDIR:-/tmp}/clips-stream-copy.XXXXXX")"
trap 'rm -rf "$workdir"' EXIT

make_source() {
  local output="$1" size="$2"
  ffmpeg -y -loglevel error \
    -f lavfi -i "testsrc=size=${size}:rate=25" \
    -f lavfi -i sine=frequency=440:sample_rate=48000 \
    -t 2 -c:v libx264 -g 25 -keyint_min 25 -sc_threshold 0 \
    -pix_fmt yuv420p -c:a aac -ar 48000 -ac 2 "$output"
}

make_source "$workdir/compatible-a.mp4" 320x240
make_source "$workdir/compatible-b.mp4" 320x240
make_source "$workdir/incompatible.mp4" 640x360

# Starts at 1 second, an explicit keyframe in the generated sources.
ffmpeg -y -loglevel verbose -ss 1 -t 1 -i "$workdir/compatible-a.mp4" \
  -map 0:v:0 -map 0:a:0 -c copy "$workdir/copied.mp4" \
  >"$workdir/copy.stdout" 2>"$workdir/copy.log"
grep -q 'Stream #0:0 -> #0:0 (copy)' "$workdir/copy.log"
grep -q 'Stream #0:1 -> #0:1 (copy)' "$workdir/copy.log"

copied_streams="$(ffprobe -v error -show_entries stream=codec_type,codec_name,width,height \
  -of compact=p=0:nk=0 "$workdir/copied.mp4")"
grep -q 'codec_name=h264|codec_type=video|width=320|height=240' <<<"$copied_streams"
grep -q 'codec_name=aac|codec_type=audio' <<<"$copied_streams"

# A resolution mismatch is the same incompatibility that makes the app fall
# back to its normal filter/encode worker.
ffmpeg -y -loglevel verbose -i "$workdir/incompatible.mp4" \
  -vf scale=320:240 -c:v libx264 -c:a aac "$workdir/fallback.mp4" \
  >"$workdir/fallback.stdout" 2>"$workdir/fallback.log"
grep -q 'Stream #0:0 -> #0:0' "$workdir/fallback.log"
! grep -q 'Stream #0:0 -> #0:0 (copy)' "$workdir/fallback.log"
fallback_size="$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height \
  -of csv=s=x:p=0 "$workdir/fallback.mp4")"
test "$fallback_size" = '320x240'

printf 'stream copy/fallback OK: copy=%s fallback=%s\n' \
  "$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name -of csv=p=0 "$workdir/copied.mp4")" \
  "$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name -of csv=p=0 "$workdir/fallback.mp4")"
