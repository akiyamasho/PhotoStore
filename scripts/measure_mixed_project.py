#!/usr/bin/env python3
"""Measure a disposable PhotoStore mixed-project session over its control channel.

Run against the same fixture and application executable for comparable measurements.
The output JSON preserves raw process, GPU and publication samples; the separate
vmmap file is scoped to the child process launched by this script.
"""

import argparse
import json
import math
import os
import pathlib
import socket
import subprocess
import tempfile
import time


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--binary", required=True)
    p.add_argument("--root", required=True)
    p.add_argument("--project", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--play-seconds", type=float, default=10)
    p.add_argument("--activate", action="store_true", help="Activate the disposable macOS window for measuring canvas publications")
    p.add_argument("--passive-playback", action="store_true", help="Sample the native callback history after playback, without inspection during playback")
    a = p.parse_args()
    root = pathlib.Path(a.root).resolve()
    project = pathlib.Path(a.project)
    if project.is_absolute() or ".." in project.parts:
        raise SystemExit("project must be relative to root")
    run = pathlib.Path(tempfile.mkdtemp(prefix="photostore-native-probe-"))
    config = run / "config"
    config.mkdir()
    token = run / "token"
    log = (run / "app.log").open("wb")
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    env = os.environ.copy()
    env["PHOTOSTORE_CONFIG_DIR"] = str(config)
    env["PHOTOSTORE_MEDIA_CACHE_DIR"] = str(run / "media-cache")
    env["PHOTOSTORE_NATIVE_TIMING_TRACE"] = "1"
    for k in (
        "PHOTOSTORE_CONTROL_PORT",
        "PHOTOSTORE_CONTROL_TOKEN",
        "PHOTOSTORE_CONTROL_TOKEN_FILE",
        "PHOTOSTORE_AUTOMATION_READ_ROOT",
        "PHOTOSTORE_AUTOMATION_WRITE_ROOT",
    ):
        env.pop(k, None)
    t0 = time.monotonic()
    child = subprocess.Popen(
        [
            a.binary,
            "--control",
            str(port),
            "--control-token-file",
            str(token),
            "--automation-read-root",
            str(root),
        ],
        env=env,
        stdout=log,
        stderr=subprocess.STDOUT,
    )
    result = {
        "control_calls": [],
        "run": str(run),
        "pid": child.pid,
        "binary": a.binary,
        "project": str(project),
        "limitations": [
            "Synthetic control-channel interaction, not native OS input. Background/occluded rendering can throttle cadence.",
            "GPU budget and compositor timing are exposed; actual GPU residency is not exposed by ui.inspect. RSS is sampled, not a true peak.",
            "Each timed exact seek invokes timeline.setFrame followed by additional timeline.presentFrame polling through automation; this is not a continuous mouse scrub.",
            "Publication cadence is native callback timing, not independently measured screen presentation; tool timings are synthetic control round trips.",
            "open_to_decode_ready_ms is app.open call duration plus decode polling duration; it excludes intervening panel/inspection time. open_to_native_publication_ms is wall time since app.open began until the first observed native publication, including panel setup and polling.",
        ],
        "samples": [],
    }
    try:
        deadline = time.monotonic() + 60
        while True:
            if child.poll() is not None:
                raise RuntimeError(
                    "app exited during startup: " + str(child.returncode)
                )
            try:
                if token.exists():
                    conn = socket.create_connection(("127.0.0.1", port), timeout=1)
                    break
            except OSError:
                pass
            if time.monotonic() > deadline:
                raise TimeoutError("control startup")
            time.sleep(0.1)
        conn.settimeout(65)
        f = conn.makefile("rwb", buffering=0)
        serial = 0

        def call(method, params={}):
            nonlocal serial
            serial += 1
            start = time.monotonic()
            f.write(
                (
                    json.dumps({"id": serial, "method": method, "params": params})
                    + "\n"
                ).encode()
            )
            reply = json.loads(f.readline())
            elapsed = (time.monotonic() - start) * 1000
            result["control_calls"].append({"method": method, "elapsed_ms": elapsed})
            if not reply.get("ok"):
                raise RuntimeError(f"{method}: {reply}")
            return reply.get("result"), elapsed

        call("auth", {"token": token.read_text().strip()})
        result["startup_ms"] = (time.monotonic() - t0) * 1000
        if a.activate:
            activation = subprocess.run(
                ["/usr/bin/osascript", "-l", "JavaScript", "-e", "ObjC.import('AppKit'); function run(argv) { return $.NSRunningApplication.runningApplicationWithProcessIdentifier(Number(argv[0])).activateWithOptions(2) }", str(child.pid)],
                capture_output=True, text=True, timeout=12, check=True,
            )
            result["window_activation"] = activation.stdout.strip()
            call("ui.focus")
        open_start = time.monotonic()
        opened, elapsed = call("app.open", {"path": str(project)})
        result["open_call_ms"] = elapsed
        result["open"] = opened

        def inspect():
            v, _ = call("ui.inspect")
            rss = subprocess.run(
                ["ps", "-o", "rss=", "-p", str(child.pid)],
                capture_output=True,
                text=True,
            ).stdout.strip()
            result["samples"].append(
                {
                    "at_s": time.monotonic() - t0,
                    "rss_kib": int(rss) if rss else None,
                    "perf": v.get("perf"),
                    "document_timeline": (v.get("document") or {}).get("timeline"),
                    "status": v.get("status"),
                    "statusError": v.get("statusError"),
                }
            )
            return v

        v = inspect()
        timeline = v["document"]["timeline"]
        duration = timeline["duration"]
        result["timeline"] = timeline
        panel, _ = call("ui.menu.invoke", {"id": "window.panel.timeline"})
        if not panel.get("open"):
            panel, _ = call("ui.menu.invoke", {"id": "window.panel.timeline"})
        if not panel.get("open"):
            raise RuntimeError("timeline panel failed to open")
        result["timeline_panel"] = panel

        # Decode readiness is distinct from native texture publication.
        def present(frame):
            start = time.monotonic()
            deadline = start + 60
            n = 0
            while True:
                r, _ = call(
                    "engine.execute",
                    {"command": "timeline.presentFrame", "params": {"frame": frame}},
                )
                n += 1
                if r.get("ready"):
                    return {
                        "frame": frame,
                        "ready_ms": (time.monotonic() - start) * 1000,
                        "polls": n,
                    }
                if time.monotonic() > deadline:
                    raise TimeoutError(f"frame {frame} decode")
                time.sleep(0.02)

        result["open_to_decode_ready_ms"] = (
            result["open_call_ms"] + present(timeline["current"])["ready_ms"]
        )

        def wait_publication(frame, prior, start):
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                v = inspect()
                t = v["perf"]["timings"]
                if (
                    t.get("nativePublications", 0) > prior
                    and t.get("nativePresentedFrame") == frame
                ):
                    return {
                        "published": True,
                        "seek_to_publication_ms": (time.monotonic() - start) * 1000,
                        "publication_count": t["nativePublications"],
                        "frame": frame,
                    }
                time.sleep(0.03)
            return {
                "published": False,
                "seek_to_publication_ms": None,
                "frame": frame,
                "timeout_s": 30,
            }

        # A warm ready sample requires an installed native texture, not merely a decoded frame.
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            v = inspect()
            t = v["perf"]["timings"]
            if t.get("nativePublications", 0) > 0:
                break
            time.sleep(0.03)
        result["warm_native_ready"] = t.get("nativePublications", 0) > 0
        result["open_to_native_publication_ms"] = (
            (time.monotonic() - open_start) * 1000
            if result["warm_native_ready"]
            else None
        )
        result["initial"] = v["perf"]
        try:
            mem = subprocess.run(
                ["vmmap", "-summary", str(child.pid)],
                capture_output=True,
                text=True,
                timeout=30,
            )
            (run / "vmmap-summary.txt").write_text(mem.stdout + "\n" + mem.stderr)
            result["vmmap"] = {
                "exit_code": mem.returncode,
                "path": str(run / "vmmap-summary.txt"),
                "ioaccelerator_iosurface_lines": [
                    x
                    for x in mem.stdout.splitlines()
                    if "IOAccelerator" in x or "IOSurface" in x
                ],
            }
        except Exception as e:
            result["vmmap"] = {"error": repr(e)}
        frames = [
            int((duration - 1) * x)
            for x in (0.05, 0.8, 0.2, 0.95, 0.4, 0.65, 0.1, 0.5, 0.9, 0.3)
        ]
        result["scrubs"] = []
        for frame in frames:
            prior = inspect()["perf"]["timings"].get("nativePublications", 0)
            seek_start = time.monotonic()
            _, ms = call(
                "engine.execute",
                {"command": "timeline.setFrame", "params": {"frame": frame}},
            )
            ready = present(frame)
            ready["set_frame_call_ms"] = ms
            ready.update(wait_publication(frame, prior, seek_start))
            result["scrubs"].append(ready)
            if not ready["published"]:
                raise TimeoutError(f"frame {frame} decoded but no new Metal publication was observed within 30 seconds")
        call("engine.execute", {"command": "timeline.setFrame", "params": {"frame": 0}})
        present(0)
        if a.activate:
            call("ui.focus")
        before_play = inspect()["perf"]["timings"]
        baseline = before_play.get("nativeVideoPublications", [])
        last_preplay = max((x["atMs"] for x in baseline), default=-1)
        call("engine.execute", {"command": "timeline.playPause"})
        start = time.monotonic()
        trace = {}
        result["playback_inspection"] = not a.passive_playback
        if a.passive_playback:
            # A blank line keeps the server socket alive without dispatching a UI command.
            deadline = start + a.play_seconds
            result["playback_process_samples"] = []
            while (remaining := deadline - time.monotonic()) > 0:
                time.sleep(min(1, remaining))
                f.write(b"\n")
                stats = subprocess.run(["ps", "-o", "%cpu=,rss=", "-p", str(child.pid)],
                                       capture_output=True, text=True, check=True).stdout.split()
                if len(stats) == 2:
                    result["playback_process_samples"].append({"at_s": time.monotonic() - start,
                                                              "cpu_percent": float(stats[0]), "rss_kib": int(stats[1])})
        while not a.passive_playback and time.monotonic() - start < a.play_seconds:
            v = inspect()
            for item in v["perf"]["timings"].get("nativeVideoPublications", []):
                if item["atMs"] > last_preplay:
                    trace[(item["atMs"], item["frame"])] = item
            time.sleep(0.1)
        play_elapsed = time.monotonic() - start
        call("engine.execute", {"command": "timeline.playPause"})
        v = inspect()
        for item in v["perf"]["timings"].get("nativeVideoPublications", []):
            if item["atMs"] > last_preplay:
                trace[(item["atMs"], item["frame"])] = item
        publications = sorted(trace.values(), key=lambda x: x["atMs"])
        intervals = [
            b["atMs"] - a["atMs"]
            for a, b in zip(publications, publications[1:])
            if b["atMs"] > a["atMs"]
        ]
        result["playback_perf"] = v["perf"]
        result["playback_wall_s"] = time.monotonic() - start
        result["playback"] = {
            "publications": publications,
            "whole_window_fps": (v["perf"]["timings"]["nativeVideoFrames"] - before_play["nativeVideoFrames"]) / play_elapsed,
            "measurement_seconds": play_elapsed,
            "distinct_frame_indices": len(set(x["frame"] for x in publications)),
            "progressed": len(set(x["frame"] for x in publications)) > 1,
            "observed_publications_per_second": (len(publications) - 1)
            * 1000
            / (publications[-1]["atMs"] - publications[0]["atMs"])
            if len(publications) > 1
            and publications[-1]["atMs"] > publications[0]["atMs"]
            else None,
            "inter_publication_ms": intervals,
            "loop_wraps": sum(
                b["frame"] < a["frame"] for a, b in zip(publications, publications[1:])
            ),
        }
        result["tool_switch_ms"] = []
        for tool in (
            "brush",
            "move",
            "brush",
            "move",
            "brush",
            "move",
            "brush",
            "move",
        ):
            _, ms = call("ui.set", {"tool": tool})
            actual = inspect().get("tool")
            result["tool_switch_ms"].append(
                {
                    "tool": tool,
                    "ms": ms,
                    "actual_tool": actual,
                    "verified": str(actual).lower() == tool,
                }
            )
        if a.activate:
            call("ui.focus")
        result["tool_shortcuts"] = []
        for key, target in (("B", "brush"), ("V", "move")):
            started = time.monotonic()
            call("ui.key", {"key": key})
            deadline = started + 2
            while True:
                tool = str(inspect().get("tool")).lower()
                if tool == target or time.monotonic() >= deadline:
                    break
                time.sleep(0.02)
            result["tool_shortcuts"].append({"key": key, "actual": tool, "verified": tool == target,
                                              "observed_ms": (time.monotonic() - started) * 1000})
        # This untimed probe samples a document-space point from the GPU texture. It is
        # evidence for a later comparison, not a correctness verdict by itself.
        document = v.get("document") or {}
        point = [document.get("width", 1) // 2, document.get("height", 1) // 2]
        try:
            sample, _ = call("ui.gpu.sample", {"points": [point]})
            result["native_pixel_sample"] = {"point": point, "result": sample}
        except Exception as error:
            result["native_pixel_sample"] = {"point": point, "error": repr(error)}
        result["final"] = inspect()
        result["alive_at_end"] = child.poll() is None
        vals = sorted(x["ready_ms"] for x in result["scrubs"])
        result["scrub_nearest_rank_ms"] = {
            str(q): vals[math.ceil(q * len(vals)) - 1] for q in (0.5, 0.95, 1)
        }

        def nearest_rank(values):
            ordered = sorted(values)
            return (
                {
                    str(q): ordered[math.ceil(q * len(ordered)) - 1]
                    for q in (0.5, 0.95, 1)
                }
                if ordered
                else {}
            )

        result["aggregation"] = {
            "method": "nearest rank: sorted[ceil(q * N) - 1]; timeouts excluded and counted separately",
            "seek_to_publication_ms": nearest_rank(
                x["seek_to_publication_ms"]
                for x in result["scrubs"]
                if x["seek_to_publication_ms"] is not None
            ),
            "seek_publication_timeouts": sum(
                not x["published"] for x in result["scrubs"]
            ),
            "playback_inter_publication_ms": nearest_rank(intervals),
            "tool_switch_ms": nearest_rank(x["ms"] for x in result["tool_switch_ms"]),
        }
    except Exception as e:
        result["error"] = repr(e)
        result["alive_at_error"] = child.poll() is None
    finally:
        # Only the disposable subprocess launched by this runner is signalled.
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        result["peak_sampled_rss_kib"] = max(
            (x["rss_kib"] or 0 for x in result["samples"]), default=0
        )
        result["max_control_call_ms"] = max(
            (x["elapsed_ms"] for x in result["control_calls"]), default=0
        )
        result["exit_code_after_cleanup"] = child.returncode
        log.close()
        pathlib.Path(a.out).write_text(json.dumps(result, indent=2))
        print(
            json.dumps({"output": a.out, "run": str(run), "error": result.get("error")})
        )
    if "error" in result:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
