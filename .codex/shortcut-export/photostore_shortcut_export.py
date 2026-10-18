#!/opt/homebrew/opt/python@3.15/bin/python3.15
import json
import os
import sys
import tempfile

SOURCE = "/Users/computer/Library/Application Support/PhotoStore/preferences.json"
DEST = "/Users/computer/Desktop/dev/tools/Animation/photostore_keys.json"

def fail(message):
    print(f"photostore-shortcut-export: {message}", file=sys.stderr)
    return 1

def main():
    try:
        if os.path.islink(DEST):
            return fail(f"refusing symlink destination: {DEST}")
        with open(SOURCE, "rb") as f:
            data = json.load(f)
        overrides = data.get("shortcuts")
        alternates = data.get("shortcutAlternates")
        if not isinstance(overrides, dict) or any(not isinstance(k, str) or not isinstance(v, str) for k, v in overrides.items()):
            return fail("preferences.shortcuts is missing or contains non-string entries")
        if not isinstance(alternates, dict) or any(not isinstance(k, str) or not isinstance(v, list) or any(not isinstance(x, str) for x in v) for k, v in alternates.items()):
            return fail("preferences.shortcutAlternates is missing or contains invalid entries")
        payload = {"format": "photostore-shortcuts", "version": 1, "overrides": dict(sorted(overrides.items())), "alternates": {k: v for k, v in sorted(alternates.items())}}
        encoded = (json.dumps(payload, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
        if os.path.exists(DEST):
            with open(DEST, "rb") as f:
                if f.read() == encoded:
                    print(f"unchanged {DEST} ({len(overrides)} overrides, {len(alternates)} alternate groups)")
                    return 0
        parent = os.path.dirname(DEST)
        os.makedirs(parent, exist_ok=True)
        fd, tmp = tempfile.mkstemp(prefix=".photostore_keys.", suffix=".tmp", dir=parent)
        try:
            with os.fdopen(fd, "wb") as f:
                f.write(encoded)
                f.flush()
                os.fsync(f.fileno())
            os.replace(tmp, DEST)
        finally:
            if os.path.exists(tmp):
                os.unlink(tmp)
        print(f"wrote {DEST} ({len(overrides)} overrides, {len(alternates)} alternate groups)")
        return 0
    except FileNotFoundError as e:
        return fail(f"missing file: {e.filename}")
    except (OSError, ValueError, json.JSONDecodeError) as e:
        return fail(str(e))

if __name__ == "__main__":
    raise SystemExit(main())
