#!/bin/bash
set -euo pipefail

# Install an isolated, unsigned local development bundle. This script never touches the
# production PhotoStore.app and never launches an app unless --launch-background is explicit.
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SHARED_TARGET="${CARGO_TARGET_DIR:-/Users/computer/Desktop/dev/PhotoStore/target}"
TARGET="${SHARED_TARGET}/debug/photostore"
DEST="/Applications/PhotoStore-dev.app"
APP_NAME="PhotoStore-dev"
APP_ID="ai.photostore.app.dev"
GOAL=""
SKIP_BUILD=0
LAUNCH=0
SHORTCUT_FILE=""

usage() {
  echo "usage: $0 [--goal SLUG] [--skip-build] [--binary PATH] [--shortcut-file PATH] [--launch-background]" >&2
}
while [ "$#" -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1 ;;
    --goal) shift; [ "$#" -gt 0 ] || { usage; exit 2; }; GOAL="$1" ;;
    --binary) shift; [ "$#" -gt 0 ] || { usage; exit 2; }; TARGET="$1" ;;
    --shortcut-file) shift; [ "$#" -gt 0 ] || { usage; exit 2; }; SHORTCUT_FILE="$1" ;;
    --launch-background) LAUNCH=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "error: unknown option: $1" >&2; usage; exit 2 ;;
  esac
  shift
done

if [ -n "$GOAL" ]; then
  case "$GOAL" in *[!a-z0-9-]*) echo "error: --goal must match [a-z0-9-]" >&2; exit 2 ;; esac
  [ "${#GOAL}" -le 64 ] || { echo "error: --goal is limited to 64 characters" >&2; exit 2; }
  APP_NAME="PhotoStore-dev-$GOAL"
  APP_ID="ai.photostore.app.dev.$GOAL"
  DEST="/Applications/$APP_NAME.app"
fi

if [ "$SKIP_BUILD" -eq 0 ]; then
  (cd "$ROOT" && CARGO_TARGET_DIR="$SHARED_TARGET" CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=1 nice -n 10 cargo build --locked --offline -p photostore --bin photostore --features dev-install)
fi
[ -x "$TARGET" ] || { echo "error: dev binary not found or not executable: $TARGET" >&2; exit 1; }
if ! "$TARGET" --version 2>/dev/null | grep -Fq 'PhotoStore-dev build'; then
  echo "error: refusing to install a binary without the dev-install build marker" >&2
  exit 1
fi

if pgrep -f "/Applications/${APP_NAME}\.app/Contents/MacOS/${APP_NAME}" >/dev/null 2>&1; then
  echo "error: ${APP_NAME} is running; close it before updating this dev bundle (production is untouched)" >&2
  exit 1
fi

WORK="$(mktemp -d /tmp/photostore-dev-install.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
APP="$WORK/$APP_NAME.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/Licenses"
cp "$TARGET" "$APP/Contents/MacOS/$APP_NAME"
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
/usr/libexec/PlistBuddy -c 'Delete :CFBundleDocumentTypes' "$APP/Contents/Info.plist" >/dev/null 2>&1 || true
/usr/libexec/PlistBuddy -c 'Delete :UTExportedTypeDeclarations' "$APP/Contents/Info.plist" >/dev/null 2>&1 || true
/usr/libexec/PlistBuddy -c 'Delete :UTImportedTypeDeclarations' "$APP/Contents/Info.plist" >/dev/null 2>&1 || true
/usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName $APP_NAME" -c "Set :CFBundleName $APP_NAME" \
  -c "Set :CFBundleExecutable $APP_NAME" -c "Set :CFBundleIdentifier $APP_ID" \
  -c 'Set :CFBundleIconFile PhotoStore' "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Add :PhotoStoreBuildVariant string $APP_NAME" -c "Add :PhotoStoreWorkingTreeHash string $DIFF_SHA" "$APP/Contents/Info.plist"
if [ -n "$GOAL" ]; then
  /usr/libexec/PlistBuddy -c 'Add :LSEnvironment dict' -c "Add :LSEnvironment:PHOTOSTORE_DEV_GOAL string $GOAL" \
    -c "Add :LSEnvironment:PHOTOSTORE_CONFIG_DIR string ${HOME}/Library/Application Support/$APP_NAME" "$APP/Contents/Info.plist"
fi
/usr/bin/plutil -convert xml1 -o - "$APP/Contents/Info.plist" >/dev/null
/usr/bin/plutil -lint "$APP/Contents/Info.plist" >/dev/null
printf 'APPL????' >"$APP/Contents/PkgInfo"
codesign --force --deep --sign - "$APP" >/dev/null
codesign --verify --deep --strict "$APP"

# Seed only the canonical dev preferences location, once. Production preferences are read-only
# input; recovery, artwork, and UI layout are deliberately excluded.
DEV_PREF_DIR="${HOME}/Library/Application Support/$APP_NAME"
DEV_PREF="$DEV_PREF_DIR/preferences.json"
PROD_PREF="${HOME}/Library/Application Support/PhotoStore/preferences.json"
if [ ! -e "$DEV_PREF" ] && [ -r "$PROD_PREF" ]; then
  mkdir -p "$DEV_PREF_DIR"
  SEED="$WORK/preferences.json"
  /usr/bin/python3 - "$PROD_PREF" "$SEED" "$SHORTCUT_FILE" <<'PY'
import json, sys
source, target, shortcut = sys.argv[1:]
with open(source, encoding="utf-8") as handle:
    preferences = json.load(handle)
if not isinstance(preferences, dict):
    raise SystemExit("production preferences must be a JSON object")
preferences["shortcutFile"] = shortcut
preferences["disableRecording"] = True
with open(target, "w", encoding="utf-8") as handle:
    json.dump(preferences, handle, ensure_ascii=False, indent=2)
    handle.write("\n")
PY
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
