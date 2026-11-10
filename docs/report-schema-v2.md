# DFlow JSON report schema v2

`dflow local` and `dflow branch` emit one JSON object followed by a newline. `--format json` forces JSON; redirected input or output selects JSON by default. Exit codes: 0 complete, 1 partial/output failure, 2 invalid input. Unresolved calls alone do not make analysis partial.

## Top-level fields

| Field | Meaning |
| --- | --- |
| `schema_version` | Integer `2` |
| `tool_version` | Package version |
| `comparison` | `mode`, `depth`, and local `selection` (`combined`, `staged`, `unstaged`) or branch `base`/`target` |
| `snapshots` | `base` and `target` metadata: `kind`, `role`, `commit_id`, `branch`, `content_id` |
| `file_changes` | Repository path and `added`, `modified`, or `deleted` kind |
| `symbols` | Function/method definitions from each snapshot |
| `module_contexts` | Synthetic module-level callers |
| `symbol_changes` | `kind` (`added`, `deleted`, `modified`), `before` and `after` symbol IDs or null |
| `edge_changes` | Added/deleted certain directed caller→callee *pairs* across snapshots |
| `call_sites` | All extracted calls, including unresolved calls |
| `edges` | Certain directed pairs and their call-site evidence in each snapshot |
| `impacts` | Caller subgraphs rooted at changed symbols only, at the requested depth |
| `diagnostics` | Git, extraction, parsing, and matching diagnostics |
| `complete` | False if capture or parsing prevents complete supported analysis |

A symbol has `id`, `language`, `name`, `qualified_name`, `kind`, `snapshot`, `path`, `range`, `fingerprint`. The full-definition `fingerprint` is retained from v1. A matched unique symbol is `modified` for a direct definition edit, except a call-target-only edit whose every changed callee is covered by certain deleted and added pairs. Nested-function body edits change the nested symbol without changing its enclosing function. Arguments, signatures/defaults, docstrings, decorators/attributes, return statements, guards, and other body edits remain node modifications. Import-only resolution changes can produce edge changes without symbol changes.

Each `edge_changes` entry is `{ "kind": "added"|"deleted", "before": null|{"caller": ID, "callee": ID}, "after": null|{"caller": ID, "callee": ID} }`. Exactly one side is non-null: deleted pairs reference base IDs and added pairs reference target IDs. Each non-null pair exists in `edges` for that snapshot. Comparison is by unique resolved directed pair, **not** call-site count or position: repeated calls contribute one edge change at most; removing one of several calls does not remove the pair. An unresolved call creates no edge. Resolved→unresolved removes the pair only if no other resolved call supports it; unresolved→resolved adds a pair. Ambiguous duplicate-name endpoints and source units invalid in either present snapshot produce no cross-snapshot edge delta. Genuinely missing source units can produce added/deleted pairs. Ordering is deterministic by reconciled caller and callee identity, then kind.

A call site has `id`, `caller`, `expression`, `snapshot`, `path`, `range`, `target` (null if unresolved), and `unresolved_reason` (null if resolved). The reason is an open string; consumers should preserve unfamiliar values. An `edges` record has `caller`, `callee`, `snapshot`, and `call_sites` evidence IDs. An impact has `root`, `snapshot`, `nodes` (`id`, minimum caller `depth`), and `edges` (`caller`, `callee`); deleted roots use base IDs, added/modified roots target IDs. Edge-only changes do not generate symbol impacts.

IDs start with `s-` for symbols, `m-` for module contexts, `c-` for call sites, followed by snapshot role (`b` or `t`) and deterministic ordinals. IDs are report-local. Paths are repository-relative: UTF-8 paths use `{"text":"pkg/module.py"}`, other byte sequences use `{"hex":"..."}`. Rust source units include their crate target/module context, not just the file path. Ranges contain `start` and exclusive `end`, each with one-based `line`, one-based UTF-8 `byte_column`, and zero-based `byte_offset`.

Diagnostics contain `category`, `message`, `path`, `snapshot`, `range`, `affects_completeness`; unavailable fields may be null. `complete: true` claims the supported set was captured and parsed, not that all calls resolved or runtime behavior is proven. Repeating identical captured inputs/options produces identical JSON bytes. Consumers should reject unsupported schema versions. [Schema v1](report-schema-v1.md) remains available for historical reports.
