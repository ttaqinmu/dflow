use crate::git::{CapturedSnapshot, SnapshotRole};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LanguageId {
    Python,
    Rust,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceUnitKey {
    pub path: Vec<u8>,
    pub target: String,
    pub module: String,
}
impl SourceUnitKey {
    pub fn python(path: &[u8]) -> Self {
        Self {
            path: path.to_vec(),
            target: String::new(),
            module: String::new(),
        }
    }
}
#[derive(Debug, Clone)]
pub struct DiscoveredUnit {
    pub key: SourceUnitKey,
    pub module: Option<String>,
    pub subtree: Option<(usize, usize)>,
    pub conditional: bool,
    pub unknown_attribute: bool,
}
#[derive(Debug, Default)]
pub struct Discovery {
    pub units: Vec<DiscoveredUnit>,
    pub diagnostics: Vec<Diagnostic>,
    pub incomplete: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    AsyncFunction,
    Method,
    AsyncMethod,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub byte_column: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRange {
    pub start: Position,
    pub end: Position,
    pub start_byte: usize,
    pub end_byte: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
    pub snapshot: SnapshotRole,
    pub path: Vec<u8>,
    pub range: SourceRange,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeKind {
    Module,
    Class,
    Function,
    Comprehension,
    RustType,
    RustTrait,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub id: usize,
    pub parent: Option<usize>,
    pub name: String,
    pub qualified_name: String,
    pub kind: ScopeKind,
    pub source: SourceRef,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub unit: SourceUnitKey,
    pub language: LanguageId,
    pub source: SourceRef,
    pub scope_id: usize,
    pub parent_scope: usize,
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub fingerprint: String,
    pub direct_fingerprint: String,
    pub non_call_fingerprint: String,
    pub decorated: bool,
    pub property: bool,
    pub conditional: bool,
    pub unknown_attribute: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingKind {
    Parameter,
    Assignment,
    Import,
    Global,
    Nonlocal,
    Definition,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub scope_id: usize,
    pub name: String,
    pub kind: BindingKind,
    pub source: SourceRef,
    pub order: usize,
    pub conditional: bool,
    pub unknown_attribute: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportBinding {
    pub scope_id: usize,
    pub module: String,
    pub name: Option<String>,
    pub alias: Option<String>,
    pub relative_level: usize,
    pub wildcard: bool,
    pub conditional: bool,
    pub unknown_attribute: bool,
    pub source: SourceRef,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedCallSite {
    pub scope_id: usize,
    pub expression: String,
    pub target: CallTarget,
    pub source: SourceRef,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallTarget {
    Name(String),
    Qualified(Vec<String>),
    Property { constructor: String, name: String },
    TypedMethod { constructor: String, name: String },
    Dynamic,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticKind {
    InvalidEncoding,
    ParseError,
    ModuleAmbiguity,
    UnsupportedLayout,
    DuplicateSymbol,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub message: String,
    pub snapshot: SnapshotRole,
    pub path: Vec<u8>,
    pub range: Option<SourceRange>,
}
#[derive(Debug, Clone)]
pub struct ParsedFile {
    pub unit: SourceUnitKey,
    pub path: Vec<u8>,
    pub module: Option<String>,
    pub language: LanguageId,
    pub valid: bool,
    pub scopes: Vec<Scope>,
    pub symbols: Vec<Symbol>,
    pub bindings: Vec<Binding>,
    pub imports: Vec<ImportBinding>,
    pub calls: Vec<ExtractedCallSite>,
    pub diagnostics: Vec<Diagnostic>,
}
#[derive(Debug)]
pub struct ExtractedSnapshot {
    pub role: SnapshotRole,
    pub files: BTreeMap<SourceUnitKey, ParsedFile>,
    pub diagnostics: Vec<Diagnostic>,
    pub complete: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolChangeKind {
    Added,
    Modified,
    Deleted,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolChange {
    pub kind: SymbolChangeKind,
    pub before: Option<Symbol>,
    pub after: Option<Symbol>,
}
#[derive(Debug)]
pub struct ChangeAnalysis {
    pub changes: Vec<SymbolChange>,
    pub diagnostics: Vec<Diagnostic>,
    pub complete: bool,
}

pub struct FingerprintBuilder(Sha256);

impl FingerprintBuilder {
    pub fn new() -> Self {
        Self(Sha256::new())
    }

    pub fn push(&mut self, part: &[u8]) {
        self.0.update((part.len() as u64).to_be_bytes());
        self.0.update(part);
    }

    pub fn finish(self) -> String {
        format!("{:x}", self.0.finalize())
    }
}

pub fn extract_snapshot(snapshot: &CapturedSnapshot, include_test: bool) -> ExtractedSnapshot {
    let mut files = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut complete = true;
    for adapter in crate::languages::adapters() {
        let discovery = adapter.discover(snapshot, include_test);
        complete &= !discovery.incomplete;
        diagnostics.extend(discovery.diagnostics);
        let mut covered = BTreeSet::new();
        for unit in discovery.units {
            let Some(file) = snapshot.files.get(&unit.key.path) else {
                complete = false;
                continue;
            };
            covered.insert(unit.key.path.clone());
            let parsed = adapter.extract(file, snapshot.role, &unit, include_test);
            complete &= parsed.valid;
            diagnostics.extend(parsed.diagnostics.clone());
            files.insert(unit.key, parsed);
        }
        for path in snapshot.files.keys().filter(|path| {
            adapter.accepts_source(path)
                && (include_test || !crate::languages::is_test_path(path))
                && !covered.contains(*path)
        }) {
            complete = false;
            diagnostics.push(Diagnostic {
                kind: DiagnosticKind::UnsupportedLayout,
                message: "Source unit could not be discovered".into(),
                snapshot: snapshot.role,
                path: path.clone(),
                range: None,
            });
        }
    }
    ExtractedSnapshot {
        role: snapshot.role,
        files,
        diagnostics,
        complete,
    }
}

pub fn classify_symbols(base: &ExtractedSnapshot, target: &ExtractedSnapshot) -> ChangeAnalysis {
    let mut changes = Vec::new();
    let mut diagnostics = Vec::new();
    let units: BTreeSet<_> = base
        .files
        .keys()
        .chain(target.files.keys())
        .cloned()
        .collect();
    for unit in units {
        let before = base.files.get(&unit);
        let after = target.files.get(&unit);
        if before.is_some_and(|f| !f.valid) || after.is_some_and(|f| !f.valid) {
            continue;
        }
        type Key = (LanguageId, String, String);
        let group = |file: Option<&ParsedFile>| -> BTreeMap<Key, Vec<Symbol>> {
            let mut groups: BTreeMap<Key, Vec<Symbol>> = BTreeMap::new();
            if let Some(file) = file {
                for symbol in &file.symbols {
                    groups
                        .entry((
                            symbol.language,
                            symbol.qualified_name.clone(),
                            symbol.name.clone(),
                        ))
                        .or_default()
                        .push(symbol.clone());
                }
            }
            groups
        };
        let b = group(before);
        let a = group(after);
        let keys: BTreeSet<_> = b.keys().chain(a.keys()).cloned().collect();
        for key in keys {
            let bs = b.get(&key).map(Vec::as_slice).unwrap_or(&[]);
            let as_ = a.get(&key).map(Vec::as_slice).unwrap_or(&[]);
            if bs.len() > 1 || as_.len() > 1 {
                diagnostics.push(Diagnostic {
                    kind: DiagnosticKind::DuplicateSymbol,
                    message: format!("ambiguous duplicate symbol: {}", key.1),
                    snapshot: if as_.len() > 1 {
                        target.role
                    } else {
                        base.role
                    },
                    path: unit.path.clone(),
                    range: None,
                });
                if bs.len() > 1 && as_.len() > 1 && bs.len() == as_.len() {
                    let same_order = bs.iter().zip(as_).all(|(before, after)| {
                        before.fingerprint == after.fingerprint && before.kind == after.kind
                    });
                    if same_order {
                        continue;
                    }
                    for symbol in bs {
                        changes.push(SymbolChange {
                            kind: SymbolChangeKind::Deleted,
                            before: Some(symbol.clone()),
                            after: None,
                        });
                    }
                    for symbol in as_ {
                        changes.push(SymbolChange {
                            kind: SymbolChangeKind::Added,
                            before: None,
                            after: Some(symbol.clone()),
                        });
                    }
                    continue;
                }
                let mut unmatched_after = as_.to_vec();
                let mut unmatched_before = Vec::new();
                for s in bs {
                    if let Some(index) = unmatched_after
                        .iter()
                        .position(|a| a.fingerprint == s.fingerprint && a.kind == s.kind)
                    {
                        unmatched_after.remove(index);
                    } else {
                        unmatched_before.push(s);
                    }
                }
                for s in unmatched_before {
                    changes.push(SymbolChange {
                        kind: SymbolChangeKind::Deleted,
                        before: Some(s.clone()),
                        after: None,
                    });
                }
                for s in unmatched_after {
                    changes.push(SymbolChange {
                        kind: SymbolChangeKind::Added,
                        before: None,
                        after: Some(s),
                    });
                }
                continue;
            }
            match (bs.first(), as_.first()) {
                (Some(b), Some(a)) if b.direct_fingerprint != a.direct_fingerprint => {
                    changes.push(SymbolChange {
                        kind: SymbolChangeKind::Modified,
                        before: Some(b.clone()),
                        after: Some(a.clone()),
                    })
                }
                (Some(b), None) => changes.push(SymbolChange {
                    kind: SymbolChangeKind::Deleted,
                    before: Some(b.clone()),
                    after: None,
                }),
                (None, Some(a)) => changes.push(SymbolChange {
                    kind: SymbolChangeKind::Added,
                    before: None,
                    after: Some(a.clone()),
                }),
                _ => {}
            }
        }
    }
    ChangeAnalysis {
        changes,
        diagnostics,
        complete: base.complete && target.complete,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{CapturedFile, SnapshotKind};
    fn snapshot(role: SnapshotRole, files: &[(&[u8], &[u8])]) -> CapturedSnapshot {
        let files = files
            .iter()
            .map(|(path, bytes)| {
                (
                    path.to_vec(),
                    CapturedFile {
                        path: path.to_vec(),
                        bytes: bytes.to_vec(),
                        mode: 0o100644,
                    },
                )
            })
            .collect();
        CapturedSnapshot {
            kind: SnapshotKind::WorkingTree,
            role,
            commit_id: None,
            branch: None,
            files,
            content_id: String::new(),
        }
    }
    #[test]
    fn extracts_functions_scopes_calls_and_imports() {
        let src = include_bytes!("../../../dflow/tests/fixtures/python/symbols.py");
        let snap = snapshot(
            SnapshotRole::Target,
            &[
                (b"pkg/module.py", src),
                (b"pkg/__init__.py", b""),
                (b"pkg/tools.py", b""),
            ],
        );
        let result = extract_snapshot(&snap, true);
        let file = result
            .files
            .get(&SourceUnitKey::python(b"pkg/module.py"))
            .unwrap();
        assert!(file.valid);
        assert_eq!(file.module.as_deref(), Some("pkg.module"));
        let names: Vec<_> = file
            .symbols
            .iter()
            .map(|s| s.qualified_name.as_str())
            .collect();
        assert_eq!(names, ["process", "process.inner", "process.Local.method"]);
        assert_eq!(file.symbols[0].kind, SymbolKind::AsyncFunction);
        assert_eq!(file.symbols[2].kind, SymbolKind::Method);
        assert_eq!(file.imports.len(), 2);
        assert_eq!(file.imports[1].relative_level, 1);
        assert_eq!(file.imports[1].alias.as_deref(), Some("send"));
        let outer = file.symbols[0].parent_scope;
        assert!(
            file.calls
                .iter()
                .any(|c| c.expression == "factory" && c.scope_id == outer)
        );
        assert!(
            file.calls
                .iter()
                .any(|c| c.expression == "default_call" && c.scope_id == outer)
        );
        assert!(
            file.calls
                .iter()
                .any(|c| c.expression == "inner" && c.scope_id == file.symbols[2].scope_id)
        );
    }
    #[test]
    fn formatting_and_comments_do_not_change_symbol() {
        let before = snapshot(
            SnapshotRole::Base,
            &[(b"a.py", b"def f():\n    # comment\n    return 1\n")],
        );
        let after = snapshot(
            SnapshotRole::Target,
            &[(b"a.py", b"def f( ):\n\n    return 1 # other\n")],
        );
        let changes = classify_symbols(
            &extract_snapshot(&before, true),
            &extract_snapshot(&after, true),
        );
        assert!(changes.changes.is_empty());
    }
    #[test]
    fn literal_and_nested_body_changes_are_detected() {
        let before = snapshot(
            SnapshotRole::Base,
            &[(
                b"a.py",
                b"def outer():\n    def inner():\n        return 1\n",
            )],
        );
        let after = snapshot(
            SnapshotRole::Target,
            &[(
                b"a.py",
                b"def outer():\n    def inner():\n        return 2\n",
            )],
        );
        let changes = classify_symbols(
            &extract_snapshot(&before, true),
            &extract_snapshot(&after, true),
        );
        assert_eq!(changes.changes.len(), 1);
        assert_eq!(changes.changes[0].kind, SymbolChangeKind::Modified);
        assert_eq!(changes.changes[0].after.as_ref().unwrap().name, "inner");
    }
    #[test]
    fn python_direct_and_non_call_fingerprints_separate_changes() {
        let symbol = |source: &'static [u8]| {
            extract_snapshot(&snapshot(SnapshotRole::Base, &[(b"a.py", source)]), true).files
                [&SourceUnitKey::python(b"a.py")]
                .symbols[0]
                .clone()
        };
        let original =
            symbol(b"def f(x=1):\n    \"doc\"\n    def inner(): return 1\n    return B(x)\n");
        let nested =
            symbol(b"def f(x=1):\n    \"doc\"\n    def inner(): return 2\n    return B(x)\n");
        assert_eq!(original.direct_fingerprint, nested.direct_fingerprint);
        let retarget =
            symbol(b"def f(x=1):\n    \"doc\"\n    def inner(): return 1\n    return C(x)\n");
        assert_ne!(original.direct_fingerprint, retarget.direct_fingerprint);
        assert_eq!(original.non_call_fingerprint, retarget.non_call_fingerprint);
        for changed in [
            b"def f(x=2):\n    \"doc\"\n    def inner(): return 1\n    return B(x)\n".as_slice(),
            b"def f(x=1):\n    \"new doc\"\n    def inner(): return 1\n    return B(x)\n",
            b"def f(x=1):\n    \"doc\"\n    def inner(): return 1\n    return B(2)\n",
            b"def f(x=1):\n    \"doc\"\n    def inner(): return 1\n    B(x)\n",
        ] {
            assert_ne!(
                original.non_call_fingerprint,
                symbol(changed).non_call_fingerprint
            );
        }
        assert_eq!(original.direct_fingerprint, symbol(b"def f( x = 1 ):\n    \"doc\"\n    def inner(): return 1\n    return B(x) # layout\n").direct_fingerprint);
    }
    #[test]
    fn broken_source_does_not_fabricate_deletions() {
        let before = snapshot(SnapshotRole::Base, &[(b"a.py", b"def kept():\n    pass\n")]);
        let after = snapshot(
            SnapshotRole::Target,
            &[(b"a.py", b"def kept(:\n    pass\n")],
        );
        let target = extract_snapshot(&after, true);
        assert!(!target.complete);
        assert!(
            target
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::ParseError)
        );
        let changes = classify_symbols(&extract_snapshot(&before, true), &target);
        assert!(!changes.complete);
        assert!(changes.changes.is_empty());
    }
    #[test]
    fn encoding_and_module_layout_are_explicit() {
        let snap = snapshot(
            SnapshotRole::Target,
            &[
                (b"src/pkg/__init__.py", b""),
                (b"src/pkg/m.py", b"\xef\xbb\xbfdef f():\n    pass\n"),
                (b"bad.py", b"\xff"),
            ],
        );
        let result = extract_snapshot(&snap, true);
        assert_eq!(
            result
                .files
                .get(&SourceUnitKey::python(b"src/pkg/m.py"))
                .unwrap()
                .module
                .as_deref(),
            Some("pkg.m")
        );
        assert!(
            result
                .files
                .get(&SourceUnitKey::python(b"src/pkg/m.py"))
                .unwrap()
                .valid
        );
        assert!(
            !result
                .files
                .get(&SourceUnitKey::python(b"bad.py"))
                .unwrap()
                .valid
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::InvalidEncoding)
        );
    }
    #[test]
    fn classifies_added_deleted_and_unchanged_files() {
        let base = snapshot(
            SnapshotRole::Base,
            &[
                (b"old.py", b"def removed():\n    pass\n"),
                (b"same.py", b"def kept():\n    pass\n"),
            ],
        );
        let target = snapshot(
            SnapshotRole::Target,
            &[
                (b"new.py", b"def added():\n    pass\n"),
                (b"same.py", b"def kept():\n    pass\n"),
            ],
        );
        let result = classify_symbols(
            &extract_snapshot(&base, true),
            &extract_snapshot(&target, true),
        );
        assert_eq!(result.changes.len(), 2);
        assert_eq!(result.changes[0].kind, SymbolChangeKind::Added);
        assert_eq!(result.changes[1].kind, SymbolChangeKind::Deleted);
    }
    #[test]
    fn import_only_change_does_not_modify_function() {
        let base = snapshot(
            SnapshotRole::Base,
            &[(b"a.py", b"import old\ndef f():\n    return 1\n")],
        );
        let target = snapshot(
            SnapshotRole::Target,
            &[(b"a.py", b"import new\ndef f():\n    return 1\n")],
        );
        assert!(
            classify_symbols(
                &extract_snapshot(&base, true),
                &extract_snapshot(&target, true)
            )
            .changes
            .is_empty()
        );
    }
    #[test]
    fn bindings_and_call_ranges_are_retained() {
        let src = "def outer(π):\n    global x\n    local = helper(π)\n    return local\n";
        let snap = snapshot(SnapshotRole::Target, &[(b"a.py", src.as_bytes())]);
        let result = extract_snapshot(&snap, true);
        let file = result.files.get(&SourceUnitKey::python(b"a.py")).unwrap();
        assert!(
            file.bindings
                .iter()
                .any(|b| b.name == "π" && b.kind == BindingKind::Parameter)
        );
        assert!(
            file.bindings
                .iter()
                .any(|b| b.name == "x" && b.kind == BindingKind::Global)
        );
        assert!(
            file.bindings
                .iter()
                .any(|b| b.name == "local" && b.kind == BindingKind::Assignment)
        );
        let call = file
            .calls
            .iter()
            .find(|c| c.expression == "helper")
            .unwrap();
        assert_eq!(call.source.range.start.line, 3);
        assert_eq!(call.source.range.start.byte_column, 13);
    }
    #[test]
    fn duplicate_definition_is_ambiguous() {
        let base = snapshot(
            SnapshotRole::Base,
            &[(b"a.py", b"def f(): pass\ndef f(): pass\n")],
        );
        let target = snapshot(SnapshotRole::Target, &[(b"a.py", b"def f(): pass\n")]);
        let result = classify_symbols(
            &extract_snapshot(&base, true),
            &extract_snapshot(&target, true),
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::DuplicateSymbol)
        );
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, SymbolChangeKind::Deleted);
    }
    #[test]
    fn unchanged_duplicate_definitions_have_no_changes() {
        let source = b"def f(): return 1\ndef f(): return 2\n";
        let base = snapshot(SnapshotRole::Base, &[(b"a.py", source)]);
        let target = snapshot(SnapshotRole::Target, &[(b"a.py", source)]);
        let result = classify_symbols(
            &extract_snapshot(&base, true),
            &extract_snapshot(&target, true),
        );
        assert!(result.changes.is_empty());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::DuplicateSymbol)
        );
    }
    #[test]
    fn imports_follow_syntax_nodes_and_preserve_relative_members() {
        let source = b"from . import foo\nfrom pkg import\tbar as baz\nfrom pkg import(\n    qux, # ignored\n    zip as zap,\n)\nimport alpha.beta as ab, gamma\n";
        let snap = snapshot(SnapshotRole::Target, &[(b"a.py", source)]);
        let result = extract_snapshot(&snap, true);
        let file = result.files.get(&SourceUnitKey::python(b"a.py")).unwrap();
        assert!(file.valid);
        assert_eq!(file.imports.len(), 6);
        assert_eq!(
            (
                file.imports[0].module.as_str(),
                file.imports[0].name.as_deref(),
                file.imports[0].relative_level
            ),
            ("", Some("foo"), 1)
        );
        assert_eq!(
            (
                file.imports[1].module.as_str(),
                file.imports[1].name.as_deref(),
                file.imports[1].alias.as_deref()
            ),
            ("pkg", Some("bar"), Some("baz"))
        );
        assert_eq!(file.imports[2].name.as_deref(), Some("qux"));
        assert_eq!(file.imports[3].name.as_deref(), Some("zip"));
        assert_eq!(file.imports[4].module, "alpha.beta");
        assert_eq!(file.imports[4].alias.as_deref(), Some("ab"));
        assert!(
            file.bindings
                .iter()
                .any(|b| b.name == "foo" && b.kind == BindingKind::Import)
        );
        assert!(!file.bindings.iter().any(|b| b.name.contains("ignored")));
    }
    #[test]
    fn loop_targets_bind_in_containing_scope() {
        let source =
            b"def f(): pass\ndef outer(values):\n    for f, (x, y) in values:\n        f()\n";
        let snap = snapshot(SnapshotRole::Target, &[(b"a.py", source)]);
        let result = extract_snapshot(&snap, true);
        let file = result.files.get(&SourceUnitKey::python(b"a.py")).unwrap();
        let outer = file.symbols.iter().find(|s| s.name == "outer").unwrap();
        for name in ["f", "x", "y"] {
            assert!(file.bindings.iter().any(|b| b.scope_id == outer.scope_id
                && b.name == name
                && b.kind == BindingKind::Assignment));
        }
    }
    #[test]
    fn attribute_write_does_not_bind_receiver() {
        let snap = snapshot(
            SnapshotRole::Target,
            &[(
                b"a.py",
                b"def f(obj):\n    obj.field = 1\n    x, y = pair()\n",
            )],
        );
        let result = extract_snapshot(&snap, true);
        let file = result.files.get(&SourceUnitKey::python(b"a.py")).unwrap();
        assert_eq!(file.bindings.iter().filter(|b| b.name == "obj").count(), 1);
        assert!(
            file.bindings
                .iter()
                .any(|b| b.name == "x" && b.kind == BindingKind::Assignment)
        );
        assert!(
            file.bindings
                .iter()
                .any(|b| b.name == "y" && b.kind == BindingKind::Assignment)
        );
    }
    #[test]
    fn static_pyproject_source_root_resolves_ambiguity() {
        let code = b"def f(): pass\n";
        let files: &[(&[u8], &[u8])] = &[
            (b"src/__init__.py", b""),
            (b"src/pkg/__init__.py", b""),
            (b"src/pkg/m.py", code),
        ];
        let ambiguous = extract_snapshot(&snapshot(SnapshotRole::Target, files), true);
        assert!(
            ambiguous
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::ModuleAmbiguity)
        );
        let configured = snapshot(
            SnapshotRole::Target,
            &[
                (
                    b"pyproject.toml",
                    b"[tool.setuptools.package-dir]\n\"\" = \"src\"\n",
                ),
                (b"src/__init__.py", b""),
                (b"src/pkg/__init__.py", b""),
                (b"src/pkg/m.py", code),
            ],
        );
        let resolved = extract_snapshot(&configured, true);
        assert_eq!(
            resolved
                .files
                .get(&SourceUnitKey::python(b"src/pkg/m.py"))
                .unwrap()
                .module
                .as_deref(),
            Some("pkg.m")
        );
        assert!(
            !resolved
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::ModuleAmbiguity)
        );
    }
    #[test]
    fn unsupported_custom_source_root_stays_unresolved() {
        let snap = snapshot(
            SnapshotRole::Target,
            &[
                (
                    b"pyproject.toml",
                    b"[tool.setuptools.package-dir]\n\"\" = \"lib\"\n",
                ),
                (b"lib/pkg/__init__.py", b""),
                (b"lib/pkg/m.py", b"def f(): pass\n"),
            ],
        );
        let result = extract_snapshot(&snap, true);
        assert!(
            result
                .files
                .get(&SourceUnitKey::python(b"lib/pkg/m.py"))
                .unwrap()
                .module
                .is_none()
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::UnsupportedLayout)
        );
    }
    #[test]
    fn deleted_package_marker_changes_module_identity() {
        let base = snapshot(
            SnapshotRole::Base,
            &[(b"pkg/__init__.py", b""), (b"pkg/m.py", b"def f(): pass\n")],
        );
        let target = snapshot(SnapshotRole::Target, &[(b"pkg/m.py", b"def f(): pass\n")]);
        assert_eq!(
            extract_snapshot(&base, true)
                .files
                .get(&SourceUnitKey::python(b"pkg/m.py"))
                .unwrap()
                .module
                .as_deref(),
            Some("pkg.m")
        );
        let target_result = extract_snapshot(&target, true);
        assert!(
            target_result
                .files
                .get(&SourceUnitKey::python(b"pkg/m.py"))
                .unwrap()
                .module
                .is_none()
        );
        assert!(
            target_result
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::UnsupportedLayout)
        );
    }
    #[test]
    fn with_except_walrus_and_comprehension_bindings_are_recorded() {
        let code = b"def f(xs):\n    with acquire() as resource:\n        resource()\n    try:\n        pass\n    except Error as caught:\n        caught()\n    if (chosen := pick()):\n        chosen()\n    return [item() for item in xs]\n";
        let snap = snapshot(SnapshotRole::Target, &[(b"a.py", code)]);
        let result = extract_snapshot(&snap, true);
        let file = result.files.get(&SourceUnitKey::python(b"a.py")).unwrap();
        let f = file.symbols.iter().find(|s| s.name == "f").unwrap();
        for name in ["resource", "caught", "chosen"] {
            assert!(
                file.bindings.iter().any(|b| b.scope_id == f.scope_id
                    && b.name == name
                    && b.kind == BindingKind::Assignment),
                "missing {name}"
            );
        }
        let comprehension = file
            .scopes
            .iter()
            .find(|s| s.kind == ScopeKind::Comprehension)
            .unwrap();
        assert!(
            file.bindings
                .iter()
                .any(|b| b.scope_id == comprehension.id && b.name == "item")
        );
        assert!(
            !file
                .bindings
                .iter()
                .any(|b| b.scope_id == f.scope_id && b.name == "item")
        );
    }
    #[test]
    fn duplicate_reorder_is_reported_as_change() {
        let base = snapshot(
            SnapshotRole::Base,
            &[(b"a.py", b"def f(): return 1\ndef f(): return 2\n")],
        );
        let target = snapshot(
            SnapshotRole::Target,
            &[(b"a.py", b"def f(): return 2\ndef f(): return 1\n")],
        );
        let result = classify_symbols(
            &extract_snapshot(&base, true),
            &extract_snapshot(&target, true),
        );
        assert_eq!(result.changes.len(), 4);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::DuplicateSymbol)
        );
    }
    #[test]
    fn signature_docstring_and_multiline_ranges() {
        let base = snapshot(
            SnapshotRole::Base,
            &[(b"a.py", b"def f(x):\n    \"old\"\n    return x\n")],
        );
        let target = snapshot(
            SnapshotRole::Target,
            &[(
                b"a.py",
                b"def f(\n    x, y\n):\n    \"new\"\n    return x\n",
            )],
        );
        let result = classify_symbols(
            &extract_snapshot(&base, true),
            &extract_snapshot(&target, true),
        );
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, SymbolChangeKind::Modified);
        let range = result.changes[0].after.as_ref().unwrap().source.range;
        assert_eq!(range.start.line, 1);
        assert_eq!(range.end.line, 5);
        assert_eq!(range.end.byte_column, 13);
    }

    #[test]
    fn walrus_inside_comprehension_binds_in_enclosing_function() {
        let snap = snapshot(
            SnapshotRole::Target,
            &[(
                b"a.py",
                b"def f(xs):\n    return [(chosen := item()) for item in xs]\n",
            )],
        );
        let result = extract_snapshot(&snap, true);
        let file = result.files.get(&SourceUnitKey::python(b"a.py")).unwrap();
        assert!(file.valid);
        let f = file.symbols.iter().find(|s| s.name == "f").unwrap();
        let comprehension = file
            .scopes
            .iter()
            .find(|s| s.kind == ScopeKind::Comprehension)
            .unwrap();
        assert!(
            file.bindings
                .iter()
                .any(|b| b.scope_id == f.scope_id && b.name == "chosen")
        );
        assert!(
            file.bindings
                .iter()
                .any(|b| b.scope_id == comprehension.id && b.name == "item")
        );
    }
    #[test]
    fn docstring_only_change_marks_definition_modified() {
        let base = snapshot(SnapshotRole::Base, &[(b"a.py", b"def f():\n    \"old\"\n")]);
        let target = snapshot(
            SnapshotRole::Target,
            &[(b"a.py", b"def f():\n    \"new\"\n")],
        );
        let result = classify_symbols(
            &extract_snapshot(&base, true),
            &extract_snapshot(&target, true),
        );
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, SymbolChangeKind::Modified);
    }

    #[test]
    fn decorator_change_and_unmatched_move_are_classified() {
        let base = snapshot(SnapshotRole::Base, &[(b"old.py", b"@old\ndef f(): pass\n")]);
        let target = snapshot(
            SnapshotRole::Target,
            &[(b"new.py", b"@new\ndef f(): pass\n")],
        );
        let result = classify_symbols(
            &extract_snapshot(&base, true),
            &extract_snapshot(&target, true),
        );
        assert_eq!(result.changes.len(), 2);
        assert!(
            result
                .changes
                .iter()
                .any(|c| c.kind == SymbolChangeKind::Deleted)
        );
        assert!(
            result
                .changes
                .iter()
                .any(|c| c.kind == SymbolChangeKind::Added)
        );
        let base = snapshot(SnapshotRole::Base, &[(b"a.py", b"@old\ndef f(): pass\n")]);
        let target = snapshot(SnapshotRole::Target, &[(b"a.py", b"@new\ndef f(): pass\n")]);
        let result = classify_symbols(
            &extract_snapshot(&base, true),
            &extract_snapshot(&target, true),
        );
        assert_eq!(result.changes[0].kind, SymbolChangeKind::Modified);
    }
    #[test]
    fn bom_offsets_remain_in_original_source_bytes() {
        let snap = snapshot(
            SnapshotRole::Target,
            &[(b"a.py", b"\xef\xbb\xbfdef f():\n    pass\n")],
        );
        let result = extract_snapshot(&snap, true);
        let file = result.files.get(&SourceUnitKey::python(b"a.py")).unwrap();
        let range = file.symbols[0].source.range;
        assert_eq!(range.start_byte, 3);
        assert_eq!(range.start.byte_column, 4);
    }

    #[test]
    fn nonlocal_and_nested_class_scope_are_retained() {
        let snap = snapshot(SnapshotRole::Target, &[(b"a.py", b"def outer():\n    x = 1\n    def inner():\n        nonlocal x\n        class Local:\n            async def method(self):\n                return x\n")]);
        let result = extract_snapshot(&snap, true);
        let file = result.files.get(&SourceUnitKey::python(b"a.py")).unwrap();
        let inner = file
            .symbols
            .iter()
            .find(|s| s.qualified_name == "outer.inner")
            .unwrap();
        assert!(file.bindings.iter().any(|b| b.scope_id == inner.scope_id
            && b.name == "x"
            && b.kind == BindingKind::Nonlocal));
        assert!(
            file.symbols
                .iter()
                .any(|s| s.qualified_name == "outer.inner.Local.method"
                    && s.kind == SymbolKind::AsyncMethod)
        );
    }
    #[test]
    fn rust_unit_identity_matches_only_same_target_module_and_tracks_moves() {
        let manifest = (
            b"Cargo.toml".as_slice(),
            b"[package]\nname='changes'\nversion='0.1.0'\n".as_slice(),
        );
        let base = extract_snapshot(
            &snapshot(
                SnapshotRole::Base,
                &[
                    manifest,
                    (b"src/lib.rs", b"mod source;\nmod destination;\n".as_slice()),
                    (
                        b"src/source.rs",
                        b"pub fn moved() -> u32 { 1 }\n".as_slice(),
                    ),
                    (b"src/destination.rs", b"".as_slice()),
                ],
            ),
            true,
        );
        let target = extract_snapshot(
            &snapshot(
                SnapshotRole::Target,
                &[
                    manifest,
                    (b"src/lib.rs", b"mod source;\nmod destination;\n".as_slice()),
                    (b"src/source.rs", b"".as_slice()),
                    (
                        b"src/destination.rs",
                        b"pub fn moved() -> u32 { 1 }\n".as_slice(),
                    ),
                ],
            ),
            true,
        );
        let changes = classify_symbols(&base, &target);
        assert!(changes.complete);
        assert_eq!(changes.changes.len(), 2);
        assert!(
            changes
                .changes
                .iter()
                .any(|c| c.kind == SymbolChangeKind::Deleted
                    && c.before.as_ref().unwrap().unit.module == "crate::source")
        );
        assert!(
            changes
                .changes
                .iter()
                .any(|c| c.kind == SymbolChangeKind::Added
                    && c.after.as_ref().unwrap().unit.module == "crate::destination")
        );
    }

    #[test]
    fn rust_source_errors_are_partial_but_unresolved_calls_are_complete() {
        let manifest = (
            b"Cargo.toml".as_slice(),
            b"[package]\nname='errors'\nversion='0.1.0'\n".as_slice(),
        );
        let malformed = extract_snapshot(
            &snapshot(
                SnapshotRole::Target,
                &[manifest, (b"src/lib.rs", b"fn broken( {\n".as_slice())],
            ),
            true,
        );
        assert!(!malformed.complete);
        assert!(
            malformed
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::ParseError)
        );
        let invalid = extract_snapshot(
            &snapshot(
                SnapshotRole::Target,
                &[manifest, (b"src/lib.rs", b"fn broken() {}\xff".as_slice())],
            ),
            true,
        );
        assert!(!invalid.complete);
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::InvalidEncoding)
        );
        let uncertain = extract_snapshot(
            &snapshot(
                SnapshotRole::Target,
                &[
                    manifest,
                    (b"src/lib.rs", b"pub fn caller() { unknown(); }".as_slice()),
                ],
            ),
            true,
        );
        assert!(uncertain.complete);
    }
}
