#!/usr/bin/env bash
# Build, sign and (optionally) notarize the macOS release artifacts:
#
#   $DIST/photostore-<version>-macos-<arch>.dmg          PhotoStore.app on a drag-to-Applications DMG
#   $DIST/photostore-cli-<version>-macos-<arch>.zip      the headless CLI
#
# Usage: packaging/macos/package.sh [--arch universal|aarch64|x86_64] [--skip-build]
#
# Signing:
#   Production defaults to the persistent local identity in
#   ~/Library/Application Support/PhotoStore/signing/release-signing.json.
#   MACOS_SIGN_IDENTITY / MACOS_KEYCHAIN may override it with a valid certificate-backed identity.
#   Ad-hoc signing is available only through package-dev.sh.
# Notarization (env; all three needed, and a real identity):
#   APPLE_ID, APPLE_PASSWORD (app-specific password), APPLE_TEAM_ID
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/macos"

ARCH=universal
SKIP_BUILD=0
while [ $# -gt 0 ]; do
  case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    -h | --help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$ARCH" in
  universal) TARGETS=(aarch64-apple-darwin x86_64-apple-darwin) ;;
  aarch64) TARGETS=(aarch64-apple-darwin) ;;
  x86_64) TARGETS=(x86_64-apple-darwin) ;;
  *) echo "unknown --arch $ARCH" >&2; exit 2 ;;
esac

# Keep in sync with LSMinimumSystemVersion in Info.plist.in.
export MACOSX_DEPLOYMENT_TARGET=11.0

# Resolve and validate signing before any Cargo work. The resolver checks the persisted public
# certificate fingerprints and that the certificate has a usable code-signing identity; it never
# creates or imports keys. NUL-delimited fields preserve keychain paths containing spaces.
exec 3< <(python3 "$HERE/resolve-signing.py")
if ! IFS= read -r -d '' IDENTITY <&3 || ! IFS= read -r -d '' MACOS_KEYCHAIN <&3 || ! IFS= read -r -d '' SIGNING_KIND <&3; then
  exec 3<&-
  echo "error: could not resolve a certificate-backed production signing identity" >&2
  exit 2
fi
exec 3<&-
SHORT_VERSION="${VERSION%%-*}"
WORK="$CARGO_TARGET_DIR/macos-package"
APP="$WORK/PhotoStore.app"
DMG="$DIST/photostore-$VERSION-macos-$ARCH.dmg"
CLI_ZIP="$DIST/photostore-cli-$VERSION-macos-$ARCH.zip"

NOTARIZE=0
if [ "$SIGNING_KIND" = "developer_id" ] && [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
  NOTARIZE=1
elif [ "$SIGNING_KIND" != "developer_id" ]; then
  warn "macOS: certificate-backed signing (not Developer ID); artifacts are not notarized"
else
  warn "macOS: APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID incomplete; signed but not notarized"
fi

echo "==> PhotoStore $VERSION for macOS ($ARCH), identity: $IDENTITY, notarize: $NOTARIZE"

# ---- build -------------------------------------------------------------------------------------
if [ "$SKIP_BUILD" = 0 ]; then
  args=()
  for t in "${TARGETS[@]}"; do args+=(--target "$t"); done
  (cd "$ROOT" && cargo build --release --locked -p photostore -p photostore-cli --features heif "${args[@]}")
fi

rm -rf "$WORK"
mkdir -p "$WORK/bin"
for bin in photostore photostore-cli; do
  inputs=()
  for t in "${TARGETS[@]}"; do inputs+=("$CARGO_TARGET_DIR/$t/release/$bin"); done
  lipo -create -output "$WORK/bin/$bin" "${inputs[@]}"
  lipo -info "$WORK/bin/$bin"
done

# ---- signing helpers ---------------------------------------------------------------------------
sign() {
  # Hardened runtime is used for both identities. Only Developer ID gets Apple's secure timestamp.
  local ts=(--timestamp)
  [ "$SIGNING_KIND" != "developer_id" ] && ts=(--timestamp=none)
  local kc=()
  [ -n "${MACOS_KEYCHAIN:-}" ] && kc=(--keychain "$MACOS_KEYCHAIN")
  codesign --force --sign "$IDENTITY" ${kc[@]+"${kc[@]}"} "${ts[@]}" "$@"
}

notarize() {
  local file="$1" out id status
  echo "==> notarizing $(basename "$file") (this can take a few minutes)"
  out="$(xcrun notarytool submit "$file" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" --wait --timeout 1h --output-format json)" || true
  echo "$out"
  id="$(printf '%s' "$out" | plutil -extract id raw -o - - 2>/dev/null || true)"
  status="$(printf '%s' "$out" | plutil -extract status raw -o - - 2>/dev/null || true)"
  if [ "$status" != "Accepted" ]; then
    if [ -n "$id" ]; then
      xcrun notarytool log "$id" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" || true
    fi
    echo "error: notarization of $(basename "$file") failed (status: ${status:-unknown})" >&2
    exit 1
  fi
}

