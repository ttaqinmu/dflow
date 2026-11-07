# DFlow Product Requirements Document

Status: Draft for development  
Date: November 7, 2026  
Context: [Project README](../README.md)

## Product summary

DFlow is a Rust CLI for understanding the impact of source changes in a Git repository. It compares two snapshots, identifies added, modified, and deleted functions, and traces their callers through a static call graph. Python is the first supported and tested analysis language; Rust support follows after the Python MVP.

The product answers three questions: which functions call a changed function, which functions call a newly added function, and which functions previously called a deleted function? Users explore these relationships in an interactive terminal graph or export a versioned JSON report. Mermaid export and a Zed extension follow later.

The current checkout contains documentation but no Cargo manifest or Rust source files. The README's description of an existing executable is not reflected in this checkout. All capabilities below are planned requirements, not implemented features.

## Problem and users

Git diffs show changed lines but do not explain call relationships. Developers must inspect references manually, and deleted functions require historical context to recover their callers.

| User | Need | Expected outcome |
| --- | --- | --- |
| Python developer, followed by Rust developer | Understand changes before committing | Changed functions and relevant caller chains |
| Code reviewer | Inspect changes between two branches | Source locations and verifiable call-site evidence |
| Maintainer | Review deletions and refactoring | Caller relationships from the earlier snapshot |

An arrow `A → B` means A calls B. Impact analysis follows incoming edges toward callers. Static reachability identifies code worth inspecting; it does not prove that runtime behavior changes.

## Goals and release boundaries

Goals:

- Identify added, modified, and deleted functions and methods.
- Trace direct and indirect callers, including callers in unchanged files.
- Preserve historical relationships for deleted symbols.
- Support both local development changes and comparisons between two branches.
- Draw an interactive graph directly in the terminal.
- Share one report model across terminal, JSON, and future presentation layers.
- Keep uncertain call resolution explicit.

Outside the Python MVP:

- Rust analysis, which is the next language milestone.
- Mermaid export, callee exploration, advanced rename matching, and Zed integration.
- Complete runtime call graphs, executing analyzed code, coverage measurement, or guarantees that a change is safe.
- Analysis of third-party dependency source outside the repository.
- Automatic test selection, source modification, cloud services, accounts, or team collaboration.

| Phase | Deliverable | Release boundary |
| --- | --- | --- |
| P0 — Python MVP | Single crate; local and branch comparisons; Python analysis; caller traversal; versioned JSON; interactive terminal graph | Conservative static resolution with explicit unresolved calls |
| P1 — Rust support | Rust extraction, resolution, and fixtures using the same snapshot and report interfaces | Begins after Python MVP acceptance passes |
| P2 — Export and editor integration | Mermaid export and a Zed extension prototype | Consumes the stable CLI report contract |

## Architecture and repository layout

The initial project must be a single Rust binary crate, using edition 2024. Put `Cargo.toml` directly in the project root. Do not introduce a Cargo workspace, a nested application project, separate core crates, or a plugin system in the initial implementation.

```text
Cargo.toml
Cargo.lock
src/
  main.rs
  cli.rs
  git/
  analysis/
  languages/
    python/
  graph/
  report/
  tui/
tests/
  fixtures/
    python/
```

Add Rust language modules and fixtures in P1. The initial binary crate uses internal modules rather than a separate library target. Analysis must remain independent of terminal rendering so a future extension can consume CLI JSON without requiring a crate split.

Required dependencies and responsibilities:

| Dependency | Responsibility |
| --- | --- |
| `git2` | Repository discovery, branch resolution, diffs, index access, and historical snapshot reads; production Git operations must not shell out to Git |
| `tree-sitter` | Parse source and extract syntax structure for analysis |
| `tree-sitter-python` | Python grammar for P0 |
| `tree-sitter-rust` | Rust grammar added in P1 |
| `petgraph` | Directed call graphs, incoming-edge traversal, and cycle handling |
| `ratatui` and `crossterm` | Terminal drawing, keyboard input, resize events, and terminal lifecycle |

