#!/usr/bin/env python3
"""Verify frozen fixture bytes and browser Canvas reference artifacts."""

import argparse
import gzip
import hashlib
import json
import struct
import zlib
from pathlib import Path


ROOT = Path(__file__).resolve().parent
REFERENCE = ROOT / "reference"
CAPTURED = [
    "hard-edge-opaque.png", "wide-alpha.png", "portrait-strands.png",
    "low-contrast.png", "gamma-tagged.png", "icc-tagged.png", "exif-orient6.jpg",
]


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def png_rgba(path: Path) -> tuple[int, int, bytes]:
    raw = path.read_bytes()
    assert raw.startswith(b"\x89PNG\r\n\x1a\n"), path
    cursor = 8
    compressed = bytearray()
    width = height = color = depth = None
    while cursor < len(raw):
        length = struct.unpack_from(">I", raw, cursor)[0]
        kind = raw[cursor + 4:cursor + 8]
        payload = raw[cursor + 8:cursor + 8 + length]
        crc = struct.unpack_from(">I", raw, cursor + 8 + length)[0]
        assert zlib.crc32(kind + payload) == crc, path
        cursor += 12 + length
        if kind == b"IHDR":
            width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", payload)
            assert depth == 8 and color == 6 and interlace == 0, path
        elif kind == b"IDAT":
            compressed.extend(payload)
        elif kind == b"IEND":
            break
    assert width and height
    decoded = zlib.decompress(compressed)
    stride = width * 4
    out = bytearray()
    previous = bytes(stride)
    cursor = 0
    for _ in range(height):
        mode = decoded[cursor]
        current = bytearray(decoded[cursor + 1:cursor + 1 + stride])
        cursor += stride + 1
        for x in range(stride):
            left = current[x - 4] if x >= 4 else 0
            above = previous[x]
            upper_left = previous[x - 4] if x >= 4 else 0
            if mode == 1:
                prediction = left
            elif mode == 2:
                prediction = above
            elif mode == 3:
                prediction = (left + above) // 2
            elif mode == 4:
                p = left + above - upper_left
                distances = (abs(p - left), abs(p - above), abs(p - upper_left))
                prediction = (left, above, upper_left)[distances.index(min(distances))]
            else:
                assert mode == 0, path
                prediction = 0
            current[x] = (current[x] + prediction) & 255
        out.extend(current)
        previous = current
    assert cursor == len(decoded), path
    return width, height, bytes(out)


def verify_samples() -> None:
    manifest = json.loads((ROOT / "manifest.json").read_text())
    for item in manifest["samples"]:
        assert sha((ROOT / item["file"]).read_bytes()) == item["sha256"], item["file"]
    assert len(manifest["samples"]) == 10


def verify_references(reference: dict) -> None:
    samples = reference["samples"]
    assert [item["input"] for item in samples] == CAPTURED
    for item in samples:
        name = item["input"]
        tensor = gzip.decompress((REFERENCE / f"{name}.tensor.f32le.gz").read_bytes())
        mask = gzip.decompress((REFERENCE / f"{name}.mask.u8.gz").read_bytes())
        width, height, rgba = png_rgba(REFERENCE / f"{name}.synthetic.png")
        assert len(tensor) == 3 * 1024 * 1024 * 4 and sha(tensor) == item["tensorSha256"], name
        assert len(mask) == 1024 * 1024 and sha(mask) == item["restoredMaskSha256"], name
        assert (width, height) == (item["width"], item["height"]), name
        assert sha(rgba) == item["exportedPngRgbaSha256"], name


def verify_model() -> None:
    model_file = REFERENCE / "model-isnet.json"
    if not model_file.exists():
        return
    report = json.loads(model_file.read_text())
    snapshot = json.loads((ROOT / "model-snapshot.json").read_text())
    spec = next(item for item in snapshot["models"] if item["id"] == "isnet-auto-high")
    assert (report["modelId"], report["modelRevision"], report["modelSha256"]) == (
        spec["id"], spec["revision"], spec["sha256"])
    browser = json.loads((REFERENCE / "browser-canvas.json").read_text())
    assert report["browserSourceRevision"] == browser["sourceRevision"]
    assert [item["input"] for item in report["samples"]] == CAPTURED
    for sample, captured in zip(report["samples"], browser["samples"]):
        assert sample["inputTensorSha256"] == captured["tensorSha256"]
        assert sample["modelOutputShape"] == [1, 1, 1024, 1024]
        mask = gzip.decompress((REFERENCE / f"model-isnet/{sample['input']}.mask.u8.gz").read_bytes())
        assert len(mask) == 1024 * 1024 and sha(mask) == sample["restoredMaskSha256"]
        raw_output = ROOT.parents[1] / "test-output/fixtures/isnet-output" / f"{sample['input']}.output.f32le.gz"
        if raw_output.exists():
            values = gzip.decompress(raw_output.read_bytes())
            assert len(values) == 1024 * 1024 * 4 and sha(values) == sample["modelOutputSha256"]


