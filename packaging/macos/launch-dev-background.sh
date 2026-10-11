#!/usr/bin/env bash
# Launch one PhotoStore development bundle without taking over the user's UI.
#
# Usage:
#   launch-dev-background.sh /absolute/path/PhotoStore-dev[-goal].app [--args ...]
set -euo pipefail

if [ "$#" -lt 1 ]; then
  echo "usage: $0 /absolute/path/PhotoStore-dev[-goal].app [--args ...]" >&2
  exit 2
fi

APP_INPUT="$1"
shift
case "$APP_INPUT" in
  /*) ;;
  *) echo "error: app path must be absolute" >&2; exit 2 ;;
esac
case "$APP_INPUT" in
  *.app) ;;
  *) echo "error: expected a .app bundle: $APP_INPUT" >&2; exit 2 ;;
esac
[ -d "$APP_INPUT" ] || { echo "error: app bundle not found: $APP_INPUT" >&2; exit 1; }
APP="$(cd "$APP_INPUT" && pwd -P)"
PLIST="$APP/Contents/Info.plist"
[ -f "$PLIST" ] || { echo "error: missing Info.plist: $PLIST" >&2; exit 1; }

BUNDLE_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$PLIST" 2>/dev/null)" ||
  { echo "error: could not read CFBundleIdentifier" >&2; exit 1; }
BUNDLE_EXEC="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$PLIST" 2>/dev/null)" ||
  { echo "error: could not read CFBundleExecutable" >&2; exit 1; }
case "$BUNDLE_ID" in
  ai.photostore.app.dev|ai.photostore.app.dev.*) ;;
  *) echo "error: refusing non-development bundle identifier: $BUNDLE_ID" >&2; exit 1 ;;
esac
case "$BUNDLE_EXEC" in
  PhotoStore-dev|PhotoStore-dev-*) ;;
  *) echo "error: refusing non-development executable: $BUNDLE_EXEC" >&2; exit 1 ;;
esac
EXEC="$APP/Contents/MacOS/$BUNDLE_EXEC"
[ -x "$EXEC" ] || { echo "error: executable not found or not executable: $EXEC" >&2; exit 1; }

running_rows() {
  /usr/bin/osascript -l JavaScript - "$BUNDLE_ID" "$EXEC" <<'JXA'
ObjC.import('AppKit');
function run(argv) {
  const bundleID = argv[0];
  const executable = argv[1];
  const apps = $.NSWorkspace.sharedWorkspace.runningApplications;
  const frontmost = $.NSWorkspace.sharedWorkspace.frontmostApplication;
  const frontmostPID = frontmost ? Number(frontmost.processIdentifier) : -1;
  const rows = [];
  for (let i = 0; i < apps.count; i++) {
    const app = apps.objectAtIndex(i);
    const url = app.executableURL;
    const path = url ? url.path.js : '';
    const id = app.bundleIdentifier ? app.bundleIdentifier.js : '';
    const pid = Number(app.processIdentifier);
    if (path === executable && id === bundleID) {
      rows.push([pid, path, id, app.isActive ? 'true' : 'false',
        app.isHidden ? 'true' : 'false', pid === frontmostPID ? 'true' : 'false',
        app.isFinishedLaunching ? 'true' : 'false'].join('\t'));
    }
  }
  return rows.join('\n');
}
JXA
}

ROW="$(running_rows)"
ROW_COUNT="$(printf '%s\n' "$ROW" | awk 'NF { n += 1 } END { print n + 0 }')"
if [ "$ROW_COUNT" -gt 0 ]; then
  echo "error: matching development process is already running; refusing to touch it" >&2
  exit 1
fi

OPEN_ARGS=("$APP")
if [ "$#" -gt 0 ]; then
  [ "$1" = "--args" ] || { echo "error: forwarded arguments must follow --args" >&2; exit 2; }
  shift
  OPEN_ARGS+=(--args "$@")
fi
/usr/bin/open -g -j -n -a "${OPEN_ARGS[@]}"

PID=""
DEADLINE=$((SECONDS + 30))
while [ "$SECONDS" -lt "$DEADLINE" ]; do
  ROW="$(running_rows)"
  if [ -n "$ROW" ]; then
    ROW_COUNT="$(printf '%s\n' "$ROW" | awk 'NF { n += 1 } END { print n + 0 }')"
    if [ "$ROW_COUNT" -ne 1 ]; then
      echo "error: found $ROW_COUNT matching development processes after launch" >&2
      exit 1
    fi
    IFS=$'\t' read -r ROW_PID _ _ _ _ _ FINISHED_LAUNCHING <<< "$ROW"
    if [ "$FINISHED_LAUNCHING" = "true" ]; then
      PID="$ROW_PID"
      break
    fi
  fi
  sleep 0.2
done
[ -n "$PID" ] || { echo "error: development app did not start within 30 seconds" >&2; exit 1; }

# Hide only the newly launched exact process. This uses AppKit directly and
# never activates, closes, or restores any other application.
/usr/bin/osascript -l JavaScript - "$PID" "$EXEC" "$BUNDLE_ID" <<'JXA'
ObjC.import('AppKit');
function run(argv) {
  const pid = Number(argv[0]);
  const executable = argv[1];
  const bundleID = argv[2];
  const app = $.NSRunningApplication.runningApplicationWithProcessIdentifier(pid);
  if (!app || !app.executableURL || app.executableURL.path.js !== executable ||
      !app.bundleIdentifier || app.bundleIdentifier.js !== bundleID) {
    throw new Error('new process identity changed before hide');
  }
  const frontmost = $.NSWorkspace.sharedWorkspace.frontmostApplication;
  if (frontmost && Number(frontmost.processIdentifier) === pid) {
    app.performSelector($.NSSelectorFromString('hide'));
    throw new Error('development app became frontmost before hide');
  }
  app.performSelector($.NSSelectorFromString('hide'));
  return 'hidden';
}
JXA

CHECK_DEADLINE=$((SECONDS + 3))
STABLE_CHECKS=0
while [ "$SECONDS" -lt "$CHECK_DEADLINE" ]; do
  ROW="$(running_rows)"
  if [ -n "$ROW" ]; then
    ROW_PID="$(printf '%s' "$ROW" | awk -F '\t' '{print $1}')"
    ACTIVE="$(printf '%s' "$ROW" | awk -F '\t' '{print $4}')"
    HIDDEN="$(printf '%s' "$ROW" | awk -F '\t' '{print $5}')"
    FRONTMOST="$(printf '%s' "$ROW" | awk -F '\t' '{print $6}')"
    if [ "$ROW_PID" = "$PID" ] && [ "$FRONTMOST" = "true" ]; then
      /usr/bin/osascript -l JavaScript - "$PID" "$EXEC" "$BUNDLE_ID" <<'JXA'
ObjC.import('AppKit');
function run(argv) {
  const pid = Number(argv[0]);
  const executable = argv[1];
  const bundleID = argv[2];
  const app = $.NSRunningApplication.runningApplicationWithProcessIdentifier(pid);
  if (!app || !app.executableURL || app.executableURL.path.js !== executable ||
      !app.bundleIdentifier || app.bundleIdentifier.js !== bundleID) {
    throw new Error('new process identity changed after hide');
  }
  app.performSelector($.NSSelectorFromString('hide'));
  return 'hidden';
}
JXA
      echo "error: development app became frontmost after hide" >&2
      exit 1
    fi
    if [ "$ROW_PID" = "$PID" ] && [ "$ACTIVE" = "false" ] && [ "$HIDDEN" = "true" ] && [ "$FRONTMOST" = "false" ]; then
      STABLE_CHECKS=$((STABLE_CHECKS + 1))
      if [ "$STABLE_CHECKS" -ge 3 ]; then
        echo "launched hidden development app (pid=$PID, bundle=$BUNDLE_ID)"
        exit 0
      fi
    else
      STABLE_CHECKS=0
    fi
  else
    STABLE_CHECKS=0
  fi
  sleep 0.2
done
echo "error: development app did not remain hidden and inactive" >&2
exit 1
