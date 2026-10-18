#!/bin/sh
#
# These are produced by driving the real `tertre` binary inside a pseudo-terminal
# (examples/screenshot.rs) and recording what it draws, so they show the real
# interface, not a mock-up. Needs ffmpeg. Takes about two minutes.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
cargo build --release --bins --examples >/dev/null
BIN="$ROOT/target/release"

W="$(mktemp -d)"
trap 'rm -rf "$W"' EXIT
export HOME="$W/home" XDG_CONFIG_HOME="$W/home/.config" XDG_DATA_HOME="$W/home/.local/share"
mkdir -p "$HOME/Clips" "$XDG_DATA_HOME/tertre/library"

echo "making a 1080p60 source video from the built-in test film…"
"$BIN/examples/mkvideo" 1920 1080 60 8 |
  ffmpeg -v error -y -f rawvideo -pix_fmt rgb24 -s 1920x1080 -r 60 -i - \
    -f lavfi -i "sine=frequency=330:duration=8,volume=0.4" \
    -c:v libx264 -preset veryfast -pix_fmt yuv420p -crf 20 -c:a aac -shortest "$HOME/Clips/dusk-1080p60.mp4"
cp tests/fixtures/sample.av1 "$HOME/Clips/sample.av1"

# A couple of films already in the library.
"$BIN/tertre" demo -o "$XDG_DATA_HOME/tertre/library/Dusk over the tertre.av1" --seconds 12 --cols 100 >/dev/null 2>&1
"$BIN/tertre" convert "$HOME/Clips/dusk-1080p60.mp4" -o "$XDG_DATA_HOME/tertre/library/Dusk (mono).av1" --color mono --cols 120 --fps 24 -q >/dev/null

OUT="$ROOT/docs/img"
mkdir -p "$OUT"
rm -rf "$W/shots"
TERTRE_BIN="$BIN/tertre" "$BIN/examples/screenshot" "$W/shots" 124x38 \
  "wait:4300 snap:home \
   key:s wait:2500 snap:settings key:esc wait:300 \
   key:l wait:1800 snap:library key:esc wait:300 \
   key:c wait:500 key:enter wait:500 snap:picker key:enter wait:4000 snap:studio \
   key:down key:down key:right key:right key:right key:down \
   key:right key:right key:right key:right key:right key:right key:right key:right key:right key:right \
   key:right key:right key:right key:right key:right key:right key:right key:right key:right key:right \
   wait:2500 snap:studio-60fps \
   key:enter wait:1300 snap:converting wait:7000 snap:converted \
   key:enter wait:2500 snap:player key:i wait:600 snap:player-hud key:space wait:500 snap:player-paused key:q wait:800 \
   key:esc wait:300 key:q wait:500"

for n in home settings library picker studio studio-60fps converting converted player player-hud player-paused; do
  cp "$W/shots/$n.svg" "$OUT/$n.svg"
done
