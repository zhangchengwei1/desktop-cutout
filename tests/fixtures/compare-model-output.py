"""Compare native ORT and ORT Web output on the same frozen FP32 input."""

import argparse
import gzip
import json
import math
import struct
from pathlib import Path


def read_bytes(path: Path) -> bytes:
    return gzip.decompress(path.read_bytes()) if path.suffix == ".gz" else path.read_bytes()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("native_output", type=Path)
    parser.add_argument("web_output", type=Path)
    args = parser.parse_args()
    native = read_bytes(args.native_output)
    web = read_bytes(args.web_output)
    if len(native) != len(web) or len(native) != 1024 * 1024 * 4:
        raise ValueError("IS-Net output must contain 1024² FP32 values on both sides")
    differences = []
    for (a,), (b,) in zip(struct.iter_unpack("<f", native), struct.iter_unpack("<f", web)):
        if not math.isfinite(a) or not math.isfinite(b):
            raise ValueError("non-finite model output")
        differences.append(abs(a - b))
    differences.sort()
    print(json.dumps({
        "mae": sum(differences) / len(differences),
        "p99": differences[(len(differences) - 1) * 99 // 100],
        "max": differences[-1],
    }))


if __name__ == "__main__":
    main()
