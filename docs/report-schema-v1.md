# DFlow JSON report schema v1

`dflow local` and `dflow branch` print exactly one JSON object and a newline to stdout. M3 defaults to JSON in every environment. `--format tui` becomes available in M4. Exit status is 0 for complete analysis, 1 for partial analysis or output failure, and 2 for invalid input. Operational errors go to stderr. An unresolved call does not make a report partial.

## Top-level fields

| Field | Type | Meaning |
| --- | --- | --- |
| `schema_version` | integer | Currently `1` |
| `tool_version` | string | Version from `Cargo.toml` |
| `comparison` | object | `mode`, `depth`, and either local `selection` or branch `base`/`target` names |
| `snapshots` | object | `base` and `target` snapshot metadata |
| `file_changes` | array | Path and `added`, `modified`, or `deleted` kind |
| `symbols` | array | Extracted function/method definitions from both snapshots |
| `module_contexts` | array | Synthetic callers for module-level calls; never user functions |
| `symbol_changes` | array | Change kind and optional `before`/`after` symbol IDs |
| `call_sites` | array | Every safely extracted call, including unresolved calls |
| `edges` | array | Certain caller → callee relationships and evidence IDs |
| `impacts` | array | Caller subgraph for each changed symbol at requested depth |
| `diagnostics` | array | Extraction, matching, and Git capture diagnostics |
| `complete` | boolean | False if source capture or parsing prevented complete supported analysis |

Snapshot metadata contains `kind` (`commit`, `index`, `working_tree`, or `empty`), `role`, `commit_id`, `branch`, and `content_id`. A commit ID is present only for commit snapshots. Local index/worktree identity comes from the captured content fingerprint; no local content is labeled as a commit. `comparison.selection` is `combined`, `staged`, or `unstaged` in local mode. Branch mode compares the named tips directly.

## IDs, paths, and locations

Symbol IDs start with `s-`, module context IDs with `m-`, and call site IDs with `c-`. They include snapshot role (`b` or `t`) and a deterministic ordinal. They are unique within one report and are not stable identifiers across unrelated runs or repository histories.

Paths are repository relative. A UTF-8 path is `{"text":"pkg/module.py"}`. A path containing invalid UTF-8 is `{"hex":"6e616d652dff2e7079"}`; hex is the original path bytes. Consumers must decode exactly one variant and never use a lossy display path as identity.

Ranges contain `start` and `end`, each with one-based `line`, one-based UTF-8 `byte_column`, and zero-based `byte_offset`. Ends are exclusive. A symbol carries `id`, `language`, `name`, `qualified_name`, `kind`, `snapshot`, `path`, `range`, and `fingerprint`.

A call site carries `id`, `caller`, `expression`, `snapshot`, `path`, `range`, and either `target` or `unresolved_reason`. `target` is null for unresolved calls; `unresolved_reason` is null for resolved calls. An edge has `caller`, `callee`, `snapshot`, and one or more `call_sites` IDs. Callers may reference a symbol or a `module_contexts` entry. Every other referenced ID resolves within the same report and snapshot.

An impact has `root`, `snapshot`, `nodes` (`id`, minimum caller `depth`) and `edges` (`caller`, `callee`). For deleted roots, all impact references use the base snapshot. Added and modified roots use the target snapshot. Depth zero contains only the root. Unresolved calls never create edges.

Diagnostics carry `category`, `message`, `path`, `snapshot`, `range`, and `affects_completeness`. The last three may be null when unavailable. `complete: true` means the supported source set was captured and parsed; it does not claim that every call was resolved or that runtime behavior was proven.

## Ordering and compatibility

Arrays are emitted in deterministic order. Reports omit run timestamps and absolute checkout paths. Repeating the same captured inputs with the same DFlow version and options produces identical JSON bytes. Consumers should ignore unknown optional fields in schema v1 and reject an unsupported major schema version. A change to required field meaning or reference semantics requires a new schema version.

For examples of all Git modes and the analyzed language subset, see the [README](../README.md) and [PRD](../plan/PRD.md).

## Representative JSON fragments

These fragments show the fields that differ by case; each real CLI response also includes every top-level field listed above. IDs and content fingerprints below are illustrative.

Staged local comparison (`dflow local --staged --format json`):

```json
{
  "comparison": {"mode": "local", "selection": "staged", "depth": 2},
  "snapshots": {
    "base": {"role": "base", "kind": "commit", "commit_id": "<base commit OID>", "branch": null, "content_id": "<fingerprint>"},
    "target": {"role": "target", "kind": "index", "commit_id": null, "branch": null, "content_id": "<fingerprint>"}
  }
}
```

Unstaged and combined local comparisons both use a working-tree target. Unstaged uses an index base; combined uses a commit base. An unborn repository may use an `empty` base:

```json
{
  "comparison": {"mode": "local", "selection": "unstaged", "depth": 1},
  "snapshots": {
    "base": {"role": "base", "kind": "index", "commit_id": null, "branch": null, "content_id": "<fingerprint>"},
    "target": {"role": "target", "kind": "working_tree", "commit_id": null, "branch": null, "content_id": "<fingerprint>"}
  }
}
```

Branch comparison (`dflow branch --base main --target feature --format json`) uses two commit snapshots:

```json
{
  "comparison": {"mode": "branch", "base": "main", "target": "feature", "depth": 2},
  "snapshots": {
    "base": {"role": "base", "kind": "commit", "commit_id": "<base commit OID>", "branch": "main", "content_id": "<fingerprint>"},
    "target": {"role": "target", "kind": "commit", "commit_id": "<target commit OID>", "branch": "feature", "content_id": "<fingerprint>"}
  }
}
```

A non-UTF-8 filename uses its original bytes in every path field:

```json
{"file_changes": [{"path": {"hex": "6e616d652dff2e7079"}, "kind": "added"}]}
```

An empty result is still a complete report with empty change, impact, and diagnostic arrays:

```json
{"file_changes": [], "symbol_changes": [], "impacts": [], "diagnostics": [], "complete": true}
```

A file that cannot be parsed yields a partial report and exit status 1. Other analyzable files can still contribute evidence:

```json
{
  "diagnostics": [{"category": "parse_error", "message": "<parser detail>", "path": {"text": "broken.py"}, "snapshot": "target", "range": null, "affects_completeness": true}],
  "complete": false
}
```
