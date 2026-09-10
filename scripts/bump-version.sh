#!/usr/bin/env sh
# Bumps the trakr-ui desktop app version in all three places tauri-action
# reads it from, so they can't drift out of sync with each other.
#
#   scripts/bump-version.sh 0.2.0
#
# Does not commit, tag, or push -- see the "Releasing a new desktop app
# version" section in README.md for the rest of the release checklist.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CARGO_TOML="$ROOT/apps/trakr-ui/src-tauri/Cargo.toml"
TAURI_CONF="$ROOT/apps/trakr-ui/src-tauri/tauri.conf.json"
PACKAGE_JSON="$ROOT/apps/trakr-ui/package.json"

if [ "$#" -ne 1 ]; then
  echo "usage: $0 <new-version>" >&2
  echo "example: $0 0.2.0" >&2
  exit 2
fi

NEW_VERSION="$1"
case "$NEW_VERSION" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *)
    echo "error: version must look like x.y.z (got '$NEW_VERSION')" >&2
    exit 2
    ;;
esac

CURRENT_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$CARGO_TOML" | head -n1)"
if [ -z "$CURRENT_VERSION" ]; then
  echo "error: could not read current version from $CARGO_TOML" >&2
  exit 1
fi

echo "Bumping trakr-ui: $CURRENT_VERSION -> $NEW_VERSION"

update_field() {
  file="$1"
  pattern="$2"
  replacement="$3"
  if ! grep -q "$pattern" "$file"; then
    echo "error: expected pattern not found in $file: $pattern" >&2
    exit 1
  fi
  sed -i.bak "s/$pattern/$replacement/" "$file"
  rm -f "$file.bak"
}

update_field "$CARGO_TOML" \
  "^version = \"$CURRENT_VERSION\"" \
  "version = \"$NEW_VERSION\""

update_field "$TAURI_CONF" \
  "\"version\": \"$CURRENT_VERSION\"" \
  "\"version\": \"$NEW_VERSION\""

update_field "$PACKAGE_JSON" \
  "\"version\": \"$CURRENT_VERSION\"" \
  "\"version\": \"$NEW_VERSION\""

echo "Refreshing Cargo.lock..."
(cd "$ROOT" && cargo build -p trakr-ui >/dev/null 2>&1) || {
  echo "warning: 'cargo build -p trakr-ui' failed -- Cargo.lock may be stale, check manually" >&2
}

echo "Done. Review the diff, then:"
echo "  git add -A && git commit -m \"Bump trakr-ui to v$NEW_VERSION\""
echo "  git push (through a feature branch + PR, then on main:)"
echo "  git tag v$NEW_VERSION && git push origin v$NEW_VERSION"
