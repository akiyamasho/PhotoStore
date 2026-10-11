#!/usr/bin/env bash
# Build an isolated, local-only macOS development app.
#
#   packaging/macos/package-dev.sh [--skip-build] [--goal SLUG]
#
# The result is a signed PhotoStore-dev.app with no Launch Services document
# declarations. It is intentionally not a release artifact and never installs,
# replaces, notarizes, or publishes an app.
set -euo pipefail

# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/macos"
TARGET="aarch64-apple-darwin"
SKIP_BUILD=0
GOAL=""

while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --goal)
      [ $# -ge 2 ] || { echo "error: --goal requires a lowercase slug" >&2; exit 2; }
      GOAL="$2"
      shift 2
      ;;
    -h | --help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ -n "$GOAL" ] && [[ ! "$GOAL" =~ ^[a-z]$ ]] && [[ ! "$GOAL" =~ ^[a-z][a-z0-9-]{0,46}[a-z0-9]$ ]]; then
  echo "error: --goal must be a lowercase a-z0-9-hyphen slug, 1-48 characters, starting with a letter and ending alphanumeric" >&2
  exit 2
fi

export MACOSX_DEPLOYMENT_TARGET=11.0
export PHOTOSTORE_DEV_BUILD=1
export PHOTOSTORE_DEV_GOAL="$GOAL"
FULL_SHA="${PHOTOSTORE_BUILD_SHA:-unknown}"
SHORT_SHA="${FULL_SHA:0:9}"
SHORT_VERSION="${VERSION%%-*}"
BIN="$CARGO_TARGET_DIR/$TARGET/release/photostore"
APP_NAME="PhotoStore-dev"
APP_ID="ai.photostore.app.dev"
if [ -n "$GOAL" ]; then
  APP_NAME="$APP_NAME-$GOAL"
  APP_ID="$APP_ID.$GOAL"
fi

if [ "$FULL_SHA" = unknown ] || [ -z "$SHORT_SHA" ]; then
  echo "error: PHOTOSTORE_BUILD_SHA must identify the source commit" >&2
  exit 1
fi

if [ "$SKIP_BUILD" = 0 ]; then
  echo "==> building PhotoStore-dev $VERSION ($SHORT_SHA) for macOS arm64"
  (
    cd "$ROOT"
    CARGO_BUILD_JOBS=1 \
    RAYON_NUM_THREADS=1 \
    CARGO_NET_OFFLINE=true \
    CARGO_PROFILE_RELEASE_OPT_LEVEL=2 \
    CARGO_PROFILE_RELEASE_LTO=false \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
    CARGO_PROFILE_RELEASE_DEBUG=0 \
    PHOTOSTORE_BUILD_SHA="$FULL_SHA" \
    PHOTOSTORE_DEV_BUILD=1 \
    PHOTOSTORE_DEV_GOAL="$GOAL" \
      nice -n 10 cargo build --release --locked --offline -p photostore --features heif,dev-install --target "$TARGET"
  )
else
  echo "==> using existing arm64 binary (--skip-build)"
fi

[ -f "$BIN" ] || { echo "error: missing baked binary: $BIN" >&2; exit 1; }
VERSION_OUTPUT="$( "$BIN" --version 2>&1 )"
case "$VERSION_OUTPUT" in
  *"$VERSION"*"$SHORT_SHA"*"[dev]"*"($APP_NAME build)"*) ;;
  *)
    echo "error: binary is not the expected PhotoStore-dev build:" >&2
    echo "$VERSION_OUTPUT" >&2
    exit 1
    ;;
esac

# Assemble in a unique temporary directory. Keep it after completion so the
# exact staged path remains available for manual inspection and launch.
WORK="$(mktemp -d "${TMPDIR:-/tmp}/photostore-dev.XXXXXX")"
APP="$WORK/$APP_NAME.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/Licenses"
cp "$BIN" "$APP/Contents/MacOS/$APP_NAME"
cp "$ROOT/assets/app-icon/photostore.icns" "$APP/Contents/Resources/PhotoStore.icns"
[ -f "$ROOT/README.md" ] && cp "$ROOT/README.md" "$APP/Contents/Resources/"
copy_licenses "$APP/Contents/Resources/Licenses"

cp "$HERE/Info.plist.in" "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName $APP_NAME" \
  -c "Set :CFBundleName $APP_NAME" \
  -c "Set :CFBundleExecutable $APP_NAME" \
  -c "Set :CFBundleIdentifier $APP_ID" \
  -c "Set :CFBundleShortVersionString $SHORT_VERSION" \
  -c "Set :CFBundleVersion $SHORT_VERSION" \
  -c "Set :PhotoStoreVersion $VERSION [dev]" \
  -c "Set :PhotoStoreBuildCommit $FULL_SHA" \
  -c "Add :PhotoStoreBuildVariant string $APP_NAME" \
  -c "Add :PhotoStoreBuildGoal string $GOAL" \
  -c "Delete :CFBundleDocumentTypes" \
  -c "Delete :UTExportedTypeDeclarations" \
  -c "Delete :UTImportedTypeDeclarations" \
  "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
ACTUAL_APP_ID="$(plutil -extract CFBundleIdentifier raw -o - "$APP/Contents/Info.plist")"
[ "$ACTUAL_APP_ID" = "$APP_ID" ] || { echo "error: plist bundle identifier mismatch: $ACTUAL_APP_ID (expected $APP_ID)" >&2; exit 1; }
printf 'APPL????' >"$APP/Contents/PkgInfo"

codesign --force --sign - --timestamp=none "$APP/Contents/MacOS/$APP_NAME"
codesign --force --sign - --timestamp=none "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"

# Never overwrite an existing app: a running dev bundle must remain intact.
OUTPUT="${PHOTOSTORE_DEV_OUTPUT:-$DIST/dev/$APP_NAME-$SHORT_SHA.app}"
if [ -e "$OUTPUT" ]; then
  echo "error: output already exists; choose PHOTOSTORE_DEV_OUTPUT or remove it manually: $OUTPUT" >&2
  exit 1
fi
mkdir -p "$(dirname "$OUTPUT")"
ditto "$APP" "$OUTPUT"

echo "==> $APP_NAME staged at $OUTPUT"
echo "==> retained assembly directory: $WORK"
echo "==> launch without foreground focus: open -g -j -n -a '$OUTPUT'"
