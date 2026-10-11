#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"; LABEL="ai.photostore.local-ci"; RUNTIME="$HOME/Library/Application Support/PhotoStore/local-ci/runtime"; PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
[ -n "${CRAFT_FONTS_DIR:-}" ] && [ -d "$CRAFT_FONTS_DIR/fonts" ] || { echo 'error: set CRAFT_FONTS_DIR to the reviewed craft-fonts directory before enabling' >&2; exit 1; }
mkdir -p "$RUNTIME" "$(dirname "$PLIST")"
cp "$ROOT/packaging/local-ci/photostore-local-ci.py" "$RUNTIME/.photostore-local-ci.py.$$"; chmod 755 "$RUNTIME/.photostore-local-ci.py.$$"; mv -f "$RUNTIME/.photostore-local-ci.py.$$" "$RUNTIME/photostore-local-ci.py"
cp "$ROOT/packaging/local-ci/photostore-local-ci.sh" "$RUNTIME/.photostore-local-ci.sh.$$"; chmod 755 "$RUNTIME/.photostore-local-ci.sh.$$"; mv -f "$RUNTIME/.photostore-local-ci.sh.$$" "$RUNTIME/photostore-local-ci.sh"
"/usr/bin/python3" "$RUNTIME/photostore-local-ci.py" --bootstrap
"/usr/bin/python3" - "$PLIST" "$RUNTIME/photostore-local-ci.py" "$HOME" "$CRAFT_FONTS_DIR" <<'PY'
import plistlib, sys
path, script, home, fonts = sys.argv[1:]
plist = {
  "Label": "ai.photostore.local-ci",
  "ProgramArguments": ["/usr/bin/python3", script],
  "EnvironmentVariables": {"HOME": home, "PATH": home + "/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin", "CRAFT_FONTS_DIR": fonts, "CRAFT_FONTS_REQUIRED": "1"},
  "RunAtLoad": True, "StartInterval": 60, "ProcessType": "Background", "LowPriorityIO": True, "Nice": 10,
  "StandardOutPath": home + "/Library/Application Support/PhotoStore/local-ci/launchd.stdout.log",
  "StandardErrorPath": home + "/Library/Application Support/PhotoStore/local-ci/launchd.stderr.log",
}
with open(path, "wb") as f: plistlib.dump(plist, f, fmt=plistlib.FMT_XML, sort_keys=False)
PY
if launchctl print "gui/$(id -u)/$LABEL" >/dev/null 2>&1; then
  echo "already enabled $PLIST (existing registration preserved)"
  exit 0
fi
launchctl bootstrap "gui/$(id -u)" "$PLIST"
echo "enabled $PLIST (baseline recorded; no build performed)"
