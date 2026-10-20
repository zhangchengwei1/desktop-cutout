"""Run the native batch CLI and attach peak child RSS when available."""

import argparse
import json
import subprocess
import sys
import zipfile
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--models", type=Path, required=True)
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    zip_mode = parser.add_mutually_exclusive_group()
    zip_mode.add_argument("--zip", action="store_true")
    zip_mode.add_argument("--zip-during", action="store_true")
    args = parser.parse_args()
    command = [
        str(args.cli.resolve()),
        "batch-bench",
        str(args.runtime.resolve()),
        str(args.models.resolve()),
        str(args.plan.resolve()),
        str(args.output.resolve()),
    ]
    if args.zip:
        command.append("--zip")
    elif args.zip_during:
        command.append("--zip-during")
    result = subprocess.run(command, check=False)
    report_path = args.output / "report.json"
    if report_path.is_file():
        report = json.loads(report_path.read_text(encoding="utf-8"))
        if sys.platform in {"darwin", "linux"}:
            import resource

            usage = resource.getrusage(resource.RUSAGE_CHILDREN)
            report["peakRssBytes"] = (
                usage.ru_maxrss if sys.platform == "darwin" else usage.ru_maxrss * 1024
            )
            report["peakRssSource"] = "RUSAGE_CHILDREN.ru_maxrss"
            report["cpuUserSeconds"] = usage.ru_utime
            report["cpuSystemSeconds"] = usage.ru_stime
        if (args.zip or args.zip_during) and report.get("zip"):
            pngs = {path.name: path for path in args.output.glob("*.png")}
            with zipfile.ZipFile(args.output / "batch-results.zip") as archive:
                names = archive.namelist()
                if len(names) != len(set(names)) or set(names) != set(pngs):
                    raise ValueError("ZIP entries differ from committed PNG outputs")
                for name in names:
                    with archive.open(name) as archived, pngs[name].open("rb") as output:
                        while True:
                            left = archived.read(1024 * 1024)
                            right = output.read(1024 * 1024)
                            if left != right:
                                raise ValueError(f"ZIP entry differs from PNG: {name}")
                            if not left:
                                break
            report["zipVerified"] = True
            report["zipEntryCount"] = len(names)
        report_path.write_text(
            json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