Data flow: Git snapshots → language extraction → symbol matching and call resolution → per-snapshot graphs → impact report → JSON or terminal renderer. Tree-sitter provides syntax; language-specific code must implement name binding and call resolution. A parsed call expression alone does not establish its target.

Use internal language interfaces for extraction and resolution that return the shared symbol, call-site, and diagnostic model. Pin compatible dependency versions in the manifest and commit the binary project's lockfile.

## Git comparison modes

Both modes are required in P0.

| Mode | Base snapshot | Target snapshot |
| --- | --- | --- |
| Local, default | HEAD | Working tree, including staged and unstaged changes |
| Local, staged | HEAD | Index |
| Local, unstaged | Index | Working tree |
| Branch | Base branch tip | Target branch tip |

### Local mode

The default view compares the final working-tree state against HEAD. It reports the net change; staged edits later reversed in the working tree can disappear from this view while remaining visible in the staged view.

`--staged` compares HEAD with index contents, independently of subsequent working-tree edits. `--unstaged` compares index contents with the working tree. The flags are mutually exclusive.

Include non-ignored untracked source files by default in combined and unstaged comparisons. Staged mode reads only the index. A staged new file is present in the index base of an unstaged comparison, so it is not reported as newly added again unless appropriate to that comparison.

Treat an unborn HEAD as an empty base snapshot. Reject unresolved index conflicts with an actionable diagnostic. Local mode requires a working tree; a bare repository is invalid input for this mode.

### Branch mode

Require explicit `--base` and `--target` branch names. Accept local branches and explicit remote-tracking branch names; resolve each to a commit ID once at analysis start. Reject missing, invalid, or ambiguous branch names.

Compare the two branch-tip snapshots directly, equivalent to a two-dot diff. Do not substitute their merge base. Branch analysis is independent of dirty index and working-tree contents and does not fetch remote branches.

### Snapshot integrity

Read complete supported source context from each snapshot, not only changed files. The index is an independent source snapshot, not an alias for working-tree content. Deleted files and callers must remain readable from the base snapshot.

Analysis must not check out branches, update the index, write repository objects, modify source, or execute analyzed code. Capture the source bytes used by each analysis run and reuse those bytes for parsing and displayed evidence; do not reread live files while rendering the TUI. A local capture is not an atomic filesystem snapshot, and reports must not claim otherwise.

## Primary scenarios

### Modified function

A developer compares local changes or two branches. DFlow marks `process` as modified, shows its direct caller `entry`, and can trace `main → entry → process` within the selected depth. Call sites and source locations support verification, including when caller files did not change.

### Added function

A developer adds `notify` and calls it from `entry`. DFlow marks it added and shows callers from the target snapshot. An empty resolved caller set does not imply the function is never invoked at runtime.

### Deleted function

A developer deletes `removed`. DFlow uses the base graph to show that `legacy` previously called it. Historical nodes, edges, locations, and snapshot identity remain visibly marked as base context.

### Uncertain call

A call depends on `getattr`, dynamic imports, or ambiguous dispatch. DFlow preserves the call site and an unresolved reason without inventing an edge to a similarly named function.

## MVP functional requirements

| ID | Requirement | Acceptance criteria |
| --- | --- | --- |
| FR01 | Read both Git modes | All three local comparisons and branch-tip comparison produce the specified snapshots and record their identities |
| FR02 | Discover changed Python files | Added, modified, and deleted `.py` files appear; deleted contents remain available from base |
| FR03 | Analyze both complete source contexts | Callers in unchanged Python files can be found |
| FR04 | Extract Python symbols | Functions, async functions, and methods carry qualified names, lexical scopes, paths, and source ranges |
| FR05 | Match and classify symbols | Target-only symbols are added, base-only symbols are deleted, and changed paired definitions are modified |
| FR06 | Build graphs with `petgraph` | Each snapshot has its own directed caller-to-callee graph with call-site evidence |
| FR07 | Preserve unresolved calls | Every extracted call site is resolved or unresolved; unresolved sites retain reasons and create no certain edge |
| FR08 | Trace callers | Direct and transitive callers respect selected depth; cycles always terminate |
| FR09 | Preserve deletion context | Deleted symbols use base traversal; added and modified symbols use target traversal |
| FR10 | Export versioned JSON | Reports include snapshots, file changes, symbols, symbol changes, call sites, edges, impact, and diagnostics in deterministic order |
| FR11 | Report analysis failures | Diagnostics identify category and location where available; parse failure cannot be presented as complete analysis |
| FR12 | Handle empty results | JSON contains an empty change list; TUI shows a clear empty state; successful analysis exits zero |
| FR13 | Draw an interactive terminal graph | Directed connectors, change labels, selection, graph navigation, details, and resize handling work without external graph software |
| FR14 | Preserve repository state | Index, source files, refs, and working-tree status remain unchanged after analysis |

