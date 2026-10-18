#!/bin/sh
# tertre installer — Terminal Theatre by Sarja Technologies.
#
#   curl -fsSL https://raw.githubusercontent.com/sarja-technologies/tertre/main/install.sh | sh
#
# Options (pass after `sh -s --` when piping):
#   --prefix DIR     install to DIR/bin            (default: ~/.local)
#   --system         install to /usr/local (uses sudo when needed)
#   --version vX.Y.Z install that release          (default: latest)
#   --from-source    build with cargo instead of downloading a binary
#   --uninstall      remove tertre from the prefix
#   -h, --help
set -eu

REPO="sarja-technologies/tertre"
PREFIX="${HOME}/.local"
VERSION="latest"
FROM_SOURCE=0
UNINSTALL=0

say() { printf '\033[1;33m▌tertre▐\033[0m %s\n' "$*"; }
warn() { printf '\033[1;31m!\033[0m %s\n' "$*" >&2; }
die() { warn "$*"; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    --system) PREFIX="/usr/local"; shift ;;
    --version) VERSION="$2"; shift 2 ;;
    --from-source) FROM_SOURCE=1; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
done

BIN_DIR="$PREFIX/bin"
SUDO=""
as_root_if_needed() {
  if [ -w "$PREFIX" ] || { [ ! -e "$PREFIX" ] && [ -w "$(dirname "$PREFIX")" ]; }; then SUDO=""; else
    command -v sudo >/dev/null 2>&1 || die "$PREFIX is not writable and sudo is not available (try --prefix \$HOME/.local)"
    SUDO="sudo"
  fi
}

if [ "$UNINSTALL" = 1 ]; then
  as_root_if_needed
  $SUDO rm -f "$BIN_DIR/tertre"
  say "removed $BIN_DIR/tertre (your config in ~/.config/tertre and library in ~/.local/share/tertre were left alone)"
  exit 0
fi

[ "$(uname -s)" = "Linux" ] || die "this installer targets Linux; on other systems build from source: cargo install --git https://github.com/$REPO --locked"
case "$(uname -m)" in
  x86_64|amd64) ARCH="x86_64" ;;
  aarch64|arm64) ARCH="aarch64" ;;
  *) ARCH=""; FROM_SOURCE=1 ;;
esac

need() { command -v "$1" >/dev/null 2>&1; }

install_from_source() {
  need cargo || die "cargo not found. Install Rust from https://rustup.rs and re-run, or use a release binary."
  if ! need pkg-config || ! pkg-config --exists alsa 2>/dev/null; then
    warn "building needs the ALSA development files:  sudo apt install libasound2-dev pkg-config   (Fedora: alsa-lib-devel)"
  fi
  say "building from source with cargo (this takes a minute)…"
  cargo install --git "https://github.com/$REPO" --locked --root "$PREFIX" tertre
}

install_from_release() {
  need curl || need wget || die "need curl or wget"
  need tar || die "need tar"
  if [ "$VERSION" = "latest" ]; then
    # Asset names embed the tag, so ask the API which tag is the latest.
    TAG="$( (curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null || true) | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)"
    [ -n "$TAG" ] || return 1
  else
    TAG="$VERSION"
  fi
  NAME="tertre-$TAG-$ARCH-unknown-linux-gnu"
  URL="https://github.com/$REPO/releases/download/$TAG/$NAME.tar.gz"
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT
  say "downloading $NAME"
  if need curl; then curl -fsSL "$URL" -o "$TMP/t.tar.gz" || return 1; curl -fsSL "$URL.sha256" -o "$TMP/t.sha256" || true
  else wget -q "$URL" -O "$TMP/t.tar.gz" || return 1; wget -q "$URL.sha256" -O "$TMP/t.sha256" || true; fi
  if [ -s "$TMP/t.sha256" ] && need sha256sum; then
    want="$(cut -d' ' -f1 "$TMP/t.sha256")"
    got="$(sha256sum "$TMP/t.tar.gz" | cut -d' ' -f1)"
    [ "$want" = "$got" ] || die "checksum mismatch for $NAME.tar.gz — refusing to install"
    say "checksum ok"
  else
    warn "no checksum available; continuing"
  fi
  tar -xzf "$TMP/t.tar.gz" -C "$TMP"
  as_root_if_needed
  $SUDO mkdir -p "$BIN_DIR"
  $SUDO install -m 0755 "$TMP/$NAME/tertre" "$BIN_DIR/tertre"
}

as_root_if_needed
if [ "$FROM_SOURCE" = 1 ]; then
  install_from_source
elif ! install_from_release; then
  warn "no prebuilt binary could be downloaded; falling back to building from source"
  install_from_source
fi

say "installed: $BIN_DIR/tertre"
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) warn "$BIN_DIR is not on your PATH. Add this to your shell profile:  export PATH=\"$BIN_DIR:\$PATH\"" ;; esac

# Runtime dependencies.
need ffmpeg || warn "ffmpeg not found: playing .av1 files works, converting videos needs it:  sudo apt install ffmpeg"
ldconfig -p 2>/dev/null | grep -q 'libasound.so.2' || warn "libasound2 not found: needed for audio:  sudo apt install libasound2"
say "all set. Try:  tertre doctor   ·   tertre demo && ./dusk.av1   ·   tertre"
