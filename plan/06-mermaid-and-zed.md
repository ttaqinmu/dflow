# M6 — Mermaid export and Zed integration prototype

Status: Planned. Prerequisite: [M5](05-rust-analysis.md) and the stable M3 report contract.  
Coverage: P2 diagram export, editor invocation/report consumption, and current/historical source references.

## Deliverable

Export focused Mermaid diagrams from the shared report and provide a Zed workflow that invokes DFlow, consumes JSON, and exposes changes, callers, and source references. Keep the existing CLI crate intact. This phase does not promise arbitrary custom panels through unsupported Zed APIs.

## Mermaid implementation

1. Add `mermaid` to `--format` while retaining existing automatic TUI/JSON defaults. Both Git modes and both languages use the same depth/root selection and snapshot rules as other renderers.
2. Render the report's focused impact subgraphs, not a second analysis. Use generated safe node IDs independent of labels, caller-to-callee arrows, textual change labels, and explicit base/target markers. Namespace historical and current nodes so they cannot be accidentally merged.
3. Deduplicate shared nodes/edges, retain cycle edges, and never include unresolved calls as certain edges. At depth zero emit the changed roots. An empty report produces a valid minimal diagram with an empty-state annotation.
4. Escape quotes, brackets, markup delimiters, backslashes, and newlines in symbol/path labels using a documented encoding; fixture-test adversarial labels. Keep generation order deterministic and avoid environment-dependent labels.
5. Write Mermaid to stdout and diagnostics to stderr. Preserve analysis exit/completeness behavior. Partial diagrams must visibly state their incomplete status.

## Zed integration implementation

First validate the supported Zed version and extension capabilities against the [extension documentation](https://zed.dev/docs/extensions/developing-extensions). Zed's documented task system runs commands in the integrated terminal; use it for the native interactive graph workflow ([tasks reference](https://zed.dev/docs/tasks)). Prototype the JSON-consuming extension path through the documented context-server mechanism rather than assuming a general custom UI API.

1. Add example Zed tasks for local, staged, unstaged, and configured branch-pair analysis, running the installed `dflow` in the selected worktree with explicit TUI output. Use structured arguments and proper path quoting; do not silently save editor buffers. Document that unsaved buffers are outside Git/worktree analysis.
2. Add a thin MCP stdio adapter subcommand to the existing DFlow binary. An analysis tool accepts an explicit repository directory, comparison mode, branch/local options, and depth; it invokes the same internal analysis request and returns the versioned report, not a reconstructed graph. Protocol stdout must contain only protocol messages; logs use stderr. Validate paths/options and prohibit Git mutations exactly as CLI mode does.
3. Expose captured source evidence by snapshot identity/path/range for historical references. Commit content may be reread by immutable object ID; local source evidence must come from the retained capture, never the current live file. Expired local captures must be reported as unavailable rather than silently substituted.
4. Build a minimal development extension launcher in a separate companion project, keeping DFlow's main repository a single crate. Locate a configured/installed DFlow binary, verify its supported protocol/schema, and launch its stdio adapter. Consume the JSON report through the adapter and expose structured changed-symbol/caller evidence in the supported Zed surface.
5. Keep target paths and line references visible for navigation; visibly distinguish historical base references and supply read-only captured content when the editor surface supports it. When direct editor navigation is unavailable, show explicit path/range/snapshot references and historical text instead of opening the wrong live line. Native custom source navigation remains a capability-dependent enhancement.
6. Provide dev-install instructions, configured binary path handling, errors for missing/incompatible binaries or schema versions, and a smoke-test walkthrough. Do not automatically publish or install the extension.

The documented [MCP extension mechanism](https://zed.dev/docs/extensions/mcp-extensions) currently includes a deprecation notice in favor of registry distribution. Recheck it before implementation. Keep launcher packaging replaceable by direct server configuration if that mechanism is removed; document this as an integration constraint, and do not label a task-only workaround as the completed JSON-consuming extension deliverable. Publishing to a registry is separate release work.

## Tests and completion checklist

- [ ] Mermaid snapshots cover additions, modifications, deletions, multiple roots, cycles/shared nodes, depth zero, empty output, partial analysis, and hostile labels.
- [ ] Both languages and all Git modes produce equivalent report relationships across JSON, TUI, and Mermaid.
- [ ] Validate generated diagrams with a Mermaid parser during development; do not require a diagram renderer at normal CLI runtime.
- [ ] Adapter contract tests cover valid requests, invalid paths/options, protocol/schema mismatch, partial analysis, unresolved calls, and clean protocol stdout.
- [ ] Captured source lookups distinguish base/target and current/historical content; expired local evidence never falls back to live source.
- [ ] Extension smoke tests cover missing/configured binary, repositories with spaces, selected worktrees, unsaved editor buffers, and incompatible report versions.
- [ ] From Zed, inspect a modified symbol and its callers, an added symbol with/without callers, and a deleted symbol with historical evidence.
- [ ] Cancellation/closing the editor session ends adapter work cleanly and leaves the repository unchanged; repeated runs do not leak terminal/process state.
- [ ] Recheck documented Zed capability and packaging assumptions, record the tested editor version, and keep unsupported native UI behavior explicit.
- [ ] Update CLI/schema/editor documentation and keep all M1–M5 regressions green.

Completion requires both validated Mermaid output and a demonstrated JSON-consuming Zed prototype. Task integration alone is useful but does not close the extension requirement. Advanced rename detection, full runtime dispatch, outgoing callee browsing, and native custom editor panels remain later work unless separately scoped.
