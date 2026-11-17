# CPU profiling with Flamegraph

Use the standard [cargo-flamegraph](https://github.com/flamegraph-rs/flamegraph) tool to sample DFlow's call stacks. No profiling code or benchmark harness is required in DFlow.

Install the tool and profile a representative repository with changes:

```sh
cargo install flamegraph
CARGO_PROFILE_RELEASE_DEBUG=true cargo flamegraph --package dflow --bin dflow --no-default-features --output flamegraph.svg -- --dir /path/to/repository local --format json > /dev/null
```

Open `flamegraph.svg` in a browser. The command builds an optimized binary with debug symbols, runs DFlow's headless JSON analysis, and writes a sampled CPU flamegraph. The redirected JSON report is independent of the SVG. For a branch comparison, replace `local` with `branch --base BASE --target TARGET` using branch names that exist in the selected repository. Choose a workload that runs long enough to collect useful samples.

On Linux, `cargo-flamegraph` uses `perf`. On macOS, it uses Xcode's `xctrace`; Xcode Command Line Tools alone do not provide `xctrace`. See the tool's [platform setup](https://github.com/flamegraph-rs/flamegraph#installation) for prerequisites.
