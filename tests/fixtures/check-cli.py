#!/usr/bin/env python3
"""Exercise the Rust PNG/JPEG import boundary against frozen files."""

import argparse
import json
import os
import subprocess
from pathlib import Path


EXPECTED = {
    "hard-edge-opaque.png": (96, 64, "png"),
    "wide-alpha.png": (160, 48, "png"),
    "portrait-strands.png": (48, 160, "png"),
    "low-contrast.png": (96, 96, "png"),
    "mask-edges.png": (96, 64, "png"),
    "gamma-tagged.png": (96, 64, "png"),
    "icc-tagged.png": (96, 64, "png"),
    "exif-orient6.jpg": (64, 96, "jpeg"),
    "corrupt-truncated.png": None,
    "oversize-header.png": None,
}


def main() -> None:
    parser = argparse.ArgumentParser()
    binary = "target/debug/cutout-cli.exe" if os.name == "nt" else "target/debug/cutout-cli"
    parser.add_argument("--cli", type=Path, default=Path(binary))
    args = parser.parse_args()
    root = Path(__file__).resolve().parent / "images"
    for name, expected in EXPECTED.items():
        result = subprocess.run([str(args.cli), "inspect", str(root / name)], capture_output=True, text=True)
        if (result.returncode == 0) != (expected is not None):
            raise SystemExit(f"{name}: exit {result.returncode}: {result.stderr}")
        if expected is not None:
            data = json.loads(result.stdout)
            actual = (data["width"], data["height"], data["format"])
            if actual != expected or data["pixelFormat"] != "rgba8-srgb-straight":
                raise SystemExit(f"{name}: expected {expected}, got {data}")
    print("PASS: 7 supported PNGs, oriented JPEG, and 2 expected import rejections")


if __name__ == "__main__":
    main()
