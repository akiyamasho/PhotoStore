#!/usr/bin/env python3
"""Incremental PhotoStore development runner.

This intentionally replaces in-process Rust HMR with a small, owned process loop:
source changes build one frozen binary, then the child is asked to close only when
it has no open documents or jobs.  A failed build leaves the previous child alive.
"""
from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
TARGET = Path(os.environ.get("CARGO_TARGET_DIR", "/Users/computer/Desktop/dev/PhotoStore/target"))
STATE = Path.home() / "Library/Application Support/PhotoStore/dev-watch"
RUNTIME = STATE / "binaries"
TOKEN = STATE / "control.token"
PID = STATE / "watcher.pid"
OWNER_LOCK = STATE / "watcher.lock"
LEASE = Path.home() / "Library/Application Support/PhotoStore/local-ci/dev-watch.lease"
BUILD_LOCK = Path.home() / "Library/Application Support/PhotoStore/local-ci/poll.lock"
EXCLUDE = {".git", "target", "node_modules", "docs", ".codex", "dev-runtime"}
DEBOUNCE = 0.5


def die(message: str) -> None:
    raise SystemExit(f"photostore-dev: {message}")


def git(*args: str) -> str:
    return subprocess.run(["git", "-C", str(ROOT), *args], check=True, text=True, capture_output=True).stdout.strip()


def tree_fingerprint() -> str:
    h = hashlib.sha256()
    for path in sorted(ROOT.rglob("*")):
        if not path.is_file() or any(part in EXCLUDE for part in path.relative_to(ROOT).parts):
            continue
        try:
            st = path.stat()
        except OSError:
            continue
        h.update(str(path.relative_to(ROOT)).encode())
        h.update(f"{st.st_mtime_ns}:{st.st_size}".encode())
    return h.hexdigest()


def ensure_state() -> None:
    STATE.mkdir(parents=True, exist_ok=True)
    RUNTIME.mkdir(parents=True, exist_ok=True)
    if not TOKEN.exists():
        TOKEN.write_text(secrets.token_hex(32) + "\n", encoding="ascii")
        os.chmod(TOKEN, 0o600)


def write_lease() -> None:
    LEASE.parent.mkdir(parents=True, exist_ok=True)
    LEASE.write_text(json.dumps({"pid": os.getpid(), "state": str(STATE)}) + "\n", encoding="utf-8")
    os.chmod(LEASE, 0o600)


def clear_lease() -> None:
    LEASE.unlink(missing_ok=True)


def port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def build() -> tuple[Path, str] | None:
    commit = git("rev-parse", "HEAD")
    dirty = tree_fingerprint()
    name = f"{commit}-{dirty[:16]}"
    frozen = RUNTIME / name / "photostore"
    if frozen.is_file():
        return frozen, name
    frozen.parent.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.update({
        "CARGO_TARGET_DIR": str(TARGET),
        "CARGO_BUILD_JOBS": "1",
        "RAYON_NUM_THREADS": "1",
        "PHOTOSTORE_BUILD_SHA": commit,
        "PHOTOSTORE_BUILD_DATE": time.strftime("%Y-%m-%d"),
    })
    BUILD_LOCK.parent.mkdir(parents=True, exist_ok=True)
    with BUILD_LOCK.open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        print(f"[build] {commit[:12]} working-tree={dirty[:12]}", flush=True)
        try:
            subprocess.run(
                ["nice", "-n", "10", "cargo", "build", "--locked", "--offline", "-p", "photostore", "--bin", "photostore", "--features", "dev-install,heif"],
                cwd=ROOT, env=env, check=True,
            )
        except subprocess.CalledProcessError as exc:
            print(f"[build] failed ({exc.returncode}); keeping the current app", file=sys.stderr, flush=True)
            return None
        source = TARGET / "debug/photostore"
        if not source.is_file():
            print("[build] cargo completed without target/debug/photostore", file=sys.stderr, flush=True)
            return None
        temp = frozen.with_name(f".{frozen.name}.{os.getpid()}.tmp")
        shutil.copy2(source, temp)
        os.chmod(temp, 0o755)
        os.replace(temp, frozen)
    return frozen, name


def request_reload(child: subprocess.Popen[bytes], listen_port: int) -> bool:
    try:
        with socket.create_connection(("127.0.0.1", listen_port), timeout=1.0) as conn:
            reader = conn.makefile("rb")
            conn.sendall((json.dumps({"id": 1, "method": "auth", "params": {"token": TOKEN.read_text().strip()}}) + "\n").encode())
            if not json.loads(reader.readline()).get("ok"):
                return False
            conn.sendall(b'{"id":2,"method":"app.devReload","params":{}}\n')
            reply = json.loads(reader.readline())
            if reply.get("ok"):
                return True
            print(f"[reload] deferred: {reply.get('error', 'unknown reason')}", flush=True)
            return False
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        if child.poll() is None:
            print(f"[reload] control unavailable: {exc}", file=sys.stderr, flush=True)
        return False


def start(binary: Path, name: str) -> tuple[subprocess.Popen[bytes], int]:
    listen_port = port()
    env = os.environ.copy()
    env.update({
        "PHOTOSTORE_CONTROL_PORT": str(listen_port),
        "PHOTOSTORE_CONTROL_TOKEN_FILE": str(TOKEN),
        "PHOTOSTORE_AUTOMATION_READ_ROOT": str(ROOT),
        "PHOTOSTORE_AUTOMATION_WRITE_ROOT": str(ROOT),
        "PHOTOSTORE_DEV_GOAL": "watch",
        "PHOTOSTORE_DEV_RUNTIME": name,
    })
    child = subprocess.Popen([str(binary)], cwd=ROOT, env=env)
    print(f"[app] started pid={child.pid} runtime={name}", flush=True)
    return child, listen_port


def status() -> None:
    print(json.dumps({"state": str(STATE), "watcher": int(PID.read_text()) if PID.exists() else None,
                      "binary_count": sum(1 for p in RUNTIME.glob("*/photostore") if p.is_file())}, indent=2))


def run(once: bool, background: bool) -> int:
    ensure_state()
    if background:
        child = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), "--once" if once else "--watch"], cwd=ROOT, start_new_session=True)
        print(f"photostore-dev watcher started pid={child.pid}")
        return 0
    with OWNER_LOCK.open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            die("another watcher already owns this dev runtime")
        PID.write_text(str(os.getpid()), encoding="ascii")
        write_lease()
        child = None
        listen_port = 0
        fingerprint = ""
        try:
            while True:
                if child is None or child.poll() is not None:
                    result = build()
                    if result is None:
                        if once:
                            return 1
                        time.sleep(2)
                        continue
                    child, listen_port = start(*result)
                current = tree_fingerprint()
                if current != fingerprint:
                    fingerprint = current
                    time.sleep(DEBOUNCE)
                    if child.poll() is None:
                        result = build()
                        if result is not None:
                            request_reload(child, listen_port)
                            if child.poll() is not None:
                                child = None
                if once:
                    return 0
                time.sleep(DEBOUNCE)
        except KeyboardInterrupt:
            print("[watch] stopped; owned app left running", flush=True)
            return 0
        finally:
            PID.unlink(missing_ok=True)
            clear_lease()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("--watch", "--once", "--background", "--status"), nargs="?", default="--watch")
    args = parser.parse_args()
    if args.mode == "--status":
        status()
        return 0
    return run(args.mode == "--once", args.mode == "--background")


if __name__ == "__main__":
    raise SystemExit(main())
