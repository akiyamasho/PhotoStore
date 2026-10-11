#!/usr/bin/env python3
"""Incremental PhotoStore dev runner with conservative owned-process reloads."""
from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
TARGET = Path(os.environ.get("CARGO_TARGET_DIR", "/Users/computer/Desktop/dev/PhotoStore/target"))
STATE = Path.home() / "Library/Application Support/PhotoStore/dev-watch"
RUNTIME = STATE / "binaries"
TOKEN = STATE / "control.token"
OWNER_LOCK = STATE / "watcher.lock"
PID_FILE = STATE / "watcher.pid"
LEASE = Path.home() / "Library/Application Support/PhotoStore/local-ci/dev-watch.lease"
BUILD_LOCK = Path.home() / "Library/Application Support/PhotoStore/local-ci/poll.lock"
FONTS = Path(os.environ.get("CRAFT_FONTS_DIR", "/Users/computer/Library/Application Support/PhotoStore/local-ci/craft-fonts"))
PENDING = STATE / "pending.json"
EXCLUDED = {".git", "target", "node_modules", "docs", ".codex", "dev-runtime", "screenshots"}
SUFFIXES = {".rs", ".toml", ".lock", ".json", ".ron", ".wgsl", ".glsl", ".metal", ".plist", ".sh", ".py"}


def log(message: str) -> None:
    STATE.mkdir(parents=True, exist_ok=True)
    with (STATE / "watcher.log").open("a", encoding="utf-8") as stream:
        stream.write(f"{time.strftime('%Y-%m-%dT%H:%M:%S%z')} {message}\n")
    print(message, flush=True)


def git(*args: str) -> str:
    return subprocess.run(["git", "-C", str(ROOT), *args], check=True, text=True, capture_output=True).stdout.strip()


def atomic_write(path: Path, data: bytes, mode: int = 0o600) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp, path)
    finally:
        temp.unlink(missing_ok=True)


def source_files() -> list[Path]:
    result: list[Path] = []
    for base, dirs, files in os.walk(ROOT):
        dirs[:] = [d for d in dirs if d not in EXCLUDED and not d.startswith(".")]
        for name in files:
            path = Path(base) / name
            if path.suffix in SUFFIXES and path.is_file():
                result.append(path)
    return sorted(result)


def fingerprint() -> str:
    digest = hashlib.sha256()
    for path in source_files():
        stat = path.stat()
        digest.update(str(path.relative_to(ROOT)).encode())
        digest.update(f"{stat.st_mtime_ns}:{stat.st_size}".encode())
    return digest.hexdigest()


def ensure_state() -> None:
    STATE.mkdir(parents=True, exist_ok=True)
    RUNTIME.mkdir(parents=True, exist_ok=True)
    atomic_write(TOKEN, (secrets.token_hex(32) + "\n").encode("ascii"))


def validate_fonts() -> None:
    manifest = FONTS / "fonts/manifest.txt"
    if not manifest.is_file() or not manifest.read_text(encoding="utf-8").strip():
        raise RuntimeError(f"CRAFT_FONTS_DIR must contain fonts/manifest.txt: {FONTS}")


def build() -> tuple[Path, str, str] | None:
    commit = git("rev-parse", "HEAD")
    before = fingerprint()
    name = f"{commit}-{before[:16]}"
    frozen = RUNTIME / name / "photostore"
    if frozen.is_file():
        return frozen, name, before
    validate_fonts()
    frozen.parent.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.pop("PHOTOSTORE_DEV_GOAL", None)
    env.pop("PHOTOSTORE_CONFIG_DIR", None)
    env.update({"CARGO_TARGET_DIR": str(TARGET), "CARGO_BUILD_JOBS": "1", "RAYON_NUM_THREADS": "1",
                "PHOTOSTORE_BUILD_SHA": commit, "PHOTOSTORE_BUILD_DATE": time.strftime("%Y-%m-%d"),
                "CRAFT_FONTS_REQUIRED": "1", "CRAFT_FONTS_DIR": str(FONTS)})
    BUILD_LOCK.parent.mkdir(parents=True, exist_ok=True)
    with BUILD_LOCK.open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        log(f"[build] {commit[:12]} working-tree={before[:12]}")
        try:
            subprocess.run(["nice", "-n", "10", "cargo", "build", "--locked", "--offline", "-p", "photostore", "--bin", "photostore", "--features", "dev-install,heif"], cwd=ROOT, env=env, check=True)
        except subprocess.CalledProcessError as exc:
            log(f"[build] failed ({exc.returncode}); keeping current app")
            return None
        source = TARGET / "debug/photostore"
        if not source.is_file():
            log("[build] cargo completed without target/debug/photostore")
            return None
        marker = subprocess.run([str(source), "--version"], check=True, text=True, capture_output=True).stdout
        if "PhotoStore-dev build" not in marker or commit[:9] not in marker:
            log("[build] refusing binary without expected dev marker/provenance")
            return None
        after = fingerprint()
        if after != before:
            log("[build] source changed during build; next pass will rebuild")
            return None
        temp = frozen.with_name(f".{frozen.name}.{os.getpid()}.tmp")
        shutil.copy2(source, temp)
        os.chmod(temp, 0o755)
        os.replace(temp, frozen)
    return frozen, name, before


