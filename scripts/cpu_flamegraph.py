#!/usr/bin/env python3
"""Capture macOS stack samples of DFlow and render an offline CPU flamegraph."""

import argparse
from collections import Counter
import hashlib
from pathlib import Path
import re
import subprocess
import sys
import time
from xml.sax.saxutils import escape


FRAME = re.compile(r"^([ +!:|]*?)(\d+)\s+(.+)$")
HASH = re.compile(r"::h[0-9a-f]{16}$")
WIDTH = 1200
ROW = 23
TOP = 75


def parse_sample(path: Path) -> tuple[list, int, int]:
    text = path.read_text(errors="replace")
    interval = re.search(r"every (\d+) milliseconds", text)
    if "Call graph:\n" not in text:
        raise ValueError("sample output has no call graph")
    if not interval:
        raise ValueError("sample output has no sampling interval")
    graph = text.split("Call graph:\n", 1)[1].split("\nTotal number in stack", 1)[0]
    roots: list = []
    parents: list = []
    for line in graph.splitlines():
        match = FRAME.match(line)
        if not match:
            continue
        column = len(match.group(1))
        if column < 4 or (column - 4) % 2:
            raise ValueError(f"unrecognized sample indentation: {line[:100]}")
        depth = (column - 4) // 2
        if depth > len(parents):
            raise ValueError(f"missing parent in sample call graph: {line[:100]}")
        node = [match.group(3), int(match.group(2)), []]
        del parents[depth:]
        (parents[-1][2] if parents else roots).append(node)
        parents.append(node)

    def find_workload(node: list) -> list | None:
        if "profile_workload::main::" in node[0]:
            return node
        candidates = [found for child in node[2] if (found := find_workload(child))]
        return max(candidates, key=lambda found: found[1], default=None)

    candidates = [found for root in roots if (found := find_workload(root))]
    if not candidates:
        raise ValueError("no profile_workload::main stack found; check binary symbols")
    workload = max(candidates, key=lambda found: found[1])
    return workload, workload[1], int(interval.group(1))


def label(frame: str) -> str:
    name = frame.split("  (in ", 1)[0]
    return HASH.sub("", name)


def folded_stacks(root: list) -> Counter:
    folded: Counter = Counter()

    def visit(node: list, parents: tuple[str, ...]) -> None:
        path = (*parents, label(node[0]))
        own = node[1] - sum(child[1] for child in node[2])
        if own < 0:
            raise ValueError(f"sample child count exceeds parent count at {path[-1]}")
        if own:
            folded[path] += own
        for child in node[2]:
            visit(child, path)

    visit(root, ())
    if sum(folded.values()) != root[1]:
        raise ValueError("collapsed sample count does not match workload count")
    return folded


def tree_from_folded(folded: Counter) -> dict:
    root = {"label": "DFlow workload", "count": 0, "children": {}}
    for stack, count in folded.items():
        node = root
        node["count"] += count
        for frame in stack[1:]:
            node = node["children"].setdefault(
                frame, {"label": frame, "count": 0, "children": {}}
            )
            node["count"] += count
    return root


def color(name: str) -> str:
    hue = int.from_bytes(hashlib.sha256(name.encode()).digest()[:2], "big") % 360
    return f"hsl({hue} 58% 69%)"


