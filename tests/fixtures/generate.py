#!/usr/bin/env python3
"""Generate small, public-domain comparison inputs with no Python packages."""

import argparse
import hashlib
import json
import math
import shutil
import struct
import subprocess
import zlib
from pathlib import Path


FIXTURES = {
    "hard-edge-opaque.png": (96, 64, "RGB", "硬边、背景纹理与 JPEG EXIF 的原图"),
    "wide-alpha.png": (160, 48, "RGBA", "超宽、半透明边缘和透明像素隐藏 RGB"),
    "portrait-strands.png": (48, 160, "RGBA", "超高、细线和半透明 Alpha"),
    "low-contrast.png": (96, 96, "RGB", "低对比度主体"),
    "mask-edges.png": (96, 64, "L", "全尺寸灰度蒙版和 Alpha 合成"),
    "gamma-tagged.png": (96, 64, "RGB", "带非标准 gAMA 色彩元数据"),
    "icc-tagged.png": (96, 64, "RGB", "带自生成 RGB ICC 配置文件"),
    "exif-orient6.jpg": (96, 64, "JPEG", "非对称图像与 EXIF 方向 6"),
    "corrupt-truncated.png": (96, 64, "broken PNG", "截断文件错误路径"),
    "oversize-header.png": (20001, 1, "RGB", "超过当前单边保护值的文件"),
}


def chunk(kind: bytes, payload: bytes) -> bytes:
    return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", zlib.crc32(kind + payload))


def png(width: int, height: int, mode: str, pixels: bytes, extra_chunks=()) -> bytes:
    channels, color = {"L": (1, 0), "RGB": (3, 2), "RGBA": (4, 6)}[mode]
    stride = width * channels
    rows = b"".join(b"\0" + pixels[y * stride : (y + 1) * stride] for y in range(height))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, color, 0, 0, 0))
            + b"".join(chunk(kind, data) for kind, data in extra_chunks)
            + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b""))


def signed_fixed(value: float) -> bytes:
    return struct.pack(">i", round(value * 65536))


def synthetic_icc() -> bytes:
    # Minimal self-authored ICC v2 RGB profile, close to sRGB primaries with
    # a simple gamma 2.2 curve. It is a format fixture, not a color reference.
    xyz = {
        b"rXYZ": (0.436075, 0.222504, 0.013932),
        b"gXYZ": (0.385065, 0.716879, 0.097105),
        b"bXYZ": (0.143080, 0.060617, 0.714173),
        b"wtpt": (0.964200, 1.0, 0.824900),
    }
    curve = b"curv" + bytes(4) + struct.pack(">IH", 1, round(2.2 * 256)) + bytes(2)
    tags = {key: b"XYZ " + bytes(4) + b"".join(signed_fixed(v) for v in value) for key, value in xyz.items()}
    tags.update({b"rTRC": curve, b"gTRC": curve, b"bTRC": curve})
    header = bytearray(128)
    header[8:12] = bytes([2, 0x10, 0, 0])
    header[12:24] = b"mntrRGB XYZ "
    header[36:40] = b"acsp"
    header[40:44] = b"pyth"
    header[64:68] = struct.pack(">I", 0)
    header[68:80] = b"".join(signed_fixed(v) for v in (0.9642, 1.0, 0.8249))
    header[80:84] = b"CFXM"
    table = bytearray(struct.pack(">I", len(tags)))
    data = bytearray()
    offset = 128 + 4 + len(tags) * 12
    for signature, payload in tags.items():
        table += signature + struct.pack(">II", offset + len(data), len(payload))
        data += payload
        data += bytes((-len(data)) % 4)
    profile = header + table + data
    profile[0:4] = struct.pack(">I", len(profile))
    return bytes(profile)


def pixel(name: str, x: int, y: int, width: int, height: int) -> tuple[int, ...]:
    if name in ("hard-edge-opaque.png", "gamma-tagged.png", "icc-tagged.png", "oversize-header.png"):
        background = (32 + x * 2 % 70, 80 + y * 3 % 90, 115 + (x // 6 + y // 6) % 2 * 28)
        return (226, 65, 38) if width // 4 <= x < width * 3 // 4 and height // 4 <= y < height * 3 // 4 else background
    if name == "wide-alpha.png":
        distance = math.sqrt(((x - 79.5) / 58) ** 2 + ((y - 23.5) / 18) ** 2)
        alpha = max(0, min(255, round((1.08 - distance) * 12 * 255)))
        return (40 + x % 90, 140, 230 - y, alpha)
    if name == "portrait-strands.png":
        center = 24 + round(7 * math.sin(y / 16))
        distance = abs(x - center)
        body = 255 if distance < 8 and 28 < y < 147 else 0
        strand = 110 if distance in (10, 13, 16) and y < 115 else 0
        return (42, 49 + y % 45, 63, max(body, strand))
    if name == "low-contrast.png":
        inside = ((x - 47.5) / 29) ** 2 + ((y - 47.5) / 33) ** 2 < 1
        base = 125 + (x + 2 * y) % 7
        return (base + (7 if inside else 0), base + (5 if inside else 0), base + (6 if inside else 0))
    if name == "mask-edges.png":
        return (0 if x < 24 else 255 if x >= 72 else round((x - 24) * 255 / 48),)
    raise ValueError(name)


def add_exif_orientation(jpeg: bytes, orientation: int) -> bytes:
    tiff = (b"II" + struct.pack("<HIH", 42, 8, 1)
            + struct.pack("<HHI", 0x0112, 3, 1) + struct.pack("<H", orientation) + bytes(2)
            + struct.pack("<I", 0))
    payload = b"Exif\0\0" + tiff
    return jpeg[:2] + b"\xff\xe1" + struct.pack(">H", len(payload) + 2) + payload + jpeg[2:]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parent / "images")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)

    for name, (width, height, mode, _) in FIXTURES.items():
        if mode not in ("L", "RGB", "RGBA"):
            continue
        pixels = bytes(channel for y in range(height) for x in range(width)
                       for channel in pixel(name, x, y, width, height))
        extra = []
        if name == "gamma-tagged.png":
            extra = [(b"gAMA", struct.pack(">I", 55000))]
        if name == "icc-tagged.png":
            extra = [(b"iCCP", b"synthetic-rgb\0\0" + zlib.compress(synthetic_icc(), 9))]
        (args.output / name).write_bytes(png(width, height, mode, pixels, extra))

    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        raise SystemExit("生成 EXIF JPEG 需要 ffmpeg；其余 PNG 已生成")
    jpeg_path = args.output / "exif-orient6.jpg"
    subprocess.run([ffmpeg, "-v", "error", "-y", "-i", str(args.output / "hard-edge-opaque.png"),
                    "-frames:v", "1", "-q:v", "2", "-pix_fmt", "yuvj444p", str(jpeg_path)], check=True)
    jpeg_path.write_bytes(add_exif_orientation(jpeg_path.read_bytes(), 6))
    source = (args.output / "hard-edge-opaque.png").read_bytes()
    (args.output / "corrupt-truncated.png").write_bytes(source[:48])

    manifest = {
        "schemaVersion": 1,
        "generator": "generate.py",
        "license": "CC0-1.0; generated for this repository",
        "samples": [
            {"id": name.rsplit(".", 1)[0], "file": f"images/{name}", "width": width, "height": height,
             "format": mode, "purpose": purpose, "sha256": hashlib.sha256((args.output / name).read_bytes()).hexdigest()}
            for name, (width, height, mode, purpose) in FIXTURES.items()
        ],
    }
    (args.output.parent / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")


if __name__ == "__main__":
    main()
