# DFlow implementation plans

Status: Planned; no implementation is claimed.  
Source of truth: [PRD](PRD.md). All plans are in English.

## Execution order

Implement milestones sequentially. Each milestone plan is self-contained about its deliverable, interfaces, failure handling, and acceptance, and names its prerequisites. Together they cover the entire PRD; later plans extend the accepted behavior rather than reimplementing earlier milestones.

| Order | Plan | Outcome |
| --- | --- | --- |
| M1 | [Project and Git snapshots](01-project-and-git-snapshots.md) | Root-level single binary crate and all Git comparisons |
| M2 | [Python extraction and changes](02-python-symbols-and-changes.md) | Python syntax, symbols, call sites, imports, and change classification |
| M3 | [Call graphs and reports](03-call-graphs-and-reports.md) | Conservative resolution, impact traversal, and stable JSON |
| M4 | [Interactive terminal and Python release](04-terminal-ui-and-python-mvp.md) | Complete Python MVP, terminal graph, and release validation |
| M5 | [Rust analysis](05-rust-analysis.md) | Second analysis language with the same Git/report/TUI behavior |
| M6 | [Mermaid and Zed](06-mermaid-and-zed.md) | Diagram export and an editor integration prototype |

The first release ends at M4. Start Rust work only after Python acceptance passes. Zed follows CLI stability. These files plan work; creating them does not authorize publishing packages or installing an extension into a user's editor.

## Shared implementation rules

- Use one Rust 2024 binary crate with root `Cargo.toml` and `Cargo.lock`; keep analysis and presentation in internal modules. Do not create a workspace or initial library crate.
- Use `git2` for production Git access, Tree-sitter plus language grammars for parsing, and `petgraph` for directed graphs. Use Ratatui/Crossterm for the terminal.
- Treat source snapshots as immutable captured inputs. Never check out branches, write the index or objects, execute repository programs, or fetch dependencies from an analyzed project.
- Keep arrows caller → callee. Traverse incoming edges for callers. Deleted roots use base; added and modified roots use target.
- Preserve unresolved call evidence without speculative resolved edges. Report parse failures as incomplete, not empty successful analysis.
- Keep JSON deterministic and terminal-free. Default to TUI only when both stdin and stdout are terminals; explicit TUI otherwise fails.
- Every milestone updates affected documentation and retains earlier acceptance behavior. Do not advertise future features as implemented.

## PRD coverage

| Requirement or behavior | Implementation owner | Final validation |
| --- | --- | --- |
| FR01: all local and branch comparisons | M1 | M4, M5 |
| FR02: changed Python files and historical contents | M1 | M2, M4 |
| FR03: complete source context, unchanged callers | M1, M2, M3 | M4 |
| FR04: Python functions, async functions, methods, scopes, locations | M2 | M4 |
| FR05: identity and added/modified/deleted classification | M2 | M4 |
| FR06: separate directed graphs with evidence | M3 | M4 |
| FR07: resolved/unresolved status and reasons | M2, M3 | M4 |
| FR08: depth, callers, cycles | M3 | M4 |
| FR09: historical deletion context | M1, M3 | M4, M6 |
| FR10: versioned deterministic JSON | M3 | M4, M5, M6 |
| FR11: actionable diagnostics and incomplete analysis | M1–M3 | M4 |
| FR12: empty JSON/TUI result | M3, M4 | M4 |
| FR13: interactive graph and terminal lifecycle | M4 | M4, M5 |
| FR14: repository integrity | M1 and every consumer | Every milestone |
| Root-level single crate and required dependencies | M1 | M4 |
| Non-ignored untracked files, unborn HEAD, conflicts, dirty branches | M1 | M4 |
| Source discovery, package roots, symlinks, submodules | M1, M2 | M4 |
| Report locations, content identity, schema compatibility | M1–M3 | M4, M6 |
| Supported Python syntax and conservative resolution | M2, M3 | M4 |
| Supported Rust syntax and conservative resolution | M5 | M5 |
| Mermaid escaping and snapshot labels | M6 | M6 |
| Zed invocation, report consumption, current/historical locations | M6 | M6 |
| Accuracy, reproducibility, performance, usability evaluation | M3, M4 | M5, M6 regressions |

## Validation conventions

Use Rust integration tests with temporary repositories populated through `git2`. Test the binary through `CARGO_BIN_EXE_dflow`; keep focused internal tests inside binary modules. Do not add a library target just to expose internals to tests.

Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` at each implemented milestone. In this workspace, prefix shell commands with `rtk` as required by the repository instructions. Fixture expectations must encode independently known symbols, targets, and evidence, not copy the analyzer's own output as the sole oracle.

The coverage table assigns work; it does not waive any acceptance scenario from the PRD. Check off milestone completion only after its tests and inherited regressions pass. Benchmark measurements are recorded separately from deterministic reports. Keep advanced rename matching, outgoing callee exploration, and before/after relationship comparison for modified roots as later backlog items, not invented MVP gates.
