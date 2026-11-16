#!/usr/bin/env python3
"""Benchmark DFlow's headless JSON pipeline and render an offline HTML report."""

import argparse
from contextlib import ExitStack
from datetime import datetime, timezone
import html
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

sys.dont_write_bytecode = True

import benchmark_m4
import benchmark_m5


STAGES = [
    ("git_capture", "Git capture"),
    ("extract_base", "Extract base"),
    ("extract_target", "Extract target"),
    ("classify_symbols", "Classify symbols"),
    ("graph_base", "Build base graph"),
    ("graph_target", "Build target graph"),
    ("compare_edges", "Compare edges"),
    ("filter_changes", "Filter changes"),
    ("impact_traversal", "Caller traversal"),
    ("report_assembly", "Report assembly"),
    ("json_serialization", "JSON serialization"),
    ("output_write", "Output write"),
]
STAGE_NAMES = dict(STAGES)


def peak_rss_bytes(usage: resource.struct_rusage) -> int:
    # Darwin reports bytes; Linux reports KiB.
    return usage.ru_maxrss if sys.platform == "darwin" else usage.ru_maxrss * 1024


def sample(binary: Path, arguments: list[str], profile_path: Path) -> dict:
    profile_path.unlink(missing_ok=True)
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        started = time.perf_counter_ns()
        process = subprocess.Popen(
            [str(binary), *arguments, "--format", "json", "--profile-json", str(profile_path)],
            stdout=stdout,
            stderr=stderr,
        )
        _, status, usage = os.wait4(process.pid, 0)
        elapsed_ns = time.perf_counter_ns() - started
        exit_code = os.waitstatus_to_exitcode(status)
        stderr.seek(0)
        error_text = stderr.read().decode(errors="replace")
        if exit_code not in (0, 1) or not profile_path.is_file():
            raise RuntimeError(f"DFlow exited {exit_code}: {error_text}")

        stdout.seek(0)
        report = json.load(stdout)
        profile = json.loads(profile_path.read_text())

    if profile["schema_version"] != 1 or report["complete"] != profile["complete"]:
        raise RuntimeError("DFlow report and profile disagree")
    if profile["counts"]["symbols"] != len(report["symbols"]):
        raise RuntimeError("DFlow symbol counts disagree")
    stages = {stage["name"]: stage["duration_ns"] for stage in profile["stages"]}
    if set(stages) != set(STAGE_NAMES):
        raise RuntimeError(f"Unexpected profiling stages: {sorted(stages)}")
    return {
        "elapsed_ns": elapsed_ns,
        "peak_rss_bytes": peak_rss_bytes(usage),
        "stages_ns": stages,
        "counts": profile["counts"],
        "complete": profile["complete"],
        "exit_code": exit_code,
    }


def benchmark(binary: Path, arguments: list[str], warmups: int, repetitions: int) -> list[dict]:
    with tempfile.TemporaryDirectory() as temporary:
        profile_path = Path(temporary) / "profile.json"
        for _ in range(warmups):
            sample(binary, arguments, profile_path)
        return [sample(binary, arguments, profile_path) for _ in range(repetitions)]


def summarize(samples: list[dict]) -> dict:
    median = statistics.median
    stage_ns = {
        name: median(item["stages_ns"][name] for item in samples)
        for name, _ in STAGES
    }
    overhead = median(
        max(0, item["elapsed_ns"] - sum(item["stages_ns"].values()))
        for item in samples
    )
    return {
        "median_ns": median(item["elapsed_ns"] for item in samples),
        "min_ns": min(item["elapsed_ns"] for item in samples),
        "max_ns": max(item["elapsed_ns"] for item in samples),
        "peak_rss_bytes": max(item["peak_rss_bytes"] for item in samples),
        "stages_ns": stage_ns,
        "unmeasured_ns": overhead,
        "slowest_stage": max(stage_ns, key=stage_ns.get),
        "counts": samples[-1]["counts"],
        "complete": all(item["complete"] for item in samples),
    }


