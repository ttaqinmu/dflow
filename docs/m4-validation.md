# M4 validation record

## Automated and terminal checks

On 2026-11-08, `cargo test` passed 75 tests in three suites. The new binary acceptance suite covers combined/staged/unstaged local capture, staged additions and deletions, untracked and ignored sources, unborn HEAD, conflicts, diverged branch tips in a dirty worktree, changed and deleted symbols, unchanged callers, aliases, cycles, uncertain calls, classification boundaries, renames, partial/empty results, invalid input, and deterministic JSON. It checks source bytes, index bytes, refs, and Git status before and after success and failure. Existing Ratatui test-backend checks cover graph rendering and interaction. Formatting and strict Clippy pass.

Run `cargo build` followed by `python3 scripts/validate_tui_pty.py`. On Linux, the harness checks normal exit, resize, partial Ctrl-C exit 1, injected panic, and injected operational error. Each case confirms that the prior termios settings and alternate screen are restored; panic and error text appears only after screen restoration. The fault injection environment variable is recognized only by debug builds.

## Generated benchmark baseline

Run `python3 scripts/benchmark_m4.py` after building the debug binary. Reference environment: Intel Core i7-12700F, x86_64 Linux, Rust 1.93.1, Tree-sitter 0.25.10, tree-sitter-python 0.25.0. Each generated repository has one function per Python file, one modified root, a two-function cycle, and a caller chain reaching every function. Three local JSON runs used `--depth` equal to the file count. Times are median wall-clock seconds including process startup; peak RSS is measured for each DFlow process with Linux `wait4`.

| Files | Symbols across two snapshots | Edges across two snapshots | Focused impact nodes | Median elapsed | Max child RSS |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 20 | 40 | 40 | 20 | 0.0121 s | 18,072 KiB |
| 100 | 200 | 200 | 100 | 0.0394 s | 22,560 KiB |

The earlier 100-file cyclic graph reached its first TUI graph frame in 0.0411 s in a pseudoterminal. Phase-specific capture, parse, resolution, traversal, and render timing is not isolated by the current binary; no performance budget is asserted from these first measurements.

## Task walkthroughs

The fixture checks completed three navigation tasks: a modified root showed its caller through an arrow and call-site evidence; an added root was selectable and displayed its target-snapshot callers; a deleted root displayed its base-snapshot caller and historical source. These are automated developer walkthroughs. The user chose to defer a human study. Human task times and a manual-inspection comparison have not been measured, so M4's full release gate remains open and no adoption or time-saving claim is made.

For a human comparison, use one repository fixture with all three change kinds. Ask each participant to identify (1) the modified function's two-step caller chain, (2) every caller of an added function, and (3) the deleted function's historical caller and source location. Run the tasks once with DFlow and once by manual code inspection, alternate the order across participants, and record correctness and elapsed seconds separately for each task and method. Keep those results out of the deterministic JSON report.
