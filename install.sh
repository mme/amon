#!/bin/sh
# Installs amon: the latest release binary into ~/.local/bin, no root.
#
#   curl -fsSL amon.sh/install | sh
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

echo "installed $BIN_DIR/amon"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "note: $BIN_DIR is not on PATH in this shell" ;;
esac
echo "next: amon setup"
