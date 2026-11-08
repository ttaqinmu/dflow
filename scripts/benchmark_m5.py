#!/usr/bin/env python3
"""Reproducible M5 Rust static caller-graph benchmark (Linux)."""

import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import tempfile
import time


def git(root: Path, *args: str) -> None:
    subprocess.run(["git", *args], cwd=root, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def fixture(root: Path, count: int) -> None:
    source = root / "src"
    source.mkdir()
    (root / "Cargo.toml").write_text('[package]\nname = "bench-rust"\nversion = "0.1.0"\nedition = "2021"\n')
    modules = "".join(f"mod m{i};\n" for i in range(1, count))
    (source / "lib.rs").write_text(modules + "pub fn f0() -> usize { m1::f1() }\n")
    for index in range(1, count):
        predecessor = 0 if index == 1 else index - 1
        path = "crate::f0" if predecessor == 0 else f"crate::m{predecessor}::f{predecessor}"
        (source / f"m{index}.rs").write_text(f"pub fn f{index}() -> usize {{ {path}() }}\n")
    git(root, "init")
    git(root, "config", "user.name", "Fixture")
    git(root, "config", "user.email", "fixture@example.com")
    git(root, "add", ".")
    git(root, "commit", "-m", "base")
    (source / "lib.rs").write_text(modules + "pub fn f0() -> usize { 1 + m1::f1() }\n")


def measure(binary: Path, root: Path, count: int) -> tuple[float, int, dict, int, int]:
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        start = time.perf_counter()
        process = subprocess.Popen([str(binary), "local", "--format", "json", "--depth", str(count)], cwd=root, stdout=stdout, stderr=stderr)
        _, status, usage = os.wait4(process.pid, 0)
        elapsed = time.perf_counter() - start
        process.returncode = os.waitstatus_to_exitcode(status)
        stderr.seek(0)
        assert process.returncode == 0, stderr.read().decode(errors="replace")
        stdout.seek(0)
        report = json.load(stdout)
    assert report["complete"] is True
    assert len(report["symbols"]) == count * 2
    assert len(report["impacts"]) == 1
    symbols = {entry["id"]: entry for entry in report["symbols"]}
    actual = set()
    for call in report["call_sites"]:
        if call["target"] is None:
            continue
        caller, callee = symbols[call["caller"]], symbols[call["target"]]
        assert caller["snapshot"] == callee["snapshot"] == call["snapshot"]
        actual.add((call["snapshot"], caller["name"], callee["name"]))
    oracle = {(role, f"f{i}", f"f{0 if i == 1 else i - 1}" if i else "f1") for role in ("base", "target") for i in range(count)}
    assert actual <= oracle, f"false resolved edges: {actual - oracle}"
    assert len(report["edges"]) == len(actual)
    return elapsed, usage.ru_maxrss, report, len(actual), len(oracle)


def main() -> None:
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/dflow").resolve()
    if not binary.is_file():
        raise SystemExit(f"binary not found: {binary}; run cargo build first")
    cpu = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), "unknown")
    print(f"CPU: {cpu}; platform: {platform.platform()}; rustc: {subprocess.check_output(['rustc', '--version'], text=True).strip()}")
    for count in (20, 100):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture(root, count)
            samples = [measure(binary, root, count) for _ in range(3)]
            median = statistics.median(sample[0] for sample in samples)
            rss = max(sample[1] for sample in samples)
            report, known, oracle = samples[-1][2:]
            print(f"files={count} symbols={len(report['symbols'])} edges={len(report['edges'])} impact_nodes={len(report['impacts'][0]['nodes'])} known_edges={known}/{oracle} precision=100% coverage={known / oracle:.1%} median_seconds={median:.4f} peak_rss_kib={rss}")


if __name__ == "__main__":
    main()
