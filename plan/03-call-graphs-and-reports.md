# M3 — Call resolution, impact graphs, and JSON

Status: Planned. Prerequisite: [M2](02-python-symbols-and-changes.md).  
Coverage: FR03, FR06–FR12, and repository integrity inherited from M1.

## Deliverable

Resolve the supported Python call subset, build independent directed `petgraph` graphs, trace changed roots to callers, and expose a complete deterministic versioned JSON report through the CLI.

## Implementation

1. Build a snapshot-specific symbol/module index from all extracted files. Resolve direct lexical/module calls and explicit repository imports, including unambiguous aliases and relative imports with known package roots. Support `import package.module as alias` followed by calls through the known module alias.
2. Honor parameters, assignments, lexical binding rules, and ambiguity. Use conservative whole-scope exclusion when safe flow-sensitive resolution is unavailable. Calls through assigned objects, uncertain instance dispatch, inheritance, callable-changing decorators, wildcard/dynamic imports, or unknown module layouts remain unresolved. No global same-name fallback is allowed.
3. Mark every extracted call site resolved or unresolved with an enumerated reason and source expression. Missing repository targets, external targets, shadowing, dynamic targets, ambiguous bindings, unsupported dispatch, and invalid syntax need distinguishable explanations. Exclude unsafe parse-error evidence.
4. Build one directed graph per snapshot with symbol nodes and deduplicated caller-to-callee edges carrying sorted call-site IDs. Preserve module-call contexts explicitly, without treating them as user functions. Keep graph indices private; reports use stable symbol IDs.
5. For each changed root, select target for added/modified and base for deleted. Traverse incoming edges using bounded breadth-first search and a visited-distance map. Depth is minimum edge distance from the root; depth zero returns only the root. Preserve relevant edges and cycle/back references within the selected reachable subgraph, not only a spanning tree.
6. Build a serializable report with schema version 1, tool version, mode/options, completeness, snapshots, file changes, symbols, changes, call sites, edges, impact, and diagnostics. Publish a schema/field reference and representative examples as part of implementation documentation.
7. Document exact JSON field names and path representation. Use `path` for UTF-8 relative paths and a tagged byte/hex representation for non-UTF-8 paths; never emit a lossy path as authoritative identity. Every reference must resolve to an entity in the same report.
8. Assign deterministic IDs from sorted snapshot-role/path/scope/name/occurrence keys; deterministic collision suffixes are acceptable. Sort every serialized collection, including diagnostics and evidence. Do not serialize raw graph indices, randomized hash-map iteration, timestamps, or absolute checkout paths.
9. Wire `dflow ... --format json` end to end. Build output before writing so fatal errors do not leave misleading truncated JSON. Write operational errors to stderr. Serialize partial reports with `complete: false` and exit 1; unresolved calls alone permit complete success. Valid empty results exit 0.
10. Keep the in-memory analysis session richer than the serialized report: retain captured source and full per-snapshot graphs so M4 can adjust depth without reparsing or rereading files. Keep these objects independent of TUI state.

## Report and compatibility decisions

Use explicit base/target snapshot references everywhere evidence depends on source identity. Commit snapshots carry commit IDs and resolved branch names; local snapshots carry their captured content fingerprint and kind. Changes reference before/after symbol IDs as appropriate. Source ranges follow M2 conventions.

Publish JSON schema version 1 before M4 release. Within version 1, consumers must ignore unknown optional fields; incompatible required-field/meaning changes require a schema-version increment. Consumers reject unknown major schema versions with an actionable message. Do not claim IDs are stable across independent repository histories.

A complete report means supported files were analyzed without operational/parse failure; it does not mean every call was resolved. Excluded source categories and resolution limits remain documented. For modified roots, only target caller relationships participate in impact; before/after relationship comparison is backlog work.

## Test and completion checklist

- [ ] Resolve lexical/module calls, explicit aliases, known relative imports, recursion, and cross-file callers in unchanged files against independently specified targets.
- [ ] Never invent edges for parameters/assignments shadowing names, dynamic calls, wildcard imports, duplicate candidates, external symbols, or unsupported methods/decorators.
- [ ] Added roots with no callers remain valid; deleted roots and historical callers use only base evidence.
- [ ] Depth 0/1/larger values, diamonds, self recursion, mutual recursion, shared callers, and multiple call sites preserve distance and terminate.
- [ ] Each resolved edge references correct caller/callee/call-site entities in the same snapshot; unresolved sites have no certain edge.
- [ ] JSON schema examples cover all snapshot kinds, non-UTF-8 paths, partial reports, and empty results.
- [ ] Repeated runs over identical captures and different hash-map insertion order produce byte-identical JSON.
- [ ] CLI integration tests verify invalid input exit 2, analysis failure exit 1, complete/empty/unresolved-only exit 0, clean stdout, and stderr diagnostics.
- [ ] All four snapshot comparisons exercise the complete JSON pipeline, preserving M1 repository integrity.

Measure resolved-edge precision and resolution coverage separately; fixture precision must be 100%. A test must verify target identity and call location rather than merely counting edges. Handoff to M4: accepted JSON/report model and a reusable captured analysis session.
