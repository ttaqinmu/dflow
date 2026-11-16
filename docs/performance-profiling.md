# Performance profiling

DFlow can record its headless JSON pipeline without changing the versioned analysis report. Build an optimized headless binary and run the benchmark suite:

```sh
cargo build --release -p dflow --no-default-features
python3 scripts/profile_dflow.py --output docs/performance-profile.html --repo .
```

The runner uses the existing generated Python and Rust caller-chain fixtures at 20 and 100 files, plus the optional `--repo` worktree. It runs two warmups and ten measured processes per workload. `--repo` defaults to a local comparison; for named branch tips, add `--base BRANCH --target BRANCH`. `--samples`, `--warmups`, `--depth`, and `--binary` are configurable. It writes a self-contained [visual report](performance-profile.html) and [raw measurements](performance-profile.json).

For a single comparison, pass `--profile-json PATH` to `dflow local` or `dflow branch`. It selects JSON output when no format is specified, writes stage timings to the given sidecar file, and keeps normal stdout report bytes unchanged. `--profile-json` cannot be combined with interactive rendering or a non-JSON format. A partial analysis still writes a profile and exits 1; invalid input exits 2 without a profile.

## Baseline on this worktree

Measured on 2026-11-17 (Asia/Jakarta), macOS arm64, Rust 1.95.0, using the release headless binary. Wall time includes process startup and shutdown; peak RSS comes from per-process `wait4`. Values are medians across ten runs except peak RSS, which is the largest sample. The current worktree comparison includes the profiling implementation and is a snapshot-specific observation.

| Workload | Median total | Slowest measured stage | Stage time | Max peak RSS |
| --- | ---: | --- | ---: | ---: |
| Python, 20 source files | 6.647 ms | Git capture | 1.712 ms | 11.5 MiB |
| Python, 100 source files | 13.266 ms | Git capture | 4.396 ms | 16.5 MiB |
| Rust, 20 source files | 6.907 ms | Git capture | 1.789 ms | 11.9 MiB |
| Rust, 100 source files | 23.717 ms | Git capture | 7.138 ms | 17.0 MiB |
| DFlow worktree, local | 239.330 ms | Target extraction | 100.711 ms | 95.2 MiB |

For the DFlow worktree, base extraction took another 97.256 ms. Together, base and target extraction account for about 83% of the median total, so that is the first area to investigate on this workload. Extraction includes source discovery and parsing; this profile does not yet split those operations. Generated fixtures show Git capture as the largest measured stage, at roughly 26–33% of total time. No performance threshold is inferred from these initial measurements.

The chart's “Startup / unmeasured” segment is external process wall time minus the measured stages per run. Stage medians and total median need not add exactly. JSON sidecar writing itself falls into that unmeasured segment. The benchmark runs DFlow repeatedly on captured source; it does not build or execute the analyzed repository.

## Sampled flamegraph

The [flamegraph](dflow-cpu-flamegraph.svg) is a function-level stack sample of the DFlow worktree workload. Hover over a frame for its inclusive sample count; click a frame to zoom, and use “Reset zoom” to return. The [original macOS sample capture](dflow-cpu-flamegraph.sample.txt) and [folded stacks](dflow-cpu-flamegraph.folded.txt) are included so the result can be inspected or rendered with other flamegraph tools.

Reproduce it on macOS with Apple’s `sample` profiler:

```sh
cargo build -p dflow --example profile_workload --profile profiling --no-default-features
python3 scripts/cpu_flamegraph.py --repo . --iterations 50 --duration 5 --interval 2
```

The custom `profiling` Cargo profile keeps release optimizations and adds debug symbols. The example repeats Git capture, analysis, report construction, and JSON serialization in one process, allowing a sampler to collect enough stacks. Its output is discarded through `black_box`; analyzed code is never executed. macOS may require Developer Tools access for `sample`. To regenerate only the SVG and folded stacks from an existing capture, use `--from-sample PATH`. Automatic sampling in this script is macOS-specific.

This capture contains **1,919 workload samples** over a five-second window at a nominal two-millisecond interval. `AnalysisSession::build` appears in all of them and `extract_snapshot` in 98.4%. Within extraction, Rust syntax walking (`Walker::visit`, `visit_children`, and `visit_item`) appears in 33.6%; recursive `Walker::canonical` appears in 27.4%; Rust module discovery and `Finder::targets` appear in 24.6%. These are inclusive stack shares and overlap. The largest self-sample sites are Tree-sitter parsing (`ts_parser_parse`, 11.3%), SHA-256 compression (8.8%), and Tree-sitter cursor iteration (7.2%). The sampled stacks identify Rust extraction as the concrete hot path, complementing the end-to-end stage timing above. Sampling changes the workload’s timing; use the unprofiled benchmark for latency comparisons.
