#!/usr/bin/env bash
set -euo pipefail
PYTHON="${PHOTOSTORE_LOCAL_CI_PYTHON:-/usr/bin/python3}"
exec "$PYTHON" "$(cd "$(dirname "$0")" && pwd)/photostore-local-ci.py" "$@"
