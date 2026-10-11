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
BACKGROUND_BUNDLE = RUNTIME / "PhotoStore-dev.app"
EXCLUDED = {".git", "target", "node_modules", "docs", ".codex", "dev-runtime", "screenshots"}
SUFFIXES = {".rs", ".toml", ".lock", ".json", ".ron", ".wgsl", ".glsl", ".metal", ".plist", ".sh", ".py", ".svg", ".icns", ".ttf", ".otf", ".png"}
LAST_DEFERRED = ""


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
        dirs[:] = [d for d in dirs if d not in EXCLUDED and (not d.startswith(".") or d == ".cargo")]
        for name in files:
            path = Path(base) / name
            if path.suffix in SUFFIXES and path.is_file():
                result.append(path)
    return sorted(result)


def fingerprint() -> str:
    digest = hashlib.sha256()
    for path in source_files():
        try:
            stat = path.stat()
        except OSError:
            continue
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
        try:
            marker = subprocess.run([str(frozen), "--version"], check=True, text=True, capture_output=True, timeout=5).stdout
            if "PhotoStore-dev build" in marker and commit[:9] in marker:
                return frozen, name, before
        except (OSError, subprocess.SubprocessError):
            pass
        frozen.unlink(missing_ok=True)
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
        copied_marker = subprocess.run([str(frozen), "--version"], check=True, text=True, capture_output=True, timeout=5).stdout
        if "PhotoStore-dev build" not in copied_marker or commit[:9] not in copied_marker:
            frozen.unlink(missing_ok=True)
            log("[build] copied frozen binary failed provenance validation")
            return None
    return frozen, name, before


def choose_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


class OwnedProcess:
    def __init__(self, pid: int, process: subprocess.Popen[bytes] | None = None, binary: Path | None = None, port: int = 0) -> None:
        self.pid = pid
        self.process = process
        self.binary = str(binary) if binary else None
        self.port = port

    def poll(self) -> int | None:
        if self.process is not None:
            return self.process.poll()
        command = ps_command(self.pid)
        if command is None or (self.binary and not command.startswith(self.binary)) or (self.port and f"--control {self.port}" not in command) or (self.binary and str(TOKEN) not in command):
            return 1
        return None


def ps_command(pid: int) -> str | None:
    for line in subprocess.run(["/bin/ps", "-axo", "pid=,command="], text=True, capture_output=True, check=True).stdout.splitlines():
        fields = line.strip().split(None, 1)
        if len(fields) == 2 and fields[0].isdigit() and int(fields[0]) == pid:
            return fields[1]
    return None


def background_bundle(binary: Path) -> None:
    executable = BACKGROUND_BUNDLE / "Contents/MacOS/PhotoStore-dev"
    resources = BACKGROUND_BUNDLE / "Contents/Resources"
    resources.mkdir(parents=True, exist_ok=True)
    executable.parent.mkdir(parents=True, exist_ok=True)
    pointer = BACKGROUND_BUNDLE / "Contents/Resources/current-binary"
    atomic_write(pointer, (str(binary) + "\n").encode(), 0o600)
    if not executable.exists():
        executable.write_text("#!/bin/sh\nset -eu\nexec \"$(cat \"$(dirname \"$0\")/../Resources/current-binary\")\" \"$@\"\n", encoding="utf-8")
        os.chmod(executable, 0o755)
    info = BACKGROUND_BUNDLE / "Contents/Info.plist"
    if not info.exists():
        info.write_text("<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>CFBundleExecutable</key><string>PhotoStore-dev</string><key>CFBundleIdentifier</key><string>ai.photostore.app.dev</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleName</key><string>PhotoStore-dev</string></dict></plist>", encoding="utf-8")