def verify_model_render(report: dict) -> None:
    assert [item["input"] for item in report["samples"]] == CAPTURED
    for sample in report["samples"]:
        width, height, rgba = png_rgba(REFERENCE / f"model-isnet/{sample['input']}.model.png")
        assert (width, height) == (sample["width"], sample["height"])
        assert sha(rgba) == sample["exportedPngRgbaSha256"]


def verify_s04() -> None:
    directory = REFERENCE / "model-s04"
    if not directory.exists():
        return
    canvas = json.loads((directory / "canvas.json").read_text())
    model = json.loads((directory / "model.json").read_text())
    sample = canvas["sample"]
    assert model["browserSourceRevision"] == canvas["sourceRevision"]
    assert sample["original"] == {"width": 96, "height": 64}
    assert sample["samShape"] == [683, 1024, 3]
    assert sample["matteShape"] == [1, 4, 64, 96]
    assert sha((ROOT / "model-snapshot.json").read_bytes()) == model["modelSnapshotSha256"]
    for filename, length, expected in [
        ("sam-input.f32le.gz", 683 * 1024 * 3 * 4, sample["samInputSha256"]),
        ("matte-input.f32le.gz", 4 * 96 * 64 * 4, sample["matteInputSha256"]),
        ("trimap.u8.gz", 96 * 64, sample["trimapSha256"]),
        ("sam-mask.u8.gz", 96 * 64, model["samMaskSha256"]),
        ("matte-mask.u8.gz", 96 * 64, model["matteMaskSha256"]),
    ]:
        raw = gzip.decompress((directory / filename).read_bytes())
        assert len(raw) == length and sha(raw) == expected, filename
    assert model["samInputSha256"] == sample["samInputSha256"]
    assert model["matteInputSha256"] == sample["matteInputSha256"]
    assert model["trimapSha256"] == sample["trimapSha256"]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--assemble", action="store_true", help="merge the three synthetic Playwright captures")
    parser.add_argument("--assemble-model-render", action="store_true", help="freeze browser model-mask PNG references")
    args = parser.parse_args()
    verify_samples()
    if args.assemble:
        parts = [json.loads((REFERENCE / f"browser-canvas-part-{part}.json").read_text()) for part in "abc"]
        assert [(part["start"], part["end"]) for part in parts] == [(0, 3), (3, 5), (5, 7)]
        assert len({part["userAgent"] for part in parts}) == 1
        assert len({part["sourceRevision"] for part in parts}) == 1
        reference = {
            "schemaVersion": 1,
            "kind": "browser-canvas-synthetic-mask; no ONNX model was run",
            "sourceRevision": parts[0]["sourceRevision"],
            "userAgent": parts[0]["userAgent"],
            "samples": [item for part in parts for item in part["samples"]],
        }
        for item in reference["samples"]:
            _, _, rgba = png_rgba(REFERENCE / f"{item['input']}.synthetic.png")
            item["exportedPngRgbaSha256"] = sha(rgba)
        verify_references(reference)
        (REFERENCE / "browser-canvas.json").write_text(json.dumps(reference, ensure_ascii=False, indent=2) + "\n")
    elif not args.assemble_model_render:
        verify_references(json.loads((REFERENCE / "browser-canvas.json").read_text()))
    if not args.assemble:
        verify_model()
    if args.assemble_model_render:
        report = json.loads((REFERENCE / "model-isnet/browser-render-part.json").read_text())
        for sample in report["samples"]:
            _, _, rgba = png_rgba(REFERENCE / f"model-isnet/{sample['input']}.model.png")
            sample["exportedPngRgbaSha256"] = sha(rgba)
        verify_model_render(report)
        (REFERENCE / "model-isnet/browser-render.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    elif (REFERENCE / "model-isnet/browser-render.json").exists():
        verify_model_render(json.loads((REFERENCE / "model-isnet/browser-render.json").read_text()))
    verify_s04()
    print(f"PASS: 10 inputs, {len(CAPTURED)} Canvas references, model checks where present")


if __name__ == "__main__":
    main()
