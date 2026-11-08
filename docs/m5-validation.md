# M5 Rust validation record

## Coverage and constraints

Rust analysis uses captured repository bytes, `tree-sitter-rust` 0.24.2 with Tree-sitter 0.25.10, and the same Git comparison, change classification, graph, JSON v1, and TUI as Python. Fixtures exercise editions 2018, 2021, and 2024 as parsed source syntax; this is not a compiler or an assertion about all edition-specific semantics. No analyzed project is built, run, or fetched. Literal root/workspace-member manifests, conventional or explicit lib/bin roots, inline/external/`#[path]` modules, shared files under distinct targets, and conditional module descendants have unit/discovery checks. Invalid encoding, syntax errors, missing/ambiguous/escaping modules or manifests, and uncovered Rust sources produce partial reports. Unknown calls alone do not.

`cargo test` passed 93 tests across four suites. `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` passed. `cargo build` followed by `python3 scripts/validate_tui_pty.py` exercised Python and Rust graph output in a real PTY, with terminal restoration. The Rust binary acceptance tests exercise combined/staged/unstaged local snapshots, branch tips, changed/added/deleted/untracked source, manifest changes, historical deleted callers, mixed-language names, conditional aliases and type definitions, grouped imports, opaque macros, referential integrity, deterministic JSON, complete/partial exits, and unchanged repository index/refs/status/source bytes. The crate-root `cargo run -- --dir . local --format json` produced one parseable object with `comparison.mode=local` and Rust symbols; `complete=false` is expected here because this repository's test and fixture `.rs` files are not Cargo target/module sources.

## Generated benchmark

Run `python3 scripts/benchmark_m5.py` after building the debug binary. Machine: Intel Core i7-12700F, x86_64 Linux 6.18.55-1-MANJARO (glibc 2.44), Rust 1.93.1. Each generated repository contains one Rust function per source file, one modified root, a two-function cycle and full caller chain. Three debug-binary local JSON runs per size use depth equal to file count; Linux `wait4` measures per-child peak RSS. Times are medians including process startup. The fixture oracle checks every reported resolved caller→callee pair (precision) and counts known resolvable pairs found (coverage), across both snapshots.

| Rust files | Symbols | Resolved edges | Impact nodes | Oracle pairs found | Precision | Coverage | Median elapsed | Max child RSS |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 20 | 40 | 40 | 20 | 40/40 | 100% | 100% | 0.0185 s | 19,120 KiB |
| 100 | 200 | 200 | 100 | 200/200 | 100% | 100% | 0.0491 s | 23,720 KiB |

The documented [M4 Python baseline](m4-validation.md) on the same CPU/toolchain measured 0.0121 s and 18,072 KiB at 20 files, and 0.0394 s and 22,560 KiB at 100 files. These are generated, language-specific fixture comparisons, not performance budgets or general workload predictions.

## Static limits and release status

Only certain intra-target lexical/module/import and unique inherent associated-function calls become edges. Receiver methods, trait/UFCS dispatch, external crates, globs/re-exports, dynamic expressions, macro expansion, unknown procedural attributes, and unknown conditional compilation remain unresolved evidence; macro token trees are not interpreted. Cargo workspace globs, build scripts, proc-macro generated definitions, symlinks, dependencies, and compiler/type inference are not followed. A syntactically valid but unsupported layout is partial, not a guessed graph. Python M4's human comparative usability study remains unperformed; M5's technical checks do not close that release gate.
