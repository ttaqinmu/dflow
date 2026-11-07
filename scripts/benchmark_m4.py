#!/usr/bin/env python3
"""Reproducible M4 Python caller-graph benchmark (Linux)."""

import json
import os
from pathlib import Path
import platform
import resource
import statistics
import subprocess
import sys
import tempfile
import time


def git(root: Path, *args: str) -> None:
    subprocess.run(
        ["git", *args], cwd=root, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )


def fixture(root: Path, count: int) -> None:
    for index in range(count):
        predecessor = 1 if index == 0 else index - 1
        (root / f"m{index}.py").write_text(
            f"from m{predecessor} import f{predecessor}\n"
            f"def f{index}(): return f{predecessor}()\n"
        )
    git(root, "init")
    git(root, "config", "user.name", "Fixture")
    git(root, "config", "user.email", "fixture@example.com")
    git(root, "add", ".")
    git(root, "commit", "-m", "base")
    (root / "m0.py").write_text("from m1 import f1\ndef f0(): return 1 + f1()\n")


def measure(binary: Path, root: Path, count: int) -> tuple[float, int, dict]:
    # wait4 returns resource usage for this process, unlike cumulative RUSAGE_CHILDREN.
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        start = time.perf_counter()
        process = subprocess.Popen(
            [str(binary), "local", "--format", "json", "--depth", str(count)],
            cwd=root, stdout=stdout, stderr=stderr,
        )
        _, status, usage = os.wait4(process.pid, 0)
        elapsed = time.perf_counter() - start
        process.returncode = os.waitstatus_to_exitcode(status)
        stderr.seek(0)
        assert process.returncode == 0, stderr.read().decode(errors="replace")
        stdout.seek(0)
        report = json.load(stdout)
    assert report["complete"] is True
    assert len(report["symbols"]) == count * 2
    assert len(report["edges"]) == count * 2
    assert len(report["impacts"][0]["nodes"]) == count
    return elapsed, usage.ru_maxrss, report


def main() -> None:
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/dflow").resolve()
    if not binary.is_file():
        raise SystemExit(f"binary not found: {binary}; run cargo build first")
    cpu = next(
        (line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")),
        "unknown",
    )
    print(f"CPU: {cpu}; platform: {platform.platform()}; rustc: {subprocess.check_output(['rustc', '--version'], text=True).strip()}")
    for count in (20, 100):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture(root, count)
            samples = [measure(binary, root, count) for _ in range(3)]
            elapsed = statistics.median(sample[0] for sample in samples)
            peak = max(sample[1] for sample in samples)
            report = samples[-1][2]
            print(
                f"files={count} symbols={len(report['symbols'])} edges={len(report['edges'])} "
                f"impact_nodes={len(report['impacts'][0]['nodes'])} "
                f"median_seconds={elapsed:.4f} peak_rss_kib={peak}"
            )


if __name__ == "__main__":
    main()
