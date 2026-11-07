# M4 — Interactive terminal graph and Python MVP

Status: Implemented; automated validation passed. Human comparative usability timing remains unmeasured. Prerequisite: [M3](03-call-graphs-and-reports.md).
Coverage: FR11–FR14, final P0 integration, performance and usability validation.

## Deliverable

Complete the Python MVP with an interactive terminal graph, release documentation, and all inherited Git/analysis/report behavior. A symbol list or Mermaid text alone does not satisfy the graph requirement.

## Implementation

1. Resolve format defaults before terminal setup. If both stdin and stdout are terminals, default to TUI; otherwise default to JSON. Explicit JSON always bypasses terminal initialization. Explicit TUI without both terminals is invalid input and exits 2.
2. Separate application state, layout, event handling, and rendering. State contains selected changed root, panel focus, graph node selection, caller depth, pan/scroll offsets, help state, and read-only captured analysis. Recompute only the focused impact view when depth changes.
3. Render three panels: changed-symbol selector, graph viewport, and node details. Show the comparison mode, local selection or branch pair, snapshot identity, and analysis completeness in visible context. Show an explicit partial-analysis warning without changing the underlying report status.
4. Build layout from the focused graph: group strongly connected components with `petgraph`, sort components/members deterministically, layer callers to the left of callees, and route arrows through visible connector lanes. Mark grouped cycles and expose member selection in details. Deduplicate shared nodes but preserve their connectors.
5. Clip to a scrollable/pannable viewport; draw textual added/modified/deleted and base/target labels so color is optional. Preserve graph direction visually and in help. Use captured source excerpts and call sites for node details; never reread changed files during rendering.
6. Implement panel-aware arrow and `j`/`k` selection, Tab focus cycling, Shift+arrows panning, Page Up/Down scrolling, `+`/`-` depth changes, `?` help, and `q`/Ctrl-C exit. Keep depth nonnegative. Clamp selections and viewport offsets after root/depth/size changes.
7. Show guidance when the terminal cannot fit meaningful panels. Handle resize without panic; preserve selection when possible. Empty analyses show a clear empty state and diagnostics without phantom graph nodes.
8. Guard raw mode and alternate-screen lifecycle with RAII, handling partial initialization, errors, Ctrl-C, and panic cleanup. Perform analysis before entering raw mode. Restore terminal state before writing final errors to stderr. Preserve exit 1 when the user closes a partial report.
9. Document installation/build requirements, all comparison commands, flag/output defaults, source discovery policy, supported Python syntax, unresolved reasons, limitations, schema version, graph direction, keyboard shortcuts, and exit codes. Correct README's stale executable claims and dependency roadmap.

## Release validation

Use Ratatui's test backend to assert directed connectors, change/snapshot labels, cycle groups, empty states, details, diagnostics, and small/large layouts. Test the event reducer with real graph fixtures rather than snapshots alone. Use a pseudoterminal or manual smoke check for behavior a buffer backend cannot prove: keyboard decoding, resize events, Ctrl-C, raw mode, alternate-screen cleanup, and panic/error restoration.

Run the entire PRD acceptance matrix through the binary. Include combined/staged/unstaged, branch tips, non-ignored untracked sources, unborn HEAD, conflicts, dirty branch comparisons, removed historical files, unchanged callers, ambiguous calls, cycles, parse failures, and deterministic JSON. Verify original sources, index, refs, and status after success and failure.

Create a representative small/medium generated Python benchmark with known file/symbol/edge counts and a cyclic/large focused graph. Record reference hardware, toolchain, grammar versions, elapsed time, and peak memory outside report output. Measure capture, parse, resolve, traversal, and initial render separately where practical. Use the first baseline to document budgets; do not fabricate performance targets or make them environment-dependent test failures.

Run three usability tasks: find a modified function's caller chain, confirm a newly added function's callers, and inspect a deleted function's historical caller. Record success and time against manual inspection; keep adoption/time-saving targets unset until evidence exists.

## Completion checklist

- [x] All FR01–FR14 automated acceptance cases pass and all M1–M3 regressions remain green.
- [x] The CLI draws actual navigable caller → callee connectors, with inspectable cycles and shared nodes.
- [x] Keyboard controls, depth changes, evidence details, resize, help, quit, and empty/partial states work.
- [x] Color-free output remains understandable; long labels do not corrupt graph layout.
- [x] Explicit/default TUI/JSON behavior works for terminal and piped execution; JSON has no escape sequences.
- [x] Terminal state is restored after normal exit and Ctrl-C; partial reports keep failure status. Panic cleanup is guarded but not induced in a terminal test.
- [x] JSON schema and Python syntax/resolution boundaries are documented; fixtures retain conservative resolved edges.
- [x] Benchmarks and automated task walkthroughs are recorded honestly, and README matches implemented behavior.
- [x] Formatting, Clippy, tests, and a pseudoterminal smoke check pass.

Human comparative task timing and an induced-panic terminal smoke test remain release validation follow-ups. See [M4 validation](../docs/m4-validation.md).

Do not start Rust analysis or editor-specific code until this milestone's gate passes. Defer live watching, source editing, outgoing callee browsing, advanced rename matching, and modified-root relationship comparison.