### Symbol identity and classification

- Pair symbols by repository-relative path, lexical scope, and name. Internal IDs distinguish base and target symbols and connect matched pairs.
- Signature or parsed-definition structure changes mark a symbol modified. Formatting and comments alone do not. Literal changes, including docstrings, count as changes.
- Changes inside nested functions may mark both the nested and enclosing definitions modified.
- Unmatched renames or moves appear as deleted and added. Symbol rename detection is not promised in P0.
- Module-level code, import, and configuration changes remain recorded as file changes, but their semantic impact beyond changed function definitions is outside P0.

### Python call resolution

Support direct calls within a module and explicit imports to repository modules, including unambiguous aliases. Support relative imports when package structure can be established without executing code. Shadowed names, multiple candidates, or unknown import configuration remain unresolved.

Method extraction does not imply complete method dispatch resolution. Instance dispatch, inheritance, callable-changing decorators, wildcard imports, dynamic imports, and runtime-created callables may remain unresolved. Module-level calls may carry a module context rather than being represented as user functions.

## CLI interface

The following commands describe the required interface; they are not yet implemented:

```sh
dflow local
dflow local --staged
dflow local --unstaged
dflow branch --base main --target feature

dflow local --format json
dflow branch --base main --target feature --format tui --depth 3
```

- Require an explicit `local` or `branch` mode. Reject branch arguments in local mode and local comparison flags in branch mode.
- Discover the repository from the current directory, including execution from a subdirectory.
- Support `--format tui|json`. Default to TUI when stdin and stdout are terminals; otherwise default to JSON. Explicit TUI requires both to be terminals.
- Default depth is one edge. Depth zero shows changed roots only; larger values trace indirect callers. Reject negative values.
- Write JSON to stdout and operational diagnostics to stderr. Keep machine-readable output free of terminal escape sequences.
- Use exit code `0` for complete analysis, `1` for analysis failure or partial analysis, and `2` for invalid input. Unresolved calls alone are not a failure.
- A partial report after a parse error must have `complete: false` and exit code `1`. Closing its TUI must preserve that failure status.

## Interactive terminal graph

The Python MVP must provide an actual graph with directed connectors, not only a list of symbols. Use three areas: a changed-symbol selector, a graph viewport for the selected root, and selected-node details.

Required behavior:

- Display caller-to-callee arrows and textual added, modified, and deleted labels. Do not depend on color alone.
- Distinguish base and target relationships, especially historical callers of deleted symbols.
- Support arrow keys or `j`/`k` for list/node selection, Tab for panel focus, Shift+arrow keys for graph panning, and Page Up/Page Down for scrolling.
- Use `+` and `-` to adjust caller depth, with zero as the minimum; recompute the focused view from captured graphs.
- Show qualified name, change type when applicable, snapshot, path, source range, and call-site evidence in node details.
- Use deterministic layered placement, collapsing strongly connected components for layout. Expose component members in details. Shared nodes and cycle references must remain inspectable without infinite expansion.
- Handle terminal resizing, small-terminal guidance, and empty results. Provide `?` help and `q` or Ctrl-C to quit.
- Restore raw mode and alternate-screen state on exit and failure, including panic cleanup.

Unresolved calls appear in details or diagnostics, never as certain graph edges. Source editing, launching an editor, and live file watching are outside the first TUI release.

## Report contract

The report is independent of terminal layout. Final JSON field spelling must be documented before P0 release; the following information is mandatory.

