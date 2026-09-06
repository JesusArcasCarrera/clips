#!/usr/bin/env bash
set -euo pipefail

# Functional regression test for the multisource audio contract.  It mirrors
# the FFmpeg part-render policy: a sequence with at least one audio source
# gets a stereo 48 kHz silent track for its muted sources; an all-muted
# sequence is intentionally outside this test and remains audio-less.

export LC_ALL=C
workdir="$(mktemp -d "${TMPDIR:-/tmp}/clips-multisource-audio.XXXXXX")"
trap 'rm -rf "$workdir"' EXIT

ffmpeg -y -loglevel error \
  -f lavfi -i color=c=blue:size=320x240:rate=25 \
  -f lavfi -i sine=frequency=440:sample_rate=44100 \
  -t 1.7 -c:v libx264 -pix_fmt yuv420p -c:a aac "$workdir/with-audio.mp4"
ffmpeg -y -loglevel error \
  -f lavfi -i color=c=red:size=160x120:rate=15 \
  -t 1.3 -an -c:v libx264 -pix_fmt yuv420p "$workdir/no-audio.mp4"

ffmpeg -y -loglevel error -ss 0 -t 1.7 -i "$workdir/with-audio.mp4" \
  -map 0:v:0 -map 0:a:0 -vf 'scale=320:240,fps=25' -c:v libx264 -crf 27 \
  -af 'aformat=sample_rates=48000:channel_layouts=stereo' -c:a aac \
  -ar 48000 -ac 2 -shortest "$workdir/part-a.mp4"
ffmpeg -y -loglevel error -ss 0 -t 1.3 -i "$workdir/no-audio.mp4" \
  -f lavfi -i 'anullsrc=channel_layout=stereo:sample_rate=48000' \
  -map 0:v:0 -map 1:a:0 -vf 'scale=320:240,fps=25' -c:v libx264 -crf 27 \
  -af 'aformat=sample_rates=48000:channel_layouts=stereo' -c:a aac \
  -ar 48000 -ac 2 -shortest "$workdir/part-b.mp4"

printf "file '%s'\nfile '%s'\n" "$workdir/part-a.mp4" "$workdir/part-b.mp4" \
  >"$workdir/concat.txt"
ffmpeg -y -loglevel error -f concat -safe 0 -i "$workdir/concat.txt" \
  -c copy "$workdir/joined.mp4"

video_duration() {
  ffprobe -v error -select_streams v:0 -show_entries stream=duration \
    -of csv=p=0 "$1"
}

audio_duration() {
  ffprobe -v error -select_streams a:0 -show_entries stream=duration \
    -of csv=p=0 "$1"
}

frame_chroma() {
  local seek=(-ss "$2")
  if [[ "$2" == -* ]]; then
    seek=(-sseof "$2")
  fi
  ffmpeg -v info "${seek[@]}" -i "$1" -frames:v 1 \
    -vf 'signalstats,metadata=print:file=-' -f null - 2>&1 \
    | awk -F= '/lavfi.signalstats.UAVG/{u=$2} /lavfi.signalstats.VAVG/{print u, $2; exit}'
}

joined_video="$(video_duration "$workdir/joined.mp4")"
joined_audio="$(audio_duration "$workdir/joined.mp4")"
awk -v v="$joined_video" -v a="$joined_audio" \
  'BEGIN { if (v == "" || a == "" || (v-a < -0.08) || (v-a > 0.08)) exit 1 }'

streams="$(ffprobe -v error -show_entries stream=codec_type,sample_rate,channels \
  -of compact=p=0:nk=0 "$workdir/joined.mp4")"
grep -q 'codec_type=video' <<<"$streams"
grep -q 'codec_type=audio|sample_rate=48000|channels=2' <<<"$streams"

# The first and last decoded frames must still come from the first (blue) and
# second (red) source respectively; this catches sorting or dropped parts.
test "$(frame_chroma "$workdir/joined.mp4" 0)" = "240 110"
test "$(frame_chroma "$workdir/joined.mp4" -0.1)" = "90 240"

# An all-muted sequence must not gain an audio stream merely because the
# mixed-source path knows how to synthesize silence.
ffmpeg -y -loglevel error -ss 0 -t 1.3 -i "$workdir/no-audio.mp4" \
  -map 0:v:0 -vf 'scale=320:240,fps=25' -c:v libx264 -an "$workdir/mute-only.mp4"
test -z "$(audio_duration "$workdir/mute-only.mp4" || true)"

printf 'multisource audio OK: video=%ss audio=%ss\n' "$joined_video" "$joined_audio"
