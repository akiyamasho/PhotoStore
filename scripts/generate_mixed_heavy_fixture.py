#!/usr/bin/env python3
"""Generate a disposable mixed-media load fixture; output directory must not exist.

Requires ffmpeg and the installed photostore-cli. Keep the complete output directory:
video layers reference its files by absolute path. No user preferences are modified.
"""

import argparse
import json
import random
import struct
import zlib
import os
from pathlib import Path
import subprocess


def dense(source, root, cli):
    """Add 24 distinct 2560x1440 translucent pixel layers in bounded batches."""
    source = Path(source).resolve()
    root = Path(root).resolve()
    if not source.is_file():
        raise SystemExit(f"Heavy source not found: {source}")
    root.mkdir(parents=True, exist_ok=False)
    config = root / "config"
    config.mkdir()
    env = dict(
        os.environ,
        PHOTOSTORE_CONFIG_DIR=str(config),
        XDG_CONFIG_HOME=str(config),
        XDG_CACHE_HOME=str(root / "cache"),
        TMPDIR=str(root),
    )

    def chunk(kind, data):
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    def png(path, seed):
        width, height = 2560, 1440
        rng = random.Random(seed)
        compressor = zlib.compressobj(1)
        with path.open("wb") as f:
            f.write(b"\x89PNG\r\n\x1a\n")
            f.write(
                chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            )
            for y in range(height):
                # Low, uniform alpha keeps the underlying mixed-media composition visible.
                row = bytearray(rng.randbytes(width * 4))
                row[3::4] = bytes([8]) * width
                data = compressor.compress(b"\x00" + row)
                if data:
                    f.write(chunk(b"IDAT", data))
            data = compressor.flush()
            if data:
                f.write(chunk(b"IDAT", data))
            f.write(chunk(b"IEND", b""))

    current = source
    for batch in range(8):
        images = []
        args = [cli, "run", str(current)]
        for i in range(batch * 3, batch * 3 + 3):
            image = root / f"dense-source-{i:02}.png"
            png(image, 2026112300 + i)
            images.append(image)
            for command, params in [
                ("file.placeEmbedded", {"path": str(image)}),
                ("layer.rasterize.smartObject", {}),
            ]:
                args += ["--cmd", command, "--params", json.dumps(params)]
        output = root / (
            "mixed-dense.pcraft" if batch == 7 else f"stage-{batch:02}.pcraft"
        )
        subprocess.run(
            args + ["--out", str(output)],
            check=True,
            env=env,
            stdout=subprocess.DEVNULL,
        )
        if current.parent == root:
            current.unlink()
        for image in images:
            image.unlink()
        current = output
        print(
            f"Completed {batch * 3 + 3}/24 dense layers: {output.stat().st_size / 1048576:.1f} MiB",
            flush=True,
        )
    if current.stat().st_size < 200 * 1048576:
        raise SystemExit(
            "Generated project is smaller than 200 MiB; retain it for inspection."
        )
    print(current)


def heavy(root, cli, ffmpeg, seconds):
    root = Path(root).resolve()
    root.mkdir(parents=True, exist_ok=False)
    config = root / "config"
    config.mkdir()
    env = dict(
        os.environ,
        PHOTOSTORE_CONFIG_DIR=str(config),
        XDG_CONFIG_HOME=str(config),
        XDG_CACHE_HOME=str(root / "cache"),
        TMPDIR=str(root),
    )

    def run(args):
        subprocess.run(args, check=True, env=env, stdout=subprocess.DEVNULL)

    def doc(width, height, cmds, output):
        args = [cli, "run", "--new", json.dumps({"width": width, "height": height})]
        for cmd, params in cmds:
            args += ["--cmd", cmd, "--params", json.dumps(params)]
        run(args + ["--out", str(output)])

    def clip(name, size, effect):
        path = root / name
        source = f"testsrc2=size={size}:rate=30:duration={seconds},{effect}"
        run(
            [
                ffmpeg,
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                source,
                "-threads",
                "2",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-crf",
                "27",
                "-pix_fmt",
                "yuv420p",
                "-y",
                str(path),
            ]
        )
        return path

    videos = [
        clip("video1080.mp4", "1920x1080", "hue=h=40"),
        clip("video720.mp4", "1280x720", "hue=h=160"),
        clip("video-small.mp4", "320x180", "negate"),
    ]
    image = root / "image.png"
    run(
        [
            ffmpeg,
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=1",
            "-frames:v",
            "1",
            "-y",
            str(image),
        ]
    )
    children = []
    for i in range(12):
        path = root / f"child-{i:02}.pcraft"
        cmds = [
            (
                "file.placeEmbedded",
                {"path": str(image), "center": [150 + i * 8, 90 + i * 3]},
            ),
            ("layer.videoLayers.newVideoLayerFromFile", {"path": str(videos[2])}),
            ("timeline.setPositionAnimation", {"enabled": True}),
            (
                "timeline.setPositionKeyframe",
                {"frame": 30, "x": 20 + i * 7, "y": 10 + i * 3},
            ),
        ]
        doc(640, 360, cmds, path)
        children.append(path)
    nested = []
    for i, child in enumerate(children):
        path = root / f"nested-{i:02}.pcraft"
        doc(
            640,
            360,
            [
                ("file.placeEmbedded", {"path": str(child)}),
                ("layer.videoLayers.newVideoLayerFromFile", {"path": str(videos[2])}),
                ("timeline.setPositionAnimation", {"enabled": True}),
                ("timeline.setPositionKeyframe", {"frame": 30, "x": i * 5, "y": i * 4}),
            ],
            path,
        )
        nested.append(path)
    cmds = []
    for i in range(100):
        cmds += [
            (
                "file.placeEmbedded",
                {
                    "path": str(image),
                    "center": [180 + (i % 10) * 380, 110 + (i // 10) * 210],
                },
            ),
            ("layer.rasterize.smartObject", {}),
        ]
    for i, path in enumerate(nested):
        cmds += [
            (
                "file.placeEmbedded",
                {
                    "path": str(path),
                    "center": [480 + (i % 4) * 900, 360 + (i // 4) * 600],
                },
            )
        ]
    for video in videos:
        cmds += [
            ("layer.videoLayers.newVideoLayerFromFile", {"path": str(video)}),
            ("timeline.setPositionAnimation", {"enabled": True}),
            ("timeline.setPositionKeyframe", {"frame": 30, "x": 40, "y": 20}),
        ]
    doc(3840, 2160, cmds, root / "mixed-heavy.pcraft")
    print(root / "mixed-heavy.pcraft")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "output_dir", help="New disposable directory; must not already exist"
    )
    parser.add_argument(
        "--dense",
        metavar="HEAVY_PROJECT",
        help="Add dense layers to an existing heavy project",
    )
    parser.add_argument(
        "--cli", default="photostore-cli", help="PhotoStore CLI executable"
    )
    parser.add_argument("--ffmpeg", default="ffmpeg", help="ffmpeg executable")
    parser.add_argument("--seconds", type=int, default=2, help="Duration of each generated film source in seconds (default 2)")
    args = parser.parse_args()
    if args.seconds < 2:
        parser.error("--seconds must be at least 2")
    if args.dense:
        dense(args.dense, args.output_dir, args.cli)
    else:
        heavy(args.output_dir, args.cli, args.ffmpeg, args.seconds)


if __name__ == "__main__":
    main()