def render_svg(folded: Counter, interval_ms: int) -> str:
    root = tree_from_folded(folded)
    total = root["count"]
    depth = max(len(stack) for stack in folded)
    height = TOP + ROW * (depth + 1) + 35
    frames = []

    def visit(node: dict, x: float, level: int) -> None:
        width = WIDTH * node["count"] / total
        y = TOP + ROW * (depth - level)
        if width > 0:
            name = node["label"]
            percent = node["count"] / total * 100
            own = node["count"] - sum(child["count"] for child in node["children"].values())
            title = escape(
                f"{name} — inclusive {node['count']} samples ({percent:.2f}%); "
                f"self {own} samples ({own / total * 100:.2f}%)"
            )
            visible_chars = max(0, int((width - 8) / 6.7))
            text = escape(name[:visible_chars] + ("…" if len(name) > visible_chars and visible_chars > 1 else ""))
            frames.append(
                f'<g class="frame" data-x="{x:.3f}" data-w="{width:.3f}" onclick="zoom(event)">'
                f'<title>{title}</title>'
                f'<rect x="{x:.3f}" y="{y}" width="{width:.3f}" height="{ROW - 2}" '
                f'fill="{color(name)}" rx="2"/>'
                + (f'<text x="{x + 4:.3f}" y="{y + 15}">{text}</text>' if text else "")
                + "</g>"
            )
        child_x = x
        for child in sorted(node["children"].values(), key=lambda item: (-item["count"], item["label"])):
            visit(child, child_x, level + 1)
            child_x += WIDTH * child["count"] / total

    visit(root, 0.0, 0)
    script = """<![CDATA[
const svg = document.documentElement;
const frames = [...document.querySelectorAll('.frame')];
function zoom(event) {
  event.stopPropagation();
  const focus = event.currentTarget;
  const x = Number(focus.dataset.x), w = Number(focus.dataset.w);
  for (const frame of frames) {
    const fx = Number(frame.dataset.x), fw = Number(frame.dataset.w);
    const shown = fx >= x - 0.01 && fx + fw <= x + w + 0.01;
    frame.style.display = shown ? '' : 'none';
    if (!shown) continue;
    const rect = frame.querySelector('rect');
    rect.setAttribute('x', ((fx - x) * 1200 / w).toFixed(3));
    rect.setAttribute('width', (fw * 1200 / w).toFixed(3));
    const text = frame.querySelector('text');
    if (text) text.setAttribute('x', ((fx - x) * 1200 / w + 4).toFixed(3));
  }
}
function resetZoom() {
  for (const frame of frames) {
    frame.style.display = '';
    frame.querySelector('rect').setAttribute('x', frame.dataset.x);
    frame.querySelector('rect').setAttribute('width', frame.dataset.w);
    const text = frame.querySelector('text');
    if (text) text.setAttribute('x', (Number(frame.dataset.x) + 4).toFixed(3));
  }
}
]]>"""
    return f'''<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" viewBox="0 0 {WIDTH} {height}">
<style>svg{{font:12px system-ui,sans-serif;background:#f7f9fc;color:#142235}}text{{fill:#142235;pointer-events:none}}.frame{{cursor:pointer}}.frame:hover rect{{stroke:#172334;stroke-width:1.5}}.reset{{cursor:pointer;fill:#1255a0;text-decoration:underline}}</style>
<text x="12" y="27" font-size="20" font-weight="700">DFlow sampled CPU flamegraph</text>
<text x="12" y="49">{total} workload samples · {interval_ms} ms sampling interval · width = inclusive samples · click a frame to zoom</text>
<text class="reset" x="1080" y="49" onclick="resetZoom()">Reset zoom</text>
{''.join(frames)}
<script>{script}</script>
</svg>'''


def capture(binary: Path, repo: Path, iterations: int, depth: int, duration: int, interval: int, raw: Path) -> None:
    if sys.platform != "darwin":
        raise RuntimeError("automatic capture currently uses macOS sample; use --from-sample to render an existing capture")
    process = subprocess.Popen(
        [str(binary), str(repo), str(iterations), str(depth)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    try:
        time.sleep(0.2)
        result = subprocess.run(
            ["sample", str(process.pid), str(duration), str(interval), "-file", str(raw)],
            capture_output=True,
            text=True,
        )
        if result.returncode or not raw.is_file():
            raise RuntimeError(result.stderr.strip() or result.stdout.strip() or "sample failed")
        stderr = process.communicate()[1].decode(errors="replace")
        if process.returncode:
            raise RuntimeError(f"profile workload exited {process.returncode}: {stderr}")
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path("."))
    parser.add_argument("--binary", type=Path, default=Path("target/profiling/examples/profile_workload"))
    parser.add_argument("--output", type=Path, default=Path("docs/dflow-cpu-flamegraph.svg"))
    parser.add_argument("--iterations", type=int, default=50)
    parser.add_argument("--depth", type=int, default=2)
    parser.add_argument("--duration", type=int, default=5, help="sample duration in seconds")
    parser.add_argument("--interval", type=int, default=2, help="sampling interval in milliseconds")
    parser.add_argument("--from-sample", type=Path, help="render an existing macOS sample text file")
    args = parser.parse_args()
    if args.iterations < 1 or args.duration < 1 or args.interval < 1 or args.depth < 0:
        parser.error("iterations, duration and interval must be positive; depth must be nonnegative")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    raw = args.output.with_suffix(".sample.txt")
    if args.from_sample:
        raw.write_bytes(args.from_sample.read_bytes())
    else:
        binary = args.binary.resolve()
        if not binary.is_file():
            parser.error(f"binary not found: {binary}; build the profiling example first")
        capture(binary, args.repo.resolve(), args.iterations, args.depth, args.duration, args.interval, raw)

    root, samples, actual_interval = parse_sample(raw)
    folded = folded_stacks(root)
    folded_path = args.output.with_suffix(".folded.txt")
    folded_path.write_text(
        "\n".join(";".join(stack) + f" {count}" for stack, count in sorted(folded.items())) + "\n"
    )
    args.output.write_text(render_svg(folded, actual_interval))
    hot = Counter()
    for stack, count in folded.items():
        for frame in set(stack):
            if frame.startswith("dflow_core::"):
                hot[frame] += count
    print(f"Wrote {args.output}, {raw}, and {folded_path}; {samples} workload samples")
    for frame, count in hot.most_common(8):
        print(f"{count / samples:5.1%}  {frame}")
    self_hot = Counter()
    for stack, count in folded.items():
        self_hot[stack[-1]] += count
    print("Top self samples:")
    for frame, count in self_hot.most_common(8):
        print(f"{count / samples:5.1%}  {frame}")


if __name__ == "__main__":
    main()
