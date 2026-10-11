#!/usr/bin/env bash
# Build an isolated, local-only macOS development app.
#
#   packaging/macos/package-dev.sh [--skip-build]
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

while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    -h | --help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

export MACOSX_DEPLOYMENT_TARGET=11.0
export PHOTOSTORE_DEV_BUILD=1
FULL_SHA="${PHOTOSTORE_BUILD_SHA:-unknown}"
SHORT_SHA="${FULL_SHA:0:9}"
SHORT_VERSION="${VERSION%%-*}"
BIN="$CARGO_TARGET_DIR/$TARGET/release/photostore"

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
      nice -n 10 cargo build --release --locked --offline -p photostore --features heif,dev-install --target "$TARGET"
  )
else
  echo "==> using existing arm64 binary (--skip-build)"
fi

[ -f "$BIN" ] || { echo "error: missing baked binary: $BIN" >&2; exit 1; }
VERSION_OUTPUT="$( "$BIN" --version 2>&1 )"
case "$VERSION_OUTPUT" in
  *"$VERSION"*"$SHORT_SHA"*" [dev]"*) ;;
  *"$VERSION"*"$SHORT_SHA"*"[dev]"*) ;;
  *)
    echo "error: binary is not the expected PhotoStore-dev build:" >&2
    echo "$VERSION_OUTPUT" >&2
    exit 1
    ;;
esac

# Assemble in a unique temporary directory. Keep it after completion so the
# exact staged path remains available for manual inspection and launch.
WORK="$(mktemp -d "${TMPDIR:-/tmp}/photostore-dev.XXXXXX")"
APP="$WORK/PhotoStore-dev.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/Licenses"
cp "$BIN" "$APP/Contents/MacOS/PhotoStore-dev"
cp "$ROOT/assets/app-icon/photostore.icns" "$APP/Contents/Resources/PhotoStore.icns"
[ -f "$ROOT/README.md" ] && cp "$ROOT/README.md" "$APP/Contents/Resources/"
copy_licenses "$APP/Contents/Resources/Licenses"

cp "$HERE/Info.plist.in" "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName PhotoStore-dev" \
  -c "Set :CFBundleName PhotoStore-dev" \
  -c "Set :CFBundleExecutable PhotoStore-dev" \
  -c "Set :CFBundleIdentifier ai.photostore.app.dev" \
  -c "Set :CFBundleShortVersionString $SHORT_VERSION" \
  -c "Set :CFBundleVersion $SHORT_VERSION" \
  -c "Set :PhotoStoreVersion $VERSION [dev]" \
  -c "Set :PhotoStoreBuildCommit $FULL_SHA" \
  -c "Add :PhotoStoreBuildVariant string PhotoStore-dev" \
  -c "Delete :CFBundleDocumentTypes" \
  -c "Delete :UTExportedTypeDeclarations" \
  -c "Delete :UTImportedTypeDeclarations" \
  "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' >"$APP/Contents/PkgInfo"

codesign --force --sign - --timestamp=none "$APP/Contents/MacOS/PhotoStore-dev"
codesign --force --sign - --timestamp=none "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"

# Never overwrite an existing app: a running dev bundle must remain intact.
OUTPUT="${PHOTOSTORE_DEV_OUTPUT:-$DIST/dev/PhotoStore-dev-$SHORT_SHA.app}"
if [ -e "$OUTPUT" ]; then
  echo "error: output already exists; choose PHOTOSTORE_DEV_OUTPUT or remove it manually: $OUTPUT" >&2
  exit 1
fi
mkdir -p "$(dirname "$OUTPUT")"
ditto "$APP" "$OUTPUT"

echo "==> PhotoStore-dev staged at $OUTPUT"
echo "==> retained assembly directory: $WORK"
echo "==> launch without foreground focus: open -g -j -n -a '$OUTPUT'"
