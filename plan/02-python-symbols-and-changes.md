# M2 — Python extraction and symbol changes

Status: Implemented; Python symbol extraction and classification are ready for M3. Prerequisite: [M1](01-project-and-git-snapshots.md).  
Coverage: FR02–FR05, extraction portion of FR07, parse diagnostics in FR11.

## Deliverable

Parse both captured Python source contexts using Tree-sitter, extract symbols/imports/call sites, and classify definition changes. Resolve calls in M3; this milestone must retain enough lexical evidence to support conservative resolution.

## Implementation

1. Define internal `LanguageId`, `Symbol`, `SymbolKind`, `SourceRange`, `ImportBinding`, `Scope`, `Binding`, `ExtractedCallSite`, `ParsedFile`, and `SymbolChange` models. Keep module contexts separate from function symbols. Use snapshot-qualified stable ordering keys rather than parser-node addresses as report identity inputs.
2. Define an internal language adapter interface that accepts captured bytes plus path/package context and returns extracted models and diagnostics. Keep parsing/extraction separate from project-wide resolution. No dynamic loading or external language plugins are needed.
3. Configure `tree-sitter-python` and parse immutable bytes. Record parser error/missing-node ranges; do not use a tree containing errors as fully valid evidence. Recover unaffected files, but mark the report incomplete and omit unsafe edges from errored files downstream.
4. Extract synchronous and asynchronous functions, methods, classes as lexical scopes, and nested functions. Include decorated-definition ranges and decorator syntax in fingerprints while retaining the underlying function name. Preserve parameters, lexical parents, and exact one-based line/UTF-8 byte-column ranges with exclusive ends.
5. Extract every call expression, including nested expressions, attributes, and dynamic targets. Associate each call with its execution context: calls in defaults/decorators execute in the enclosing definition context, not the function body. Preserve callee syntax, source evidence, and scope even when resolution will be unsupported.
6. Record explicit imports, aliases, relative import levels, wildcard imports, assignments, parameters, and binding directives such as `global`/`nonlocal`. Record binding order where needed, and preserve ambiguous redefinitions instead of overwriting them.
7. Derive Python module names for regular packages rooted at the repository root or conventional `src/` root using captured `__init__.py` markers. Read relevant captured `pyproject.toml` metadata only when a supported static layout is recognizable. Do not execute packaging code. Namespace packages, custom import hooks, unknown configured roots, or conflicting module candidates stay unresolved with an explanation.
8. Match symbols by relative path, lexical scope, and name. For duplicate definitions under the same key, preserve all definitions and report ambiguity; conservatively represent unmatched occurrences as deletion/addition instead of guessing a pair.
9. Build canonical definition fingerprints from syntax node kinds and meaningful token text, excluding comments and whitespace-only formatting. Preserve literal token contents and docstrings. Include signature, decorators, and nested definitions. Hash canonical records, not raw source ranges or source text.
10. Classify matched symbols by fingerprint and unmatched symbols by snapshot presence. Emit file changes even when they contain no changed function definitions. Keep rename/move behavior as deletion/addition.

## Supported subset and diagnostics

P0 covers regular Python source files using syntax supported by the pinned grammar, with fixtures exercising functions, async definitions, classes, decorators, imports, and nested scopes. Before the Python release, document the tested Python syntax versions based on those fixtures; do not imply runtime compatibility merely because parsing succeeded.

Invalid encoding, unsupported syntax, and parser errors must carry path, snapshot, and range where available. Support UTF-8 and UTF-8 BOM initially; unsupported Python source encodings produce explicit incomplete diagnostics rather than corrupted source positions. Range conversion must retain offsets into the original captured bytes.

Changing an import or module-level statement alone remains a file-level change. It must not invent a modified function or imply that global semantic impact has been analyzed. Parse errors affecting a snapshot must prevent a confident deleted/added conclusion based solely on missing extraction from that failed file.

## Interfaces and handoff

`extract_snapshot(snapshot)` returns symbols, scopes, bindings, imports, call sites, and diagnostics keyed by snapshot/path. `classify_symbols(base, target)` returns ordered changes and references to both definitions where paired.

Retain enough source bytes/ranges for later call evidence and TUI details. Do not expose Tree-sitter nodes across owned pipeline stages or store node references past tree lifetime. Neither extraction nor matching knows about terminal rendering.

## Test and completion checklist

- [x] Functions, async functions, methods, decorators, nested functions/classes, qualified names, and module contexts are extracted correctly.
- [x] Calls in bodies, nested expressions, default arguments, and decorators receive the correct context; unsupported calls retain their expressions.
- [x] Explicit/relative imports, aliases, parameters, shadowing assignments, binding directives, and duplicate names are retained for M3.
- [x] Root packages and `src/` packages map consistently in both snapshots, including deleted package markers and ambiguous roots.
- [x] Comment/format-only changes leave definition classification unchanged; signature, literal, docstring, decorator, and nested-body changes do not.
- [x] Unicode identifiers, multiline expressions, BOM, exclusive ends, and unsupported encodings have verified locations and diagnostics.
- [x] Added/deleted files and functions, unchanged files, file-only changes, and unmatched moves are classified deterministically.
- [x] Broken syntax produces incomplete diagnostics without falsely treating every missing extracted symbol as deleted.
- [x] Tests use Python fixtures first and inherit the M1 integrity checks.

Handoff to M3: source-linked definitions and changes, full lexical extraction, canonical identities, and completeness diagnostics for both snapshots.
