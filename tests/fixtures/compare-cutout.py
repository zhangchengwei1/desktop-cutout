"""Compare a native PNG with a frozen browser PNG in decoded RGBA space."""

import argparse
import json
from pathlib import Path

from verify import png_rgba


def percentile(values: list[int], fraction: float) -> int:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int((len(ordered) - 1) * fraction))]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("native_png", type=Path)
    parser.add_argument("browser_png", type=Path)
    args = parser.parse_args()
    nw, nh, native = png_rgba(args.native_png)
    bw, bh, browser = png_rgba(args.browser_png)
    if (nw, nh) != (bw, bh):
        raise ValueError(f"dimension mismatch: native {(nw, nh)}, browser {(bw, bh)}")
    alpha = [abs(native[index] - browser[index]) for index in range(3, len(native), 4)]
    print(json.dumps({
        "width": nw,
        "height": nh,
        "alphaMae": sum(alpha) / len(alpha),
        "alphaP99": percentile(alpha, 0.99),
        "alphaMax": max(alpha),
        "alphaPixelsChanged": sum(value != 0 for value in alpha),
    }, ensure_ascii=False))


if __name__ == "__main__":
    main()
