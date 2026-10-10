#!/usr/bin/env bash
# Regenerate every app icon from assets/app-icon/photostore-1024.png (the canonical master).
#
# Needs only installed resvg and Python 3; iconutil is optional for .icns on macOS. The outputs
# are committed, so packaging never needs these tools.
#
#   packaging/icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/assets/app-icon"
SOURCE="$DIR/photostore-1024.png"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

command -v resvg >/dev/null || { echo "error: resvg not found on PATH" >&2; exit 1; }
command -v python3 >/dev/null || { echo "error: python3 not found on PATH" >&2; exit 1; }

# The PNG is authoritative. Rebuild both self-contained SVG wrappers before rendering so a
# changed source PNG can never be overwritten by an older embedded SVG.
python3 - "$SOURCE" "$DIR/photostore.svg" "$DIR/photostore-small.svg" <<'PY'
import base64, pathlib, sys
png = base64.b64encode(pathlib.Path(sys.argv[1]).read_bytes()).decode("ascii")
svg = f'<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024"><image href="data:image/png;base64,{png}" width="1024" height="1024"/></svg>\n'
for name in sys.argv[2:]:
    pathlib.Path(name).write_text(svg, encoding="utf-8")
PY

render() { resvg -w "$2" -h "$2" "$1" "$3" </dev/null; }

# Linux hicolor theme.
for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "$DIR/hicolor/${s}x${s}/apps"
  render "$DIR/photostore.svg" "$s" "$DIR/hicolor/${s}x${s}/apps/ai.photostore.app.png"
done
mkdir -p "$DIR/hicolor/scalable/apps"
cp "$DIR/photostore-small.svg" "$DIR/hicolor/scalable/apps/ai.photostore.app.svg"

# Windows .ico.
ICO_PNGS=()
for s in 16 20 24 32 40 48 64 128 256; do
  render "$DIR/photostore.svg" "$s" "$TMP/ico-$s.png"
  ICO_PNGS+=("$TMP/ico-$s.png")
done
python3 - "$DIR/photostore.ico" "${ICO_PNGS[@]}" <<'PY'
import pathlib, struct, sys
out, paths = sys.argv[1], sys.argv[2:]
data = [pathlib.Path(p).read_bytes() for p in paths]
header = struct.pack('<HHH', 0, 1, len(data)); entries = []; offset = 6 + 16 * len(data)
for png in data:
    if len(png) < 24 or png[:8] != b'\x89PNG\r\n\x1a\n' or png[12:16] != b'IHDR':
        raise SystemExit('error: ICO input is not a non-empty PNG with an IHDR header')
    width, height = struct.unpack('>II', png[16:24])
    if not (1 <= width <= 256 and width == height):
        raise SystemExit(f'error: ICO input must be a square PNG from 1 to 256 px (got {width}x{height})')
    size = width
    w = 0 if size == 256 else size
    entries.append(struct.pack('<BBBBHHII', w, w, 0, 0, 1, 32, len(png), offset)); offset += len(png)
pathlib.Path(out).write_bytes(header + b''.join(entries) + b''.join(data))
PY

# macOS .icns.
if command -v iconutil >/dev/null; then
  SET="$TMP/photostore.iconset"
  mkdir -p "$SET"
  for s in 16 32 128 256 512; do
    render "$DIR/photostore.svg" "$s" "$SET/icon_${s}x${s}.png"
    render "$DIR/photostore.svg" $((s * 2)) "$SET/icon_${s}x${s}@2x.png"
  done
  iconutil -c icns -o "$DIR/photostore.icns" "$SET"
else
  echo "warning: iconutil not found (macOS only); photostore.icns not regenerated" >&2
fi
echo "icons written to $DIR"
