"""Compare one and two independent native batch processes on the same plan."""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import TextIO


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def runner_command(args: argparse.Namespace, plan: Path, output: Path) -> list[str]:
    return [
        sys.executable,
        str(Path(__file__).with_name("run-batch-benchmark.py")),
        "--cli",
        str(args.cli.resolve()),
        "--runtime",
        str(args.runtime.resolve()),
        "--models",
        str(args.models.resolve()),
        "--plan",
        str(plan.resolve()),
        "--output",
        str(output.resolve()),
    ]


def split_plan(plan_path: Path, plan_dir: Path) -> tuple[list[dict], list[dict]]:
    plan = json.loads(plan_path.read_text(encoding="utf-8"))
    if plan["schemaVersion"] != 1:
        raise ValueError("unsupported plan schema")
    halves: tuple[list[dict], list[dict]] = ([], [])
    totals = [0, 0]
    for case in plan["cases"]:
        repeats = int(case["repeat"])
        if repeats < 2:
            raise ValueError("each case needs at least two repeats")
        first = repeats // 2
        if repeats % 2 and totals[0] <= totals[1]:
            first += 1
        second = repeats - first
        paths = {
            "path": str((plan_path.parent / case["path"]).resolve()),
            "reference": str((plan_path.parent / case["reference"]).resolve())
            if case.get("reference")
            else None,
        }
        for index, count in enumerate((first, second)):
            halves[index].append({**paths, "repeat": count})
            totals[index] += count
    for index, cases in enumerate(halves, 1):
        shard = {
            "schemaVersion": 1,
            "description": f"two-process concurrency probe, shard {index}",
            "cases": cases,
        }
        (plan_dir / f"shard-{index}.json").write_text(
            json.dumps(shard, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
    return halves


def run_logged(command: list[str], log_path: Path) -> tuple[subprocess.Popen, TextIO]:
    log = log_path.open("w", encoding="utf-8")
    try:
        process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT)
    except BaseException:
        log.close()
        raise
    return process, log


def wait_logged(running: tuple[subprocess.Popen, TextIO], label: str) -> None:
    process, log = running
    code = process.wait()
    log.close()
    if code:
        raise RuntimeError(f"{label} failed with exit code {code}")


def system_load() -> list[float] | None:
    try:
        return list(os.getloadavg())
    except (AttributeError, OSError):
        return None


def check_outputs(cases: list[dict], halves: tuple[list[dict], list[dict]], root: Path) -> None:
    serial = root / "serial"
    for index, case in enumerate(cases):
        stem = Path(case["path"]).stem
        pattern = f"{stem}-cutout*.png"
        originals = sorted(serial.glob(pattern))
        if len(originals) != case["repeat"]:
            raise ValueError(f"serial output count mismatch: {stem}")
        expected = digest(originals[0])
        if any(digest(path) != expected for path in originals):
            raise ValueError(f"serial output differs within case: {stem}")
        for shard_index in range(2):
            outputs = sorted((root / f"parallel-{shard_index + 1}").glob(pattern))
            if len(outputs) != halves[shard_index][index]["repeat"]:
                raise ValueError(f"parallel output count mismatch: {stem}")
            if any(digest(path) != expected for path in outputs):
                raise ValueError(f"parallel output differs from serial: {stem}")


def main() -> int:
    parser = argparse.ArgumentParser()
    for name in ("cli", "runtime", "models", "plan", "output"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    parser.add_argument("--parallel-first", action="store_true")
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    plan_dir = root / "plans"
    plan_dir.mkdir()
    plan_path = args.plan.resolve()
    plan = json.loads(plan_path.read_text(encoding="utf-8"))
    halves = split_plan(plan_path, plan_dir)
    loads: dict[str, list[float] | None] = {}

    def run_serial() -> float:
        print("running serial baseline", flush=True)
        loads["serialBefore"] = system_load()
        started = time.perf_counter()
        serial = run_logged(runner_command(args, plan_path, root / "serial"), root / "serial.log")
        wait_logged(serial, "serial")
        wall = time.perf_counter() - started
        loads["serialAfter"] = system_load()
        return wall

    def run_parallel() -> float:
        print("running two native processes concurrently", flush=True)
        loads["parallelBefore"] = system_load()
        started = time.perf_counter()
        processes = [
            run_logged(
                runner_command(args, plan_dir / f"shard-{index}.json", root / f"parallel-{index}"),
                root / f"parallel-{index}.log",
            )
            for index in (1, 2)
        ]
        for index, process in enumerate(processes, 1):
            wait_logged(process, f"parallel-{index}")
        wall = time.perf_counter() - started
        loads["parallelAfter"] = system_load()
        return wall

    if args.parallel_first:
        parallel_wall = run_parallel()
        serial_wall = run_serial()
    else:
        serial_wall = run_serial()
        parallel_wall = run_parallel()

    check_outputs(plan["cases"], halves, root)
    serial_report = json.loads((root / "serial/report.json").read_text(encoding="utf-8"))
    parallel_reports = [
        json.loads((root / f"parallel-{index}/report.json").read_text(encoding="utf-8"))
        for index in (1, 2)
    ]
    total = sum(case["repeat"] for case in plan["cases"])
    if serial_report["succeeded"] != total or sum(
        report["succeeded"] for report in parallel_reports
    ) != total:
        raise ValueError("not all native inference items succeeded")
    report = {
        "schemaVersion": 1,
        "kind": "two-process-concurrency-feasibility-probe",
        "inputCount": total,
        "distinctInputCount": len(plan["cases"]),
        "platform": serial_report["platform"],
        "architecture": serial_report["architecture"],
        "availableParallelism": serial_report["availableParallelism"],
        "sourcePlanSha256": digest(plan_path),
        "modelSha256": serial_report["modelSha256"],
        "runtimeSha256": serial_report["runtimeSha256"],
        "outputBytesIdentical": True,
        "runOrder": "parallel-then-serial" if args.parallel_first else "serial-then-parallel",
        "systemLoadAverage": loads,
        "serialWallMs": round(serial_wall * 1000),
        "parallelWallMs": round(parallel_wall * 1000),
        "observedSpeedup": serial_wall / parallel_wall,
        "serialPeakRssBytes": serial_report.get("peakRssBytes"),
        "parallelIndividualPeakRssBytes": [
            report.get("peakRssBytes") for report in parallel_reports
        ],
        "parallelPeakRssUpperBoundBytes": sum(
            report.get("peakRssBytes", 0) for report in parallel_reports
        ),
        "serialCpuSeconds": serial_report.get("cpuUserSeconds", 0)
        + serial_report.get("cpuSystemSeconds", 0),
        "parallelIndividualCpuSeconds": [
            report.get("cpuUserSeconds", 0) + report.get("cpuSystemSeconds", 0)
            for report in parallel_reports
        ],
        "serialStageTimingUs": serial_report["stageTimingUs"],
        "parallelStageTimingUs": [report["stageTimingUs"] for report in parallel_reports],
    }
    path = root / "report.json"
    path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(path, flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
