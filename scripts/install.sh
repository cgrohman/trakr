#!/usr/bin/env sh
# Installs the latest trakr-ui release for Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/cgrohman/trakr/main/scripts/install.sh | sh
#
# Downloads the AppImage from the latest GitHub release, makes it
# executable, and drops it in ~/.local/bin/trakr. Windows users: grab the
# .msi installer directly from https://github.com/cgrohman/trakr/releases/latest
# -- this script only handles the curl-friendly Linux case.
set -eu

REPO="cgrohman/trakr"
INSTALL_DIR="${TRAKR_INSTALL_DIR:-$HOME/.local/bin}"
BIN_NAME="trakr"

case "$(uname -s)" in
  Linux) ;;
  *)
    echo "error: this script only supports Linux." >&2
    echo "Grab the right installer for your OS from:" >&2
    echo "  https://github.com/$REPO/releases/latest" >&2
    exit 1
    ;;
esac

command -v curl >/dev/null 2>&1 || { echo "error: curl is required" >&2; exit 1; }

echo "Looking up the latest trakr release..."
RELEASE_JSON="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest")"

ASSET_URL="$(printf '%s' "$RELEASE_JSON" | grep -o '"browser_download_url": *"[^"]*\.AppImage"' | head -n1 | sed -E 's/.*"(https[^"]+)"/\1/')"
TAG="$(printf '%s' "$RELEASE_JSON" | grep -o '"tag_name": *"[^"]*"' | head -n1 | sed -E 's/.*"([^"]+)"$/\1/')"

if [ -z "$ASSET_URL" ]; then
  echo "error: could not find an AppImage asset on the latest release." >&2
  echo "Check https://github.com/$REPO/releases/latest manually." >&2
  exit 1
fi

echo "Installing trakr $TAG to $INSTALL_DIR/$BIN_NAME"
mkdir -p "$INSTALL_DIR"
TMP_FILE="$(mktemp)"
curl -fsSL "$ASSET_URL" -o "$TMP_FILE"
chmod +x "$TMP_FILE"
mv "$TMP_FILE" "$INSTALL_DIR/$BIN_NAME"

echo "Installed trakr $TAG."
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "Note: $INSTALL_DIR is not on your PATH. Add it, e.g.:" ; echo "  export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
echo "Run '$BIN_NAME' to launch. Future updates are handled in-app via the Update button."
