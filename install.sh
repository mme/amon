#!/bin/sh
# Installs amon: the latest release binary into ~/.local/bin, no root.
#
#   curl -fsSL amon.sh/install | sh
#
# Running it again is also how amon upgrades: an existing install is replaced
# in place and `amon setup --upgrade` refreshes what setup put on disk. A
# first install hands off to the interactive `amon setup` instead, when a
# terminal is there to run it on.
#
# ~/.local/bin because Omarchy puts it on PATH for every login shell and the
# whole session (default/bash/env-bootstrap appends it). Override the
# directory with AMON_PREFIX, pin a version with AMON_VERSION=vX.Y.Z.
#
# The tarball's name and its sha256 sidecar are contracts with the publish
# workflow; a test holds the two files to each other. AMON_BASE_URL exists
# for that test's mock server and is not part of the interface.
set -eu

REPO_URL="${AMON_BASE_URL:-https://github.com/mme/amon}"
BIN_DIR="${AMON_PREFIX:-$HOME/.local/bin}"
SHARE_DIR="$HOME/.local/share/amon"

OS="$(uname -s)"
ARCH="$(uname -m)"
[ "$OS" = "Linux" ] || { echo "amon runs on Linux (this is $OS)" >&2; exit 1; }
[ "$ARCH" = "x86_64" ] || {
  echo "prebuilt amon is x86_64 only (this is $ARCH)." >&2
  echo "build from source instead: https://github.com/mme/amon#building" >&2
  exit 1
}

TAG="${AMON_VERSION:-}"
if [ -z "$TAG" ]; then
  # The latest tag, read from where the stable /releases/latest URL lands.
  # No API call: nothing to rate-limit, nothing to parse.
  LATEST="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$REPO_URL/releases/latest")"
  TAG="${LATEST##*/}"
fi
case "$TAG" in
  v[0-9]*) ;;
  *) echo "expected a release tag like v0.1.0, got '$TAG'" >&2; exit 1 ;;
esac

# Whether this is an upgrade, decided by what is on disk before the swap.
# The old version is captured now - the binary that can answer is about to be
# replaced - and used only for the closing message: `amon setup --upgrade`
# reads disk state instead, so a hand-swapped binary heals the same way.
UPGRADING=0
PREVIOUS=""
if [ -x "$BIN_DIR/amon" ]; then
  UPGRADING=1
  PREVIOUS="$("$BIN_DIR/amon" --version 2>/dev/null | cut -d' ' -f2)" || true
fi

TARBALL="amon-$TAG-x86_64-linux.tar.gz"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "downloading amon $TAG"
curl -fsSL "$REPO_URL/releases/download/$TAG/$TARBALL" -o "$TMP/$TARBALL"
curl -fsSL "$REPO_URL/releases/download/$TAG/$TARBALL.sha256" -o "$TMP/$TARBALL.sha256"
(cd "$TMP" && sha256sum -c --quiet "$TARBALL.sha256" >/dev/null 2>&1) || {
  echo "checksum mismatch: refusing to install" >&2
  exit 1
}
tar -xzf "$TMP/$TARBALL" -C "$TMP"

# To a temporary name first, then rename: installing over a running amon
# would otherwise fail with ETXTBSY while an agent is wrapped.
mkdir -p "$BIN_DIR" "$SHARE_DIR"
install -m 755 "$TMP/amon" "$BIN_DIR/.amon.new"
mv -f "$BIN_DIR/.amon.new" "$BIN_DIR/amon"
install -m 644 "$TMP/LICENSE" "$TMP/NOTICE" "$SHARE_DIR/"

if [ "$UPGRADING" = 1 ]; then
  case "$PREVIOUS" in
    "${TAG#v}") echo "reinstalled amon ${TAG#v}" ;;
    "") echo "upgraded amon to ${TAG#v}" ;;
    *) echo "upgraded amon $PREVIOUS -> ${TAG#v}" ;;
  esac
else
  echo "installed $BIN_DIR/amon"
fi
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "note: $BIN_DIR is not on PATH in this shell" ;;
esac

if [ "$UPGRADING" = 1 ]; then
  # Refreshes only what a previous setup installed; the binary is in place
  # either way, so a hiccup here is a note, not a failed install.
  "$BIN_DIR/amon" setup --upgrade \
    || echo "amon setup --upgrade did not finish cleanly - run it again" >&2
elif (exec </dev/tty) 2>/dev/null; then
  # A first install flows straight into the setup screen. Stdin is the curl
  # pipe, so the terminal has to be borrowed back from /dev/tty.
  "$BIN_DIR/amon" setup </dev/tty >/dev/tty \
    || echo "setup did not finish cleanly - run amon setup again" >&2
else
  echo "next: amon setup"
fi