| Section | Minimum information |
| --- | --- |
| Metadata | Schema version, tool version, comparison mode, local comparison selection where applicable, analysis options, completeness |
| Snapshots | Base/target role; commit, index, working-tree, or empty kind; commit IDs and branch names where applicable; content identity for captured local inputs |
| File changes | Before/after paths where available and file change type |
| Symbols | ID, language, qualified name, kind, snapshot, repository-relative path, source range |
| Symbol changes | Added, modified, or deleted; before/after symbol references where available |
| Call sites | Caller or module context, target expression, location, snapshot, resolution status, resolved target or unresolved reason |
| Edges | Caller, callee, snapshot, and supporting call-site references |
| Impact | Changed root, reachable callers, depth, and traversal relationships |
| Diagnostics | Category, message, file/location where available, and effect on completeness |

Do not label index or working-tree contents as commits. Local content identity must cover the captured analysis inputs, including paths and bytes, so different local source states remain distinguishable.

Use one-based lines and UTF-8 byte columns, with exclusive range ends. IDs are unique within a report but need not persist across Git history. Deduplicate nodes during traversal while retaining relevant edges. Multiple call sites between the same caller and callee may share an edge with multiple evidence locations.

Never merge base and target graphs without snapshot labels. Modified symbols use target relationships in P0; comparison of their removed and added call relationships is a later enhancement.

## Rust support and future integrations

### Rust analysis — P1

Begin after the Python acceptance suite passes. Add `tree-sitter-rust`, Rust-specific extraction and resolution, and Rust fixtures while retaining the same Git modes, graph model, report, and TUI.

Start with function and method extraction and direct, statically resolvable repository calls. Trait dispatch, macro-generated code, conditional compilation, and other uncertain targets must remain explicit limitations rather than fabricated resolved edges. Document the supported Rust subset and its acceptance fixtures before releasing P1.

“Python first, then Rust” refers to the languages under analysis. The CLI and integration-test harness remain implemented in Rust.

### Mermaid export and Zed extension — P2

Mermaid export renders a focused subgraph from the shared report, with caller-to-callee arrows, change labels, snapshot markers, and safely escaped labels. It does not replace the MVP terminal graph.

The later Zed extension invokes the CLI and consumes versioned JSON to display changes and caller relationships and navigate source locations. Deleted symbols require historical content or an explicit historical reference. Extension packaging and editor-specific APIs are deferred until the CLI contract stabilizes; they must not force an initial workspace or crate split.

## Nonfunctional requirements

- **Correctness:** every resolved edge in acceptance fixtures has the correct target. Keep uncertainty visible rather than increasing graph coverage through guesses.
- **Reproducibility:** identical captured inputs, options, and tool version produce identical JSON. Exclude variable timing metadata from default output.
- **Repository integrity:** perform no checkout, index writes, ref changes, source writes, or execution of analyzed Python or Rust code.
- **Separation:** language analysis and reports must not depend on TUI state or terminal availability.
- **Compatibility:** use Rust edition 2024. Document the supported Python syntax range from the selected grammar before P0 release and the Rust syntax range before P1 release.
- **Performance:** record duration and peak memory with file, symbol, and edge counts on a documented reference machine. Establish numerical budgets after measuring the first baseline; do not invent benchmark results.

## Validation and acceptance

Use small before/after fixtures with known symbols and edges. Rust integration tests create temporary Git repositories using `git2`, populate commits/index/working-tree states, invoke the binary, and verify the report. Establish Python fixtures and acceptance first; add Rust fixtures in P1.