def choose_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def start(binary: Path, name: str) -> tuple[subprocess.Popen[bytes], int]:
    port = choose_port()
    env = os.environ.copy()
    env.pop("PHOTOSTORE_DEV_GOAL", None)
    env.pop("PHOTOSTORE_CONFIG_DIR", None)
    args = [str(binary), "--control", str(port), "--control-token-file", str(TOKEN), "--automation-read-root", str(ROOT), "--automation-write-root", str(ROOT)]
    child = subprocess.Popen(args, cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    atomic_write(STATE / "child.json", json.dumps({"pid": child.pid, "binary": str(binary), "runtime": name, "port": port}).encode())
    log(f"[app] started pid={child.pid} runtime={name}")
    return child, port


def request_reload(child: subprocess.Popen[bytes], port: int) -> bool:
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=1.0) as conn:
            reader = conn.makefile("rb")
            conn.sendall((json.dumps({"id": 1, "method": "auth", "params": {"token": TOKEN.read_text().strip()}}) + "\n").encode())
            if not json.loads(reader.readline()).get("ok"):
                return False
            conn.sendall(b'{"id":2,"method":"app.devReload","params":{}}\n')
            reply = json.loads(reader.readline())
            if reply.get("ok"):
                return True
            log(f"[reload] deferred: {reply.get('error', 'unknown reason')}")
            return False
    except (OSError, ValueError, json.JSONDecodeError):
        return child.poll() is not None


def save_pending(result: tuple[Path, str, str]) -> dict:
    binary, name, source = result
    data = {"binary": str(binary), "runtime": name, "source": source}
    atomic_write(PENDING, json.dumps(data).encode())
    return data


def read_pending() -> dict | None:
    try:
        return json.loads(PENDING.read_text(encoding="utf-8")) if PENDING.exists() else None
    except (OSError, json.JSONDecodeError):
        return None


def write_lease(child_pid: int | None, binary: Path | None) -> None:
    atomic_write(LEASE, json.dumps({"watcher_pid": os.getpid(), "child_pid": child_pid, "binary": str(binary) if binary else None, "uid": os.getuid(), "state": str(STATE)}).encode())


def run(once: bool) -> int:
    ensure_state()
    with OWNER_LOCK.open("a+") as owner:
        try:
            fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise SystemExit("photostore-dev: another watcher already owns this runtime")
        PID_FILE.write_text(str(os.getpid()), encoding="ascii")
        write_lease(None, None)
        child: subprocess.Popen[bytes] | None = None
        port = 0
        applied = ""
        failed = ""
        pending = read_pending()
        try:
            while True:
                current = fingerprint()
                if child is None or child.poll() is not None:
                    if pending and Path(pending["binary"]).is_file():
                        child, port = start(Path(pending["binary"]), pending["runtime"])
                        write_lease(child.pid, Path(pending["binary"]))
                        applied = pending["source"]
                        pending = None
                        PENDING.unlink(missing_ok=True)
                    elif current != applied and current != failed:
                        result = build()
                        if result is None:
                            failed = current
                        else:
                            child, port = start(result[0], result[1])
                            write_lease(child.pid, result[0])
                            applied, failed = result[2], ""
                current = fingerprint()
                if child is not None and child.poll() is None and current != applied and current != failed:
                    result = build()
                    if result is not None:
                        pending = save_pending(result)
                        if request_reload(child, port):
                            deadline = time.monotonic() + 5
                            while child.poll() is None and time.monotonic() < deadline:
                                time.sleep(0.1)
                            if child.poll() is not None:
                                child = None
                                write_lease(None, None)
                if once:
                    return 0
                time.sleep(0.5)
        except KeyboardInterrupt:
            log("[watch] stopped; owned app left running")
            return 0
        finally:
            PID_FILE.unlink(missing_ok=True)
            if child is None or child.poll() is not None:
                LEASE.unlink(missing_ok=True)


def status() -> None:
    child = json.loads((STATE / "child.json").read_text()) if (STATE / "child.json").exists() else None
    print(json.dumps({"state": str(STATE), "watcher_pid": PID_FILE.read_text().strip() if PID_FILE.exists() else None, "child": child, "pending": read_pending()}, indent=2))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--watch", action="store_true")
    modes.add_argument("--once", action="store_true")
    modes.add_argument("--status", action="store_true")
    parser.add_argument("--background", action="store_true", help="detach the watcher and its owned child")
    args = parser.parse_args()
    if args.status:
        status()
        return 0
    if args.background:
        child = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), "--once" if args.once else "--watch"], cwd=ROOT, start_new_session=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        print(f"photostore-dev watcher started pid={child.pid}")
        return 0
    return run(args.once)


if __name__ == "__main__":
    raise SystemExit(main())
