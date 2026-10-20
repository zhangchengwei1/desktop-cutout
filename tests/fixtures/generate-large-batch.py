"""Generate a deterministic, high-entropy batch load probe without user images."""

import argparse
import hashlib
import json
import shutil
import struct
import subprocess
import zlib
from pathlib import Path


SHAPES = (
    (512, 512, "rgb-png"),
    (1024, 768, "rgba-png"),
    (768, 1024, "jpeg"),
    (1280, 720, "rgb-png"),
    (720, 1280, "rgba-png"),
    (1600, 900, "jpeg"),
    (900, 1600, "rgba-png"),
    (1536, 1024, "rgb-png"),
)


def chunk(kind: bytes, payload: bytes) -> bytes:
    payload = bytes(payload)
    return (
        struct.pack(">I", len(payload))
        + kind
        + payload
        + struct.pack(">I", zlib.crc32(kind + payload))
    )


def pixels(index: int, width: int, height: int, channels: int, seed: int) -> bytes:
    key = f"clearframe-large-batch-v1:{seed}:{index}:{width}:{height}".encode()
    raw = bytearray(hashlib.shake_256(key).digest(width * height * channels))
    stride = width * channels
    if channels == 4:
        for y in range(height):
            start = y * stride + 3
            raw[start : start + stride : 4] = bytes([96 + index % 64]) * width
    left, right = width // 4, width * 3 // 4
    top, bottom = height // 4, height * 3 // 4
    foreground = bytes((210, 54 + index % 100, 76) + ((255,) if channels == 4 else ()))
    stripe = foreground * (right - left)
    for y in range(top, bottom):
        start = y * stride + left * channels
        raw[start : start + len(stripe)] = stripe
    return bytes(raw)


def write_png(path: Path, width: int, height: int, channels: int, raw: bytes) -> None:
    stride = width * channels
    compressor = zlib.compressobj(level=1)
    with path.open("wb") as output:
        output.write(b"\x89PNG\r\n\x1a\n")
        output.write(
            chunk(
                b"IHDR",
                struct.pack(">IIBBBBB", width, height, 8, 6 if channels == 4 else 2, 0, 0, 0),
            )
        )
        pending = bytearray()
        for y in range(height):
            pending.extend(compressor.compress(b"\0" + raw[y * stride : (y + 1) * stride]))
            if len(pending) >= 1024 * 1024:
                output.write(chunk(b"IDAT", pending))
                pending.clear()
        pending.extend(compressor.flush())
        if pending:
            output.write(chunk(b"IDAT", pending))
        output.write(chunk(b"IEND", b""))


def write_jpeg(path: Path, width: int, height: int, raw: bytes, ffmpeg: str) -> None:
    subprocess.run(
        [
            ffmpeg,
            "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24",
            "-s", f"{width}x{height}", "-i", "pipe:0", "-frames:v", "1",
            "-q:v", "3", "-threads", "1", str(path),
        ],
        input=raw,
        check=True,
        stderr=subprocess.PIPE,
    )


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as source:
        for data in iter(lambda: source.read(1024 * 1024), b""):
            hasher.update(data)
    return hasher.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--count", type=int, default=100)
    parser.add_argument("--seed", type=int, default=20260929)
    args = parser.parse_args()
    if not 1 <= args.count <= 1000:
        parser.error("--count must be between 1 and 1000")
    ffmpeg = shutil.which("ffmpeg")
    if args.count >= 3 and ffmpeg is None:
        parser.error("ffmpeg is required for the JPEG cases")
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    images = root / "images"
    images.mkdir()
    cases = []
    manifest = []
    for index in range(args.count):
        width, height, kind = SHAPES[index % len(SHAPES)]
        channels = 4 if kind == "rgba-png" else 3
        name = f"synthetic-{index:03d}.{'jpg' if kind == 'jpeg' else 'png'}"
        path = images / name
        raw = pixels(index, width, height, channels, args.seed)
        if kind == "jpeg":
            write_jpeg(path, width, height, raw, ffmpeg)
        else:
            write_png(path, width, height, channels, raw)
        cases.append({"path": f"images/{name}", "repeat": 1})
        manifest.append({
            "path": f"images/{name}",
            "width": width,
            "height": height,
            "kind": kind,
            "bytes": path.stat().st_size,
            "sha256": digest(path),
        })
    (root / "plan.json").write_text(
        json.dumps({
            "schemaVersion": 1,
            "description": "Distinct deterministic high-entropy synthetic images for native batch load; not photo quality evidence",
            "cases": cases,
        }, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    (root / "manifest.json").write_text(
        json.dumps({
            "schemaVersion": 1,
            "generator": "generate-large-batch.py",
            "license": "CC0-1.0; generated for this repository",
            "seed": args.seed,
            "totalBytes": sum(item["bytes"] for item in manifest),
            "totalPixels": sum(item["width"] * item["height"] for item in manifest),
            "images": manifest,
        }, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(root / "plan.json")


if __name__ == "__main__":
    main()
