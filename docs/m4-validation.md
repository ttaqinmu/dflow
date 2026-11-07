# M4 validation record

## Automated and terminal checks

On 2026-11-07, `cargo test` passed 66 tests, including M1–M3 regressions and Ratatui test-backend checks for directed arrows, change/snapshot labels, source and call evidence, cycle members, shared callers, empty and partial states, small terminals, depth changes, focus, selection, help, historical deletion context, and terminal control-character sanitization. `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` passed.

A Linux pseudoterminal at 120×30 exercised default TUI selection, first graph frame, help input, resize to 100×20, `q`, and alternate-screen/raw-mode restoration. A separate partial-analysis run showed `PARTIAL analysis`, accepted Ctrl-C, exited 1, and restored canonical input and the prior screen. Explicit TUI over pipes exits 2; default piped output remains JSON.

## Generated benchmark baseline

Reference environment: Intel Core i7-12700F, x86_64 Linux, Rust 1.93.1, Tree-sitter 0.25.10, tree-sitter-python 0.25.0, debug build. Each generated repository has one function per Python file, one modified root, a two-function cycle, and a caller chain reaching every function. Three local JSON runs used `--depth` equal to the file count. Times are median wall-clock seconds including process startup; RSS is the maximum child-process value reported by Linux `resource.getrusage`, so it is an upper-bound baseline rather than isolated DFlow memory.

| Files | Symbols across two snapshots | Edges across two snapshots | Focused impact nodes | Median elapsed | Max child RSS |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 20 | 40 | 40 | 20 | 0.0171 s | 18,012 KiB |
| 100 | 200 | 200 | 100 | 0.0403 s | 22,504 KiB |

The 100-file cyclic graph reached its first TUI graph frame in 0.0411 s in the pseudoterminal. Phase-specific capture, parse, resolution, traversal, and render timing is not isolated by the current binary; no performance budget is asserted from these first measurements.

## Task walkthroughs

The fixture checks completed three navigation tasks: a modified root showed its caller through an arrow and call-site evidence; an added root was selectable and displayed its target-snapshot callers; a deleted root displayed its base-snapshot caller and historical source. These are automated developer walkthroughs. Human task times and a manual-inspection comparison have not been measured, so no adoption or time-saving claim is made.

For a human comparison, use one repository fixture with all three change kinds. Ask each participant to identify (1) the modified function's two-step caller chain, (2) every caller of an added function, and (3) the deleted function's historical caller and source location. Run the tasks once with DFlow and once by manual code inspection, alternate the order across participants, and record correctness and elapsed seconds separately for each task and method. Keep those results out of the deterministic JSON report.
