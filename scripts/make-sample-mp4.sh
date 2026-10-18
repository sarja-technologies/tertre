#!/bin/sh
# Make a small MP4 (H.264 + AAC, 640x360, 30 fps, 10 s) from tertre's procedural test film.
# Needs ffmpeg. Usage: scripts/make-sample-mp4.sh [out.mp4]
set -eu
out="${1:-sample.mp4}"
here="$(cd "$(dirname "$0")/.." && pwd)"
cargo build --release --example mkvideo --manifest-path "$here/Cargo.toml" >/dev/null
"$here/target/release/examples/mkvideo" 640 360 30 10 |
  ffmpeg -v error -y -f rawvideo -pix_fmt rgb24 -s 640x360 -r 30 -i - \
    -f lavfi -i "sine=frequency=220:duration=10,volume=0.4" \
    -c:v libx264 -pix_fmt yuv420p -crf 20 -c:a aac -b:a 96k -shortest -movflags +faststart "$out"
echo "wrote $out"
