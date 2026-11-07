# M1 — Project foundation and Git snapshots

Status: Planned. Prerequisite: [PRD](PRD.md) and [shared rules](README.md).  
Coverage: FR01–FR03 foundations, FR11 and FR14; all snapshot-related acceptance cases.

## Deliverable

Create the root-level Rust 2024 binary crate and read every source snapshot needed by local and branch modes without changing the analyzed repository. This milestone does not claim to produce call graphs or a complete analysis report.

## Implementation

1. Create root `Cargo.toml`, `Cargo.lock`, `src/main.rs`, and internal `cli`, `git`, `analysis`, `languages`, `graph`, `report`, and `tui` modules. Use `git2`, compatible `tree-sitter`/`tree-sitter-python`, `petgraph`, `ratatui`, and `crossterm`; use Clap for arguments, Serde/Serde JSON for reports, SHA-256 for content fingerprints, and Tempfile for test repositories. Add the Rust grammar only in M5.
2. Parse `analyze local` and `analyze branch`. Define owned request types for comparison mode, depth, and requested output format. Reject invalid flag combinations and negative depth at parsing time. Resolve terminal-dependent output selection separately from argument parsing; do not enter raw mode during validation.
3. Discover repositories upward from the current directory, supporting linked worktrees and execution from subdirectories. Return a typed error for discovery failure. Local mode requires a worktree; branch mode can read a bare repository.
4. Model `SnapshotKind`, `SnapshotRole`, `CapturedFile`, `CapturedSnapshot`, `SnapshotPair`, and `FileChange` as owned internal data. Store repository-relative paths, source bytes, file mode, commit/branch identity where applicable, and stable content identity. Downstream analysis must use these captured bytes.
5. Read commit trees recursively through `git2` blobs, read stage-zero index entries from blob objects, and capture worktree files separately. Detect conflicts before local capture. Do not use APIs that write trees or refresh/write the index.
6. Assemble the three local pairs exactly as the PRD specifies. Treat unborn HEAD as an empty base. Worktree candidates include tracked/index paths plus non-ignored untracked sources discovered through `git2`; tracked files remain candidates even if an ignore rule matches them. Detect filesystem deletion without falling back to the index bytes.
7. Resolve branch arguments to local or explicitly qualified remote-tracking branches. Accept `refs/heads/...` and `refs/remotes/...`; ordinary unqualified names identify local branches and names such as `origin/main` may identify remote-tracking branches. Reject ambiguous local/remote shorthand rather than guessing. Do not accept arbitrary revision expressions as branch names. Capture the two resolved tip trees directly, without merge-base substitution or checkout.
8. Compare captured file maps by path, bytes, and mode. Report added, modified, and deleted entries deterministically. Unmatched moves remain deletion/addition. Use `git2` diff facilities where appropriate, but ensure final change membership matches the captured source bytes rather than a second read of live files.

## Source discovery and capture policy

- P0 analyzes `.py` files throughout each snapshot. Preserve relevant package markers and configuration inputs for later package discovery; no caller search is restricted to changed files.
- Skip `.git` metadata and Git submodule entries. Do not recurse into submodules or follow symlinks, including links outside the repository. Emit a scoped diagnostic when a candidate source is skipped; do not imply that its callers were analyzed.
- Do not silently skip tracked source merely because it sits in a conventionally generated directory. Ignore rules govern untracked discovery.
- Capture each file once per snapshot. Distinguish an absent file from a permission/read failure. A read failure makes analysis incomplete; a file that disappears during capture must produce a diagnostic rather than silently pretending it was never discovered.
- Fingerprint sorted, length-delimited path/mode/content records using SHA-256. Include the relevant captured metadata inputs. Exclude absolute checkout locations and capture time. Local capture is explicitly non-atomic.
- Retain path bytes internally. Use a reversible JSON-safe representation when a path is not valid UTF-8; do not conflate different paths through lossy conversion. The report contract finalized in M3 must document this representation.

## Interfaces and failures

`capture_comparison(request)` returns a snapshot pair, ordered file changes, and diagnostics, or a fatal typed error. It does not depend on terminal types. Expose source reads from the captured map, including base-only deleted files.

Invalid branches, conflicting flags, missing repository, conflicted index, and local mode in a bare repository map to input exit code 2. Filesystem/object reading failures map to analysis exit code 1. A syntactically valid empty snapshot pair is successful. Keep errors off stdout.

Until M3/M4 wire complete analysis and output, use internal tests to validate snapshot behavior; do not ship empty placeholder analysis as a successful implemented feature.

## Test and completion checklist

- [ ] Combined HEAD/worktree, staged HEAD/index, and unstaged index/worktree have independently verified file bytes.
- [ ] Stage an edit and reverse it in the worktree: combined is unchanged, staged and unstaged remain distinct.
- [ ] Stage additions/deletions and then edit, remove, or recreate the worktree file; each mode uses its own pair correctly.
- [ ] Include non-ignored untracked `.py` files only in combined/unstaged; exclude ignored untracked sources and include tracked ignored sources.
- [ ] Cover unborn HEAD, detached HEAD in local mode, conflict entries, bare repositories, subdirectories, linked worktrees, missing/unreadable blobs, symlinks, and submodules.
- [ ] Use diverged branches where direct-tip and merge-base comparisons differ; verify direct tips and immunity to dirty/conflicted local state in branch mode.
- [ ] Cover invalid/ambiguous branches, remote-tracking names without fetch, deleted historical files, unchanged files, and deterministic fingerprints.
- [ ] Compare source bytes, index bytes, refs, object inventory, and Git status before/after successful and failed analysis, excluding expected fixture setup.
- [ ] Root manifest/lockfile exist, there is one binary crate, and no feature is advertised beyond its implementation state.

Handoff to M2: immutable complete source maps, precise file changes, snapshot identities, and structured capture diagnostics.
