#!/bin/bash
set -euo pipefail

# Install an isolated, unsigned local development bundle. This script never touches the
# production PhotoStore.app and never launches an app unless --launch-background is explicit.
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TARGET="${ROOT}/target/debug/photostore"
DEST="/Applications/PhotoStore-dev.app"
SKIP_BUILD=0
LAUNCH=0
SHORTCUT_FILE=""

usage() {
  echo "usage: $0 [--skip-build] [--binary PATH] [--shortcut-file PATH] [--launch-background]" >&2
}
while [ "$#" -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1 ;;
    --binary) shift; [ "$#" -gt 0 ] || { usage; exit 2; }; TARGET="$1" ;;
    --shortcut-file) shift; [ "$#" -gt 0 ] || { usage; exit 2; }; SHORTCUT_FILE="$1" ;;
    --launch-background) LAUNCH=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "error: unknown option: $1" >&2; usage; exit 2 ;;
  esac
  shift
done

if [ "$SKIP_BUILD" -eq 0 ]; then
  (cd "$ROOT" && CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}" CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=1 nice -n 10 cargo build --locked --offline -p photostore --bin photostore --features dev-install)
fi
[ -x "$TARGET" ] || { echo "error: dev binary not found or not executable: $TARGET" >&2; exit 1; }
VERSION_OUTPUT="$("$TARGET" --version 2>/dev/null || true)"
if ! printf '%s' "$VERSION_OUTPUT" | grep -Fq '(PhotoStore-dev build)'; then
  echo "error: refusing to install a binary without the dev-install build marker" >&2
  exit 1
fi
# This consolidated installer owns the unqualified dev namespace. A goal RC
# must be packaged and launched under its own identity and config directory.
if printf '%s' "$VERSION_OUTPUT" | grep -Eq 'PhotoStore-dev-[a-z0-9-]+|ai\.photostore\.app\.dev\.'; then
  echo "error: refusing to install a goal-specific dev binary; use package-dev.sh --goal SLUG" >&2
  exit 1
fi

if pgrep -f '/Applications/PhotoStore-dev\.app/Contents/MacOS/PhotoStore-dev' >/dev/null 2>&1; then
  echo "error: PhotoStore-dev is running; close it before updating the dev bundle (production is untouched)" >&2
  exit 1
fi

WORK="$(mktemp -d /tmp/photostore-dev-install.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
APP="$WORK/PhotoStore-dev.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/Licenses"
cp "$TARGET" "$APP/Contents/MacOS/PhotoStore-dev"
cp "$ROOT/assets/app-icon/photostore.icns" "$APP/Contents/Resources/PhotoStore.icns"
for LICENSE in LICENSE LICENSE-MIT LICENSE-APACHE NOTICE ATTRIBUTION.md COPYRIGHT; do
  [ -f "$ROOT/$LICENSE" ] && cp "$ROOT/$LICENSE" "$APP/Contents/Resources/Licenses/$LICENSE"
done
find "$ROOT/assets" -type f \( -name 'LICENSE*' -o -name 'NOTICE*' \) -exec cp {} "$APP/Contents/Resources/Licenses/" \;
if [ -n "${CRAFT_FONTS_DIR:-}" ]; then
  for FONT_LICENSE in "$CRAFT_FONTS_DIR"/fonts/*/OFL.txt; do
    [ -f "$FONT_LICENSE" ] || continue
    FONT_FAMILY="$(basename "$(dirname "$FONT_LICENSE")")"
    cp "$FONT_LICENSE" "$APP/Contents/Resources/Licenses/OFL-$FONT_FAMILY.txt"
  done
fi

VERSION="$(sed -n 's/^version = "\([0-9][^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
SHORT_VERSION="$(printf '%s' "$VERSION" | sed -E 's/-.*//')"
SHA="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf unknown)"
DIFF_SHA="$(git -C "$ROOT" diff --no-ext-diff | shasum -a 256 | awk '{print $1}')"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/$SHORT_VERSION/g" -e "s/@BUILD_SHA@/$SHA/g" \
  "$ROOT/packaging/macos/Info.plist.in" >"$APP/Contents/Info.plist"
# Dev bundles must not claim ownership of any document type or URL scheme.
/usr/libexec/PlistBuddy -c 'Delete :CFBundleDocumentTypes' -c 'Delete :UTExportedTypeDeclarations' \
  -c 'Delete :UTImportedTypeDeclarations' "$APP/Contents/Info.plist" >/dev/null
/usr/libexec/PlistBuddy -c 'Set :CFBundleDisplayName PhotoStore-dev' -c 'Set :CFBundleName PhotoStore-dev' \
  -c 'Set :CFBundleExecutable PhotoStore-dev' -c 'Set :CFBundleIdentifier ai.photostore.app.dev' \
  -c 'Set :CFBundleIconFile PhotoStore' "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Add :PhotoStoreBuildVariant string PhotoStore-dev' -c "Add :PhotoStoreWorkingTreeHash string $DIFF_SHA" "$APP/Contents/Info.plist"
/usr/bin/plutil -convert xml1 -o - "$APP/Contents/Info.plist" >/dev/null
/usr/bin/plutil -lint "$APP/Contents/Info.plist" >/dev/null
printf 'APPL????' >"$APP/Contents/PkgInfo"
codesign --force --deep --sign - "$APP" >/dev/null
codesign --verify --deep --strict "$APP"

# Seed only the canonical dev preferences location, once. Production preferences are read-only
# input; recovery, artwork, and UI layout are deliberately excluded.
DEV_PREF_DIR="${HOME}/Library/Application Support/PhotoStore-dev"
DEV_PREF="$DEV_PREF_DIR/preferences.json"
PROD_PREF="${HOME}/Library/Application Support/PhotoStore/preferences.json"
if [ ! -e "$DEV_PREF" ] && [ -r "$PROD_PREF" ]; then
  mkdir -p "$DEV_PREF_DIR"
  SEED="$WORK/preferences.json"
  cp "$PROD_PREF" "$SEED"
  if ! /usr/bin/plutil -replace shortcutFile -string "$SHORTCUT_FILE" "$SEED" 2>/dev/null; then
    /usr/bin/plutil -insert shortcutFile -string "$SHORTCUT_FILE" "$SEED"
  fi
  if ! /usr/bin/plutil -replace disableRecording -bool true "$SEED" 2>/dev/null; then
    /usr/bin/plutil -insert disableRecording -bool true "$SEED"
  fi
  SEED_JSON="$WORK/preferences.native.json"
  /usr/bin/plutil -convert json -o "$SEED_JSON" "$SEED"
  mv "$SEED_JSON" "$SEED"
  SEED_TMP="${DEV_PREF}.tmp.$$"
  cp "$SEED" "$SEED_TMP"
  mv "$SEED_TMP" "$DEV_PREF"
fi

STAGE="${DEST}.staged.$$"
rm -rf "$STAGE"
mv "$APP" "$STAGE"
if [ -e "$DEST" ]; then
  OLD="${DEST}.previous.$$"
  mv "$DEST" "$OLD"
  mv "$STAGE" "$DEST"
  rm -rf "$OLD"
else
  mv "$STAGE" "$DEST"
fi
echo "installed $DEST (source=$SHA, working-tree=$DIFF_SHA, feature=dev-install)"
if [ "$LAUNCH" -eq 1 ]; then
  open -g -j "$DEST"
fi
