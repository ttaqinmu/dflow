# DFlow

DFlow is an experimental Rust tool for exploring how changes in a Git repository affect a Python project's function call graph.

The goal is to answer questions such as: *Which functions call the function I changed? What calls a newly added function? Who called a function before it was deleted?* DFlow will turn those relationships into a diagram that can be used from a standalone CLI and, eventually, from Zed.

> **Status:** M1 Git snapshot foundation. The root-level Rust binary crate can capture local (combined, staged, or unstaged) and branch-tip snapshots through `git2`. Python extraction, call graphs, JSON reports, and terminal diagrams are not implemented yet. The `analyze` command currently exits with code 1 after capture to avoid presenting a placeholder analysis as success. See [PRD and implementation plans](plan/README.md).

## Intended workflow

1. Read two repository snapshots, such as a base commit and a target commit or working tree.
2. Parse Python files in both snapshots and identify functions, methods, imports, and call sites.
3. Match symbols across snapshots and classify them as added, modified, or deleted.
4. Resolve calls where possible and build a call graph for each snapshot.
5. Trace callers and callees around changed symbols. Deleted symbols use the graph from the earlier snapshot.
6. Produce a structured report that presentation layers can render as JSON, Mermaid, or an editor view.

An intended diagram might look like this:

```mermaid
flowchart LR
    entry["entry"] --> process["process · modified"]
    entry --> notify["notify · added"]
    legacy["legacy caller"] --> removed["removed · deleted (before)"]
```

An arrow means **calls**: `entry → process` means `entry` calls `process`. To inspect the impact of a changed function, DFlow follows incoming arrows toward its callers.

## Architecture

The core will keep analysis separate from presentation:

```mermaid
flowchart TB
    snapshots["Git snapshots"] --> symbols["Python symbols"]
    symbols --> calls["Call resolution"]
    snapshots --> changes["Change detection"]
    calls --> report["Change and impact report"]
    changes --> report
    report --> json["JSON / CLI"]
    report --> diagram["Mermaid"]
    report --> zed["Zed integration"]
```

The report should carry symbol identity, source location, change type, call sites, and whether a call target was resolved. Python allows dynamic calls, so an unresolved target must remain explicitly unresolved rather than appear as a certain edge.

This is a conceptual boundary, not the current crate layout. The first implementation can stay in one Rust crate; modules or crates can be split out as the interfaces become clear.

## Roadmap

- [ ] Compare two Git snapshots and list changed Python files.
- [ ] Extract Python functions and their source ranges.
- [ ] Identify added, modified, and deleted symbols across snapshots.
- [ ] Build a call graph for direct, statically resolvable calls.
- [ ] Trace callers of changed symbols, including deleted symbols in the earlier snapshot.
- [ ] Expose a stable JSON report through the CLI.
- [ ] Render focused Mermaid diagrams from the report.
- [ ] Prototype a Zed workflow using the same core analysis.

Initial Python support will focus on local calls and explicit imports. Method dispatch, dynamic imports, decorators, and runtime-generated calls require additional analysis and may remain unresolved.

## Development

Install a Rust toolchain that supports the 2024 edition, then run:

```sh
cargo run -- local
cargo run -- local --staged
cargo run -- local --unstaged
cargo run -- branch --base main --target feature
```

Run the current test suite with:

```sh
cargo test
```

The single root-level crate uses `git2` for Git snapshots. Tree-sitter, `petgraph`, Ratatui, and Crossterm are declared for the planned parser, graph, and terminal stages.

### Testing approach

Small Python fixtures will define known before-and-after states. Integration tests can create temporary Git repositories, commit each state, run the analyzer, and compare its JSON report with the expected symbols and call edges. This will cover commits first, then staged and working-tree changes, renamed files, and unresolved calls.