def start(binary: Path, name: str) -> tuple[OwnedProcess, int]:
    port = choose_port()
    env = os.environ.copy()
    env.pop("PHOTOSTORE_DEV_GOAL", None)
    env.pop("PHOTOSTORE_CONFIG_DIR", None)
    args = [str(binary), "--control", str(port), "--control-token-file", str(TOKEN), "--automation-read-root", str(ROOT), "--automation-write-root", str(ROOT)]
    if env.get("PHOTOSTORE_DEV_BACKGROUND") == "1" and sys.platform == "darwin":
        background_bundle(binary)
        subprocess.run(["open", "-n", "-g", str(BACKGROUND_BUNDLE), "--args", *args[1:]], cwd=ROOT, env=env, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        pid = None
        for _ in range(50):
            lines = subprocess.run(["/bin/ps", "-axo", "pid=,command="], text=True, capture_output=True, check=True).stdout.splitlines()
            matches = [int(line.split(None, 1)[0]) for line in lines if len(line.split(None, 1)) == 2 and line.split(None, 1)[1].startswith(str(binary))]
            if matches:
                pid = matches[-1]
                break
            time.sleep(0.1)
        if pid is None:
            raise RuntimeError("background PhotoStore child did not appear")
        child = OwnedProcess(pid, binary=binary, port=port)
    else:
        process = subprocess.Popen(args, cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        child = OwnedProcess(process.pid, process, binary, port)
    atomic_write(STATE / "child.json", json.dumps({"pid": child.pid, "binary": str(binary), "runtime": name, "port": port}).encode())
    log(f"[app] started pid={child.pid} runtime={name}")
    return child, port


def request_reload(child: OwnedProcess, port: int) -> bool:
    global LAST_DEFERRED
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
            reason = reply.get("error", "unknown reason")
            if reason != LAST_DEFERRED:
                log(f"[reload] deferred: {reason}")
                LAST_DEFERRED = reason
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


def child_lease_live() -> bool:
    try:
        lease = json.loads(LEASE.read_text(encoding="utf-8"))
        pid, binary, uid = lease.get("child_pid"), lease.get("binary"), lease.get("uid")
        if uid != os.getuid() or not isinstance(pid, int) or not isinstance(binary, str):
            return False
        os.kill(pid, 0)
        command = ps_command(pid)
        return command is not None and command.startswith(binary) and Path(binary).is_relative_to(RUNTIME)
    except (OSError, ValueError, json.JSONDecodeError):
        return False


def consolidated_dev_alive() -> bool:
    lines = subprocess.run(["/bin/ps", "-axo", "pid=,command="], text=True, capture_output=True, check=True).stdout.splitlines()
    return any("/Applications/PhotoStore-dev.app/Contents/MacOS/PhotoStore-dev" in line for line in lines)


def write_lease(child_pid: int | None, binary: Path | None) -> None:
    atomic_write(LEASE, json.dumps({"watcher_pid": os.getpid(), "child_pid": child_pid, "binary": str(binary) if binary else None, "uid": os.getuid(), "state": str(STATE)}).encode())


def run(once: bool) -> int:
    STATE.mkdir(parents=True, exist_ok=True)
    with OWNER_LOCK.open("a+") as owner:
        try:
            fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise SystemExit("photostore-dev: another watcher already owns this runtime")
        if child_lease_live():
            raise SystemExit("photostore-dev: an owned dev child is still running; refusing a second watcher")
        if consolidated_dev_alive():
            raise SystemExit("photostore-dev: consolidated PhotoStore-dev is running outside this watcher; refusing to attach")
        ensure_state()
        PID_FILE.write_text(str(os.getpid()), encoding="ascii")
        write_lease(None, None)
        child: OwnedProcess | None = None
        port = 0
        applied = ""
        failed = ""
        retry_at = 0.0
        pending = read_pending()
        try:
            while True:
                current = fingerprint()
                if pending and pending.get("source") != current:
                    pending = None
                    PENDING.unlink(missing_ok=True)
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
                            if once:
                                return 1
                            failed = current
                        else:
                            child, port = start(result[0], result[1])
                            write_lease(child.pid, result[0])
                            applied, failed = result[2], ""
                current = fingerprint()
                if child is not None and child.poll() is None and current != applied and current != failed:
                    if pending is None:
                        result = build()
                        if result is not None:
                            pending = save_pending(result)
                        else:
                            failed = current
                    if pending is not None and pending.get("source") == current and time.monotonic() >= retry_at and request_reload(child, port):
                        deadline = time.monotonic() + 5
                        while child.poll() is None and time.monotonic() < deadline:
                            time.sleep(0.1)
                        if child.poll() is not None:
                            child = None
                            write_lease(None, None)
                    elif pending is not None and pending.get("source") == current and time.monotonic() >= retry_at:
                        retry_at = time.monotonic() + 2.0
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
        env = os.environ.copy(); env["PHOTOSTORE_DEV_BACKGROUND"] = "1"
        child = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), "--once" if args.once else "--watch"], cwd=ROOT, env=env, start_new_session=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        print(f"photostore-dev watcher started pid={child.pid}")
        return 0
    return run(args.once)


if __name__ == "__main__":
    raise SystemExit(main())