# ---- PhotoStore.app ----------------------------------------------------------------------------
echo "==> assembling $APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
# Executable and icon carry the display name (CFBundleExecutable / CFBundleIconFile).
cp "$WORK/bin/photostore" "$APP/Contents/MacOS/PhotoStore"
cp "$ROOT/assets/app-icon/photostore.icns" "$APP/Contents/Resources/PhotoStore.icns"
# Project and third-party licence records, including embedded craft-fonts licences when present.
mkdir -p "$APP/Contents/Resources/Licenses"
copy_licenses "$APP/Contents/Resources/Licenses"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/$SHORT_VERSION/g" \
  -e "s/@BUILD_SHA@/${PHOTOSTORE_BUILD_SHA:-unknown}/g" \
  "$HERE/Info.plist.in" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' >"$APP/Contents/PkgInfo"

# Sign inside-out: nested code first, then the bundle itself (no --deep on the final signature).
# Today the only nested code is the main executable; frameworks/helpers would be signed here too.
sign --options runtime --entitlements "$HERE/entitlements.plist" "$APP/Contents/MacOS/PhotoStore"
sign --options runtime --entitlements "$HERE/entitlements.plist" "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"

if [ "$NOTARIZE" = 1 ]; then
  ditto -c -k --keepParent "$APP" "$WORK/PhotoStore-notarize.zip"
  notarize "$WORK/PhotoStore-notarize.zip"
  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"
  spctl --assess --type execute -vvv "$APP"
fi

# ---- DMG ---------------------------------------------------------------------------------------
echo "==> building $DMG"
STAGE="$WORK/dmg"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/PhotoStore.app"
ln -s /Applications "$STAGE/Applications"
# Plain DMG layout: PhotoStore.app and an Applications symlink.
rm -f "$DMG" "$WORK/raw.dmg"
# APFS avoids synthesized empty FinderInfo attributes on the mounted HFS hybrid,
# which can make a clean bundle fail strict code-signature validation on current macOS.
# APFS is available on every supported macOS version (minimum 11.0).
hdiutil create -srcfolder "$STAGE" -fs APFS -format UDZO -imagekey zlib-level=9 \
  -volname "PhotoStore" -o "$DMG"
sign "$DMG"
codesign --verify --strict --verbose=2 "$DMG"
if [ "$NOTARIZE" = 1 ]; then
  notarize "$DMG"
  xcrun stapler staple "$DMG"
  xcrun stapler validate "$DMG"
  spctl --assess --type open --context context:primary-signature -vvv "$DMG"
fi

# ---- CLI ---------------------------------------------------------------------------------------
echo "==> building $CLI_ZIP"
CLI_DIR="$WORK/photostore-cli-$VERSION-macos-$ARCH"
mkdir -p "$CLI_DIR"
cp "$WORK/bin/photostore-cli" "$CLI_DIR/"
copy_docs "$CLI_DIR"
# Same Developer ID and hardened runtime as the app, with an explicit reverse-DNS identifier
# (codesign would otherwise use the bare file name).
sign --options runtime --identifier ai.photostore.app-cli "$CLI_DIR/photostore-cli"
codesign --verify --strict --verbose=2 "$CLI_DIR/photostore-cli"
rm -f "$CLI_ZIP"
ditto -c -k --keepParent "$CLI_DIR" "$CLI_ZIP"
# notarytool accepts a zip of the bare binary. A bare Mach-O can't carry a stapled ticket, so
# Gatekeeper looks the notarization up online when a quarantined copy first runs.
if [ "$NOTARIZE" = 1 ]; then notarize "$CLI_ZIP"; fi

"$WORK/bin/photostore-cli" --version
echo "==> done (check the artifacts as shipped with packaging/macos/verify.sh --arch $ARCH)"
ls -lh "$DMG" "$CLI_ZIP"