def milliseconds(nanoseconds: float) -> str:
    return f"{nanoseconds / 1_000_000:.3f} ms"


def render_html(data: dict) -> str:
    sections = []
    for workload in data["workloads"]:
        name = html.escape(workload["name"])
        summary = summarize(workload["samples"])
        total = summary["median_ns"]
        measured = summary["stages_ns"]
        parts = [(key, label, measured[key]) for key, label in STAGES]
        parts.append(("unmeasured", "Startup / unmeasured", summary["unmeasured_ns"]))
        bar_total = sum(value for _, _, value in parts) or 1
        bars = "".join(
            f'<span style="width:{value / bar_total * 100:.4f}%;background:hsl({index * 31 % 360} 62% 47%)" '
            f'title="{html.escape(label)}: {milliseconds(value)}"></span>'
            for index, (_, label, value) in enumerate(parts)
        )
        rows = "".join(
            f'<tr><td><span class="dot" style="background:hsl({index * 31 % 360} 62% 47%)"></span>'
            f'{html.escape(label)}</td><td>{milliseconds(value)}</td>'
            f'<td>{value / total * 100:.1f}%</td></tr>'
            for index, (_, label, value) in enumerate(parts)
        )
        counts = summary["counts"]
        slowest = html.escape(STAGE_NAMES[summary["slowest_stage"]])
        sections.append(
            f'<section><h2>{name}</h2>'
            f'<p class="lead">Slowest measured stage: <strong>{slowest}</strong> '
            f'({milliseconds(measured[summary["slowest_stage"]])}).</p>'
            f'<div class="metrics"><div><b>{milliseconds(total)}</b><small>median total</small></div>'
            f'<div><b>{milliseconds(summary["min_ns"])}–{milliseconds(summary["max_ns"])}</b><small>sample range</small></div>'
            f'<div><b>{summary["peak_rss_bytes"] / 1024 / 1024:.1f} MiB</b><small>max peak RSS</small></div>'
            f'<div><b>{counts["files"]} / {counts["symbols"]} / {counts["edges"]}</b><small>files / symbols / edges</small></div></div>'
            f'<div class="bar" role="img" aria-label="Stage time breakdown for {name}">{bars}</div>'
            f'<table><thead><tr><th>Stage</th><th>Median</th><th>Share of total</th></tr></thead>'
            f'<tbody>{rows}</tbody></table>'
            + ('<p class="partial">Partial analysis: interpret counts with care.</p>' if not summary["complete"] else '')
            + '</section>'
        )
    generated = html.escape(data["generated_at"])
    machine = html.escape(data["machine"])
    binary = html.escape(data["binary"])
    flamegraph = data.get("flamegraph")
    flamegraph_link = (
        f'<p><a href="{html.escape(flamegraph, quote=True)}">Open sampled flamegraph</a></p>'
        if flamegraph else ""
    )
    return f'''<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>DFlow performance profile</title>
<style>
:root{{font:16px system-ui,sans-serif;color:#172334;background:#f3f6fa}}
body{{max-width:1100px;margin:0 auto;padding:28px 20px 60px}}
h1{{font-size:2rem;margin-bottom:4px}}h2{{margin:0 0 8px}}
.meta{{color:#526174;line-height:1.6}}section{{background:white;border:1px solid #d9e1eb;border-radius:12px;padding:24px;margin:22px 0;box-shadow:0 3px 12px #1723340b}}
.lead{{margin:0 0 20px}}.metrics{{display:grid;grid-template-columns:repeat(auto-fit,minmax(170px,1fr));gap:12px;margin-bottom:22px}}
.metrics div{{background:#f3f6fa;border-radius:8px;padding:12px}}.metrics b,.metrics small{{display:block}}.metrics b{{font-size:1.1rem}}.metrics small{{color:#526174;margin-top:4px}}
.bar{{height:30px;display:flex;overflow:hidden;border-radius:6px;background:#dde5ed}}.bar span{{min-width:1px}}
table{{border-collapse:collapse;width:100%;margin-top:20px}}th,td{{padding:9px 8px;border-bottom:1px solid #e5eaf0;text-align:left}}th:nth-child(n+2),td:nth-child(n+2){{text-align:right}}
.dot{{display:inline-block;width:11px;height:11px;border-radius:2px;margin-right:9px}}.partial{{color:#8a4e00;font-weight:600}}
footer{{color:#526174;line-height:1.6}}@media(max-width:560px){{body{{padding:16px 10px}}section{{padding:16px}}table{{font-size:.88rem}}}}
</style></head><body>
<h1>DFlow performance profile</h1>
<p class="meta">Generated {generated}<br>Machine: {machine}<br>Binary: {binary}</p>
{flamegraph_link}
{''.join(sections)}
<footer>Each stage value is the median of repeated release-binary runs. Total time includes process startup and shutdown. Startup / unmeasured is the per-run difference between external wall time and measured stages. Peak RSS is the highest sample. Stage medians and total median may not add exactly.</footer>
</body></html>'''


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/dflow"))
    parser.add_argument("--output", type=Path, default=Path("profile.html"))
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--warmups", type=int, default=2)
    parser.add_argument("--repo", type=Path, help="also benchmark this existing repository")
    parser.add_argument("--base", help="base revision for --repo branch comparison")
    parser.add_argument("--target", help="target revision for --repo branch comparison")
    parser.add_argument("--depth", type=int, default=2, help="caller depth for --repo")
    args = parser.parse_args()
    if args.samples < 1 or args.warmups < 0 or args.depth < 0:
        parser.error("samples must be positive; warmups and depth must be nonnegative")
    if bool(args.base) != bool(args.target) or (args.base and not args.repo):
        parser.error("--base and --target must be supplied together with --repo")
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error(f"binary not found: {binary}; run cargo build --release --no-default-features")

    workloads = []
    with ExitStack() as stack:
        for language, fixture in (("Python", benchmark_m4.fixture), ("Rust", benchmark_m5.fixture)):
            for count in (20, 100):
                root = Path(stack.enter_context(tempfile.TemporaryDirectory()))
                fixture(root, count)
                name = f"{language} · {count} source files"
                print(f"Benchmarking {name}…", flush=True)
                runs = benchmark(binary, ["--dir", str(root), "local", "--depth", str(count)], args.warmups, args.samples)
                if not all(run["complete"] for run in runs):
                    raise RuntimeError(f"generated {name} fixture produced partial analysis")
                workloads.append({"name": name, "samples": runs})

        if args.repo:
            repo = args.repo.resolve()
            mode = ["branch", "--base", args.base, "--target", args.target] if args.base else ["local"]
            name = f"Repository · {repo.name} ({'branch' if args.base else 'local'})"
            print(f"Benchmarking {name}…", flush=True)
            runs = benchmark(binary, ["--dir", str(repo), *mode, "--depth", str(args.depth)], args.warmups, args.samples)
            workloads.append({"name": name, "samples": runs})

    data = {
        "schema_version": 1,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "machine": f"{platform.platform()} / {platform.machine()} / {platform.processor()}",
        "binary": str(binary),
        "samples_per_workload": args.samples,
        "warmups": args.warmups,
        "workloads": workloads,
    }
    flamegraph_path = args.output.with_name("dflow-cpu-flamegraph.svg")
    if flamegraph_path.is_file():
        data["flamegraph"] = flamegraph_path.name
    args.output.parent.mkdir(parents=True, exist_ok=True)
    raw_path = args.output.with_suffix(".json")
    raw_path.write_text(json.dumps(data, indent=2) + "\n")
    args.output.write_text(render_html(data))
    print(f"Wrote {args.output} and {raw_path}")
    for workload in workloads:
        summary = summarize(workload["samples"])
        print(f"{workload['name']}: {milliseconds(summary['median_ns'])} total; slowest measured stage "
              f"{STAGE_NAMES[summary['slowest_stage']]} {milliseconds(summary['stages_ns'][summary['slowest_stage']])}")


if __name__ == "__main__":
    main()
