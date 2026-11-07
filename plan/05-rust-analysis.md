# M5 — Rust as the second analysis language

Status: Planned. Prerequisite: [M4 Python MVP](04-terminal-ui-and-python-mvp.md) accepted.  
Coverage: P1, Rust compatibility/resolution limits, regressions across all Git modes and presentations.

## Deliverable

Add Rust analysis through `tree-sitter-rust` while preserving the single binary crate and existing snapshot, graph, JSON, and TUI contracts. Python remains supported and is tested before the Rust suite. The analyzer does not compile or execute the analyzed Rust project.

## Implementation

1. Add a grammar version compatible with the pinned Tree-sitter runtime, update the lockfile, and add internal Rust language modules plus `tests/fixtures/rust/`. Reuse the M2 language interface; do not fork the Git or reporting pipeline.
2. Extend source capture to `.rs` and static Rust module metadata needed for discovery. Include language identity in all symbol keys; mixed repositories dispatch files by language and never resolve a Python function directly to a Rust function by name.
3. Discover regular crates from captured root/member `Cargo.toml` files and conventional `src/lib.rs`/`src/main.rs` plus explicit static target paths. Parse manifests as data without Cargo subprocesses, build scripts, dependency downloads, or macro expansion. Keep multiple targets distinct where module identity differs. Unknown workspace inheritance/path layouts produce diagnostics rather than guessed module identities.
4. Extract free/async functions, inherent and trait methods as symbols, inline/external module scopes, signatures, bodies, generics, attributes, imports/aliases, bindings, and call sites. Preserve full qualified scope and the shared one-based byte-column ranges. Retain macro invocations as unsupported evidence rather than inventing expanded functions/calls.
5. Reuse syntax canonicalization to ignore ordinary whitespace/comments while retaining signatures, literals, doc comments with semantic attribute meaning, attributes, and nested-definition changes. Preserve path/scope/name matching and conservative deletion/addition for unmatched moves.
6. Resolve direct free-function calls and explicit static paths using lexical modules, supported `use` aliases, and `crate`/`self`/`super`. Resolve inherent associated-function paths only when the type/module identity is unambiguous. Shadowing and multiple candidates remain unresolved.
7. Keep trait dispatch, receiver-dependent method calls, macro-generated code, glob/re-export ambiguity, conditional compilation, procedural attributes, and unknown external targets unresolved. Do not create certain edges to conditionally available definitions when configuration is unknown. Record reason codes and supported syntax boundaries.
8. Pass Rust definitions and calls through existing `petgraph` graphs, caller traversal, reports, and TUI. Maintain target traversal for added/modified roots and base traversal for deleted roots. Reuse layout, controls, diagnostics, and JSON version 1 if changes are backward-compatible optional additions; bump the schema if an incompatible change is required.
9. Document tested Rust syntax versions and the supported module/call subset with examples. Mark unsupported source layouts incomplete when extraction cannot cover candidate source, while unresolved call targets alone do not make otherwise successful analysis incomplete.

## Tests and completion checklist

- [ ] Free, async, nested functions; inherent/trait methods; modules; signatures; generics; and locations have independently specified expectations.
- [ ] Static lexical/module calls, aliases, explicit qualified paths, recursion, and unchanged-file callers resolve correctly.
- [ ] Shadowing, trait/receiver dispatch, macros, ambiguous imports, conditional compilation, and third-party targets produce no speculative edges.
- [ ] Manifest/module discovery covers ordinary lib/bin targets and a captured workspace fixture without changing DFlow's own single-crate layout.
- [ ] Added/modified/deleted definitions and unmatched moves follow shared rules, including historical callers for deleted files/functions.
- [ ] Comments, formatting, literals, signatures, attributes, and nested-body changes have documented classification results.
- [ ] Combined/staged/unstaged and branch comparisons run end to end for Rust, including untracked and deleted files.
- [ ] JSON is deterministic; all references remain valid; the TUI requires no language-specific rendering fork.
- [ ] Parse/layout failures retain diagnostics/completeness, and repository/index/ref integrity holds for all paths.
- [ ] A mixed Python/Rust fixture analyzes both without cross-language invented edges; the complete Python suite still passes.
- [ ] Measure Rust precision and coverage separately, retain 100% fixture resolved-edge precision, and record performance against the M4 baseline.

P1 is complete when the supported Rust subset and limitations are documented, its tests pass through the shared CLI/TUI workflow, and all Python MVP behavior remains accepted.