| Test scenario | Required result |
| --- | --- |
| Default local comparison | HEAD versus final working-tree contents, including staged and unstaged edits |
| Staged and unstaged views | HEAD/index and index/working-tree contents remain distinct, including overlapping edits |
| Staged additions and deletions | Classification follows the selected snapshot pair, not live-file availability |
| Non-ignored untracked file | Included in default and unstaged analysis, absent from staged analysis |
| Ignored untracked source | Excluded from local discovery |
| Diverged branches | Compare branch tips directly; fixture distinguishes this from merge-base behavior |
| Branch comparison in a dirty repository | Output depends on selected commits, not local edits |
| Unborn HEAD and conflicted index | Empty HEAD base works; conflicts yield actionable failure |
| Modified function with unchanged caller file | Modified symbol and caller are found |
| Added function with or without callers | Added classification and correct target caller set |
| Deleted function or file | Deleted symbol and historical caller evidence use base |
| Local calls, explicit imports, aliases | Unambiguous calls resolve to correct targets |
| Shadowing and dynamic or ambiguous calls | Preserve unresolved sites without invented edges |
| Call chains, recursion, and shared callers | Depth is respected; traversal and graph rendering terminate |
| Comments, formatting, signatures, literals, nested functions | Classification follows the stated rules |
| Invalid arguments/branches and parse failures | Exit codes, diagnostics, and completeness follow the CLI contract |
| No changed functions | Valid empty JSON and clear TUI empty state |
| Repeated identical captured inputs | Identical JSON and unchanged repository contents/status |
| Rename without matching | Consistent deleted/added result |
| TUI interaction | Selection, panel switching, panning, depth adjustment, details, help, and quit work |
| TUI rendering and lifecycle | Connectors, cycle groups, historical labels, resize handling, and terminal restoration work |
| Non-terminal execution | Defaults to clean JSON; explicit TUI is rejected |

P0 is complete only when both Git modes, Python analysis, versioned JSON, and the interactive graph satisfy acceptance. Use deterministic renderer checks with a test terminal backend, plus a real-terminal smoke check for input, resize, and cleanup.

Measure resolved-edge precision separately from resolution coverage. Acceptance fixtures require 100% precision for resolved edges; this is not a claim of complete accuracy on arbitrary projects. Record whether developers can identify callers and deletion context, and compare task completion time with manual inspection before setting adoption or time-saving targets.

## Development milestones

| Milestone | Deliverable | Completion gate |
| --- | --- | --- |
| M1 — Project and Git snapshots | Root-level single crate, required core dependencies, local and branch snapshot readers | Git mode, snapshot integrity, and file-change fixtures pass |
| M2 — Python symbols | Tree-sitter extraction, locations, symbol matching, change classification | Python function, method, async, and definition-change fixtures pass |
| M3 — Graph and report | `petgraph` call graphs, unresolved calls, caller traversal, versioned JSON | Cross-file callers, deleted symbols, cycles, and determinism pass |
| M4 — Python MVP | Interactive graph, diagnostics, CLI documentation | All P0 criteria and terminal lifecycle checks pass |
| M5 — Rust | Rust grammar, extraction, conservative resolution, and Rust fixtures | Documented Rust subset passes the shared analysis and presentation workflow |
| M6 — Export and Zed | Mermaid renderer and Zed extension prototype consuming CLI JSON | Changed and deleted symbols can be inspected through the editor workflow |

No release dates are committed. Synchronize README status, commands, dependencies, and roadmap with these requirements when implementation begins.

## Risks and remaining release decisions

| Risk | Effect | Mitigation |
| --- | --- | --- |
| Dynamic Python semantics | Real callers may be missed | Explicit unresolved reasons and documented resolution limits |
| Refactoring and symbol moves | Renames appear as deletion/addition | Document P0 behavior; improve matching after the baseline |
| Package/import diversity | Cross-module resolution becomes ambiguous | Validate simple packages first; preserve ambiguity |
| Large graphs and cycles | Slow analysis or unreadable output | Depth limits, deduplication, component layout, viewport navigation, and benchmarks |
| Concurrent local edits | Captured files may represent different instants | Reuse captured bytes and state the non-atomic capture limitation |
| Report schema evolution | Consumers may break | Version JSON and document compatibility before editor integration |

Settled decisions are the required crates, root-level single binary crate, Python-before-Rust sequence, all three local comparisons, inclusion of non-ignored untracked files, direct branch-tip comparison, and interactive terminal output in P0.

Remaining release decisions are the exact supported syntax versions; source-root/package discovery, symlink and submodule policy; final JSON field names and compatibility rules; benchmark datasets and budgets; and Zed packaging. Parser choice, graph library, local comparison semantics, and terminal interactivity are no longer open questions.
