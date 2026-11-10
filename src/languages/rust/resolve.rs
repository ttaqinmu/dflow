use crate::analysis::{
    BindingKind, CallTarget, ExtractedCallSite, ExtractedSnapshot, ImportBinding, LanguageId,
    ParsedFile, ScopeKind, Symbol, SymbolKind,
};
use crate::languages::{Resolution, UnresolvedReason};
use std::collections::BTreeMap;

/// Only captured units of the same Cargo target can contribute a call target.
pub struct RustIndex<'a> {
    targets: BTreeMap<&'a str, BTreeMap<&'a str, Vec<&'a ParsedFile>>>,
}

impl<'a> RustIndex<'a> {
    pub fn new(snapshot: &'a ExtractedSnapshot) -> Self {
        let mut targets: BTreeMap<&str, BTreeMap<&str, Vec<&ParsedFile>>> = BTreeMap::new();
        for file in snapshot
            .files
            .values()
            .filter(|file| file.valid && file.language == LanguageId::Rust)
        {
            targets
                .entry(&file.unit.target)
                .or_default()
                .entry(&file.unit.module)
                .or_default()
                .push(file);
        }
        Self { targets }
    }

    fn module_files(&self, target: &str, module: &str) -> Option<&Vec<&'a ParsedFile>> {
        self.targets.get(target)?.get(module)
    }

    fn safe_symbol(symbol: &'a Symbol) -> Resolution<'a> {
        if symbol.conditional {
            Resolution::Unresolved(UnresolvedReason::ConditionalCompilation)
        } else if symbol.unknown_attribute {
            Resolution::Unresolved(UnresolvedReason::UnsupportedDispatch)
        } else {
            Resolution::Resolved(symbol)
        }
    }

    /// Resolve a name at an exact module, without following imported/re-exported names.
    fn module_function(&self, target: &str, module: &str, name: &str) -> Resolution<'a> {
        let Some(files) = self.module_files(target, module) else {
            return Resolution::Unresolved(UnresolvedReason::UnknownModule);
        };
        if files.len() != 1 {
            return Resolution::Unresolved(UnresolvedReason::AmbiguousTarget);
        }
        let file = files[0];
        if file.imports.iter().any(|i| i.scope_id == 0 && i.wildcard) {
            return Resolution::Unresolved(UnresolvedReason::WildcardImport);
        }
        let bindings: Vec<_> = file
            .bindings
            .iter()
            .filter(|binding| binding.scope_id == 0 && binding.name == name)
            .collect();
        if bindings.len() > 1 {
            return Resolution::Unresolved(UnresolvedReason::AmbiguousBinding);
        }
        if let Some(binding) = bindings.first() {
            if binding.kind == BindingKind::Import {
                return Resolution::Unresolved(UnresolvedReason::UnsupportedDispatch);
            }
            if binding.kind != BindingKind::Definition {
                return Resolution::Unresolved(UnresolvedReason::Shadowed);
            }
        }
        let symbols: Vec<_> = file
            .symbols
            .iter()
            .filter(|symbol| symbol.parent_scope == 0 && symbol.name == name)
            .collect();
        match symbols.as_slice() {
            [symbol] => Self::safe_symbol(symbol),
            [] => Resolution::Unresolved(UnresolvedReason::MissingTarget),
            _ => Resolution::Unresolved(UnresolvedReason::AmbiguousTarget),
        }
    }

    /// Inherent dispatch requires a unique type/impl and a unique method. Trait
    /// implementations and method calls through a receiver have no static proof.
    fn inherent_method(&self, target: &str, module: &str, ty: &str, name: &str) -> Resolution<'a> {
        let Some(files) = self.module_files(target, module) else {
            return Resolution::Unresolved(UnresolvedReason::UnknownModule);
        };
        if files.len() != 1 {
            return Resolution::Unresolved(UnresolvedReason::AmbiguousTarget);
        }
        let file = files[0];
        if file.scopes.iter().any(|scope| {
            scope.kind == ScopeKind::RustTrait && scope.name == ty && scope.parent == Some(0)
        }) {
            return Resolution::Unresolved(UnresolvedReason::UnsupportedDispatch);
        }
        let type_bindings = file
            .bindings
            .iter()
            .filter(|binding| binding.scope_id == 0 && binding.name == ty)
            .collect::<Vec<_>>();
        if type_bindings.len() != 1 || type_bindings[0].kind != BindingKind::Definition {
            return Resolution::Unresolved(if type_bindings.len() > 1 {
                UnresolvedReason::AmbiguousBinding
            } else {
                UnresolvedReason::UnsupportedDispatch
            });
        }
        if type_bindings[0].conditional {
            return Resolution::Unresolved(UnresolvedReason::ConditionalCompilation);
        }
        if type_bindings[0].unknown_attribute {
            return Resolution::Unresolved(UnresolvedReason::UnsupportedDispatch);
        }
        let implementations: Vec<_> = file
            .scopes
            .iter()
            .filter(|scope| {
                scope.kind == ScopeKind::RustType
                    && scope.name == ty
                    && scope
                        .parent
                        .is_some_and(|parent| file.scopes[parent].kind == ScopeKind::Module)
            })
            .collect();
        let [implementation] = implementations.as_slice() else {
            return Resolution::Unresolved(if implementations.is_empty() {
                UnresolvedReason::UnsupportedDispatch
            } else {
                UnresolvedReason::AmbiguousTarget
            });
        };
        if file.imports.iter().any(|i| i.scope_id == 0 && i.wildcard) {
            return Resolution::Unresolved(UnresolvedReason::WildcardImport);
        }
        let methods: Vec<_> = file
            .symbols
            .iter()
            .filter(|s| {
                s.parent_scope == implementation.id
                    && s.name == name
                    && matches!(s.kind, SymbolKind::Method | SymbolKind::AsyncMethod)
            })
            .collect();
        match methods.as_slice() {
            [method] => Self::safe_symbol(method),
            [] => Resolution::Unresolved(UnresolvedReason::MissingTarget),
            _ => Resolution::Unresolved(UnresolvedReason::AmbiguousTarget),
        }
    }

    fn absolute_path(&self, file: &'a ParsedFile, path: &[&str]) -> Resolution<'a> {
        if path.is_empty() {
            return Resolution::Unresolved(UnresolvedReason::DynamicTarget);
        }
        let mut module = match path[0] {
            "crate" => "crate".to_owned(),
            "self" | "super" => file.unit.module.clone(),
            _ => file.unit.module.clone(),
        };
        let mut index = match path[0] {
            "crate" | "self" => 1,
            "super" => 0,
            _ => 0,
        };
        while path.get(index) == Some(&"super") {
            if module == "crate" {
                return Resolution::Unresolved(UnresolvedReason::UnknownModule);
            }
            let Some((parent, _)) = module.rsplit_once("::") else {
                return Resolution::Unresolved(UnresolvedReason::UnknownModule);
            };
            module.truncate(parent.len());
            index += 1;
        }
        if index == path.len() {
            return Resolution::Unresolved(UnresolvedReason::UnsupportedDispatch);
        }
        for component in &path[index..path.len() - 1] {
            if self.module_files(&file.unit.target, &module).is_none() {
                return Resolution::Unresolved(UnresolvedReason::ExternalTarget);
            }
            module.push_str("::");
            module.push_str(component);
        }
        if let Some(files) = self.module_files(&file.unit.target, &module) {
            if files.len() != 1 {
                return Resolution::Unresolved(UnresolvedReason::AmbiguousTarget);
            }
            let result = self.module_function(&file.unit.target, &module, path[path.len() - 1]);
            if !matches!(
                result,
                Resolution::Unresolved(UnresolvedReason::MissingTarget)
            ) {
                return result;
            }
        }
        // The last qualifier can be an inherent type, but never an inferred receiver.
        if path.len() - index >= 2 {
            let ty = path[path.len() - 2];
            let parent_module = module
                .rsplit_once("::")
                .and_then(|(parent, last)| (last == ty).then_some(parent));
            if let Some(parent_module) = parent_module {
                return self.inherent_method(
                    &file.unit.target,
                    parent_module,
                    ty,
                    path[path.len() - 1],
                );
            }
        }
        if self.module_files(&file.unit.target, &module).is_some() {
            Resolution::Unresolved(UnresolvedReason::MissingTarget)
        } else if index == 0
            && !self
                .targets
                .get(file.unit.target.as_str())
                .is_some_and(|modules| {
                    modules
                        .keys()
                        .any(|m| m.starts_with(&format!("{module}::")))
                })
        {
            Resolution::Unresolved(UnresolvedReason::ExternalTarget)
        } else {
            Resolution::Unresolved(UnresolvedReason::UnknownModule)
        }
    }

    fn import_target(
        &self,
        file: &'a ParsedFile,
        import: &ImportBinding,
        suffix: &[&str],
    ) -> Resolution<'a> {
        if import.conditional {
            return Resolution::Unresolved(UnresolvedReason::ConditionalCompilation);
        }
        if import.unknown_attribute {
            return Resolution::Unresolved(UnresolvedReason::UnsupportedDispatch);
        }
        if import.wildcard {
            return Resolution::Unresolved(UnresolvedReason::WildcardImport);
        }
        // A use declaration is an alias for one syntactic path, not evidence
        // about external crates, re-exports, glob members or runtime dispatch.
        let mut path: Vec<&str> = import
            .module
            .split("::")
            .filter(|p| !p.is_empty())
            .collect();
        if let Some(name) = &import.name {
            path.push(name);
        }
        path.extend_from_slice(suffix);
        if path.is_empty() {
            return Resolution::Unresolved(UnresolvedReason::UnsupportedDispatch);
        }
        self.absolute_path(file, &path)
    }

    pub fn resolve(&self, file: &'a ParsedFile, call: &ExtractedCallSite) -> Resolution<'a> {
        if !file.valid {
            return Resolution::Unresolved(UnresolvedReason::ParseError);
        }
        if call.expression.trim_end().ends_with('!') {
            return Resolution::Unresolved(UnresolvedReason::MacroExpansion);
        }
        let path = match &call.target {
            CallTarget::Name(name) => vec![name.as_str()],
            CallTarget::Qualified(parts) => parts.iter().map(String::as_str).collect(),
            CallTarget::Dynamic => return Resolution::Unresolved(UnresolvedReason::DynamicTarget),
        };
        if path.is_empty() {
            return Resolution::Unresolved(UnresolvedReason::DynamicTarget);
        }
        if path
            .iter()
            .any(|part| part.is_empty() || part.contains('!'))
        {
            return Resolution::Unresolved(UnresolvedReason::MacroExpansion);
        }
        if matches!(path[0], "crate" | "self" | "super") {
            return self.absolute_path(file, &path);
        }
        let name = path[0];
        let mut scope = Some(call.scope_id);
        let mut wildcard = false;
        while let Some(id) = scope {
            let Some(current) = file.scopes.get(id) else {
                return Resolution::Unresolved(UnresolvedReason::ParseError);
            };
            // An impl/trait namespace cannot be closed over by a method body.
            if matches!(current.kind, ScopeKind::RustType | ScopeKind::RustTrait) {
                scope = current.parent;
                continue;
            }
            wildcard |= file.imports.iter().any(|i| i.scope_id == id && i.wildcard);
            let bindings: Vec<_> = file
                .bindings
                .iter()
                .filter(|b| b.scope_id == id && b.name == name)
                .collect();
            if !bindings.is_empty() {
                if wildcard {
                    return Resolution::Unresolved(UnresolvedReason::WildcardImport);
                }
                if bindings.iter().any(|b| b.kind == BindingKind::Import) {
                    if bindings.len() != 1 || bindings[0].kind != BindingKind::Import {
                        return Resolution::Unresolved(UnresolvedReason::AmbiguousBinding);
                    }
                    let imports: Vec<_> = file
                        .imports
                        .iter()
                        .filter(|import| {
                            import.scope_id == id
                                && import
                                    .alias
                                    .as_deref()
                                    .or(import.name.as_deref())
                                    .or_else(|| import.module.rsplit("::").next())
                                    == Some(name)
                        })
                        .collect();
                    return match imports.as_slice() {
                        [import] => self.import_target(file, import, &path[1..]),
                        [] => Resolution::Unresolved(UnresolvedReason::UnsupportedDispatch),
                        _ => Resolution::Unresolved(UnresolvedReason::AmbiguousBinding),
                    };
                }
                if bindings.iter().any(|b| b.kind != BindingKind::Definition) {
                    return Resolution::Unresolved(UnresolvedReason::Shadowed);
                }
                if path.len() == 1 {
                    let symbols: Vec<_> = file
                        .symbols
                        .iter()
                        .filter(|s| s.parent_scope == id && s.name == name)
                        .collect();
                    return match symbols.as_slice() {
                        [symbol] if bindings.len() == 1 => Self::safe_symbol(symbol),
                        [] => Resolution::Unresolved(UnresolvedReason::Shadowed),
                        _ => Resolution::Unresolved(UnresolvedReason::AmbiguousTarget),
                    };
                }
                // A module or type item is a namespace, not a callable
                // function. Only syntactically proven inherent types dispatch.
                if bindings.len() == 1
                    && current.kind == ScopeKind::Module
                    && !file
                        .symbols
                        .iter()
                        .any(|s| s.parent_scope == id && s.name == name)
                {
                    let mut module = file.unit.module.clone();
                    module.push_str("::");
                    module.push_str(name);
                    if self.module_files(&file.unit.target, &module).is_some() {
                        return self.absolute_path(file, &path);
                    }
                    if path.len() == 2
                        && file.scopes.iter().any(|s| {
                            s.kind == ScopeKind::RustType && s.name == name && s.parent == Some(id)
                        })
                    {
                        return self.inherent_method(
                            &file.unit.target,
                            &file.unit.module,
                            name,
                            path[1],
                        );
                    }
                }
                return Resolution::Unresolved(UnresolvedReason::UnsupportedDispatch);
            }
            scope = current.parent;
        }
        if wildcard {
            return Resolution::Unresolved(UnresolvedReason::WildcardImport);
        }
        if path.len() == 1 {
            return self.module_function(&file.unit.target, &file.unit.module, name);
        }
        // Unqualified leading modules/types belong to the current target only.
        // An absent leading module may still be a type in this module.
        if path.len() == 2 {
            let type_resolution =
                self.inherent_method(&file.unit.target, &file.unit.module, name, path[1]);
            if matches!(
                type_resolution,
                Resolution::Resolved(_)
                    | Resolution::Unresolved(
                        UnresolvedReason::AmbiguousTarget
                            | UnresolvedReason::ConditionalCompilation
                    )
            ) {
                return type_resolution;
            }
        }
        self.absolute_path(file, &path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{
        Binding, ExtractedSnapshot, ImportBinding, Position, Scope, SourceRange, SourceRef,
        SourceUnitKey,
    };
    use crate::git::SnapshotRole;

    fn source(path: &[u8]) -> SourceRef {
        SourceRef {
            snapshot: SnapshotRole::Target,
            path: path.to_vec(),
            range: SourceRange {
                start: Position {
                    line: 1,
                    byte_column: 1,
                },
                end: Position {
                    line: 1,
                    byte_column: 2,
                },
                start_byte: 0,
                end_byte: 1,
            },
        }
    }

    fn file(path: &str, target: &str, module: &str) -> ParsedFile {
        let unit = SourceUnitKey {
            path: path.as_bytes().to_vec(),
            target: target.to_owned(),
            module: module.to_owned(),
        };
        ParsedFile {
            path: unit.path.clone(),
            module: Some(module.to_owned()),
            language: LanguageId::Rust,
            valid: true,
            scopes: vec![Scope {
                id: 0,
                parent: None,
                name: module.to_owned(),
                qualified_name: format!("{target}::{module}"),
                kind: ScopeKind::Module,
                source: source(&unit.path),
            }],
            symbols: Vec::new(),
            bindings: Vec::new(),
            imports: Vec::new(),
            calls: Vec::new(),
            diagnostics: Vec::new(),
            unit,
        }
    }

    fn function(file: &mut ParsedFile, parent: usize, name: &str) -> usize {
        let scope_id = file.scopes.len();
        let qualified_name = format!("{}::{name}", file.scopes[parent].qualified_name);
        file.symbols.push(Symbol {
            unit: file.unit.clone(),
            language: LanguageId::Rust,
            source: source(&file.path),
            scope_id,
            parent_scope: parent,
            name: name.to_owned(),
            qualified_name: qualified_name.clone(),
            kind: if file.scopes[parent].kind == ScopeKind::RustType {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            },
            fingerprint: String::new(),
            direct_fingerprint: String::new(),
            non_call_fingerprint: String::new(),
            decorated: false,
            conditional: false,
            unknown_attribute: false,
        });
        file.scopes.push(Scope {
            id: scope_id,
            parent: Some(parent),
            name: name.to_owned(),
            qualified_name,
            kind: ScopeKind::Function,
            source: source(&file.path),
        });
        file.bindings.push(Binding {
            scope_id: parent,
            name: name.to_owned(),
            kind: BindingKind::Definition,
            source: source(&file.path),
            order: file.bindings.len(),
            conditional: false,
            unknown_attribute: false,
        });
        scope_id
    }

    fn call(scope_id: usize, target: CallTarget, expression: &str) -> ExtractedCallSite {
        ExtractedCallSite {
            scope_id,
            expression: expression.to_owned(),
            target,
            source: source(b"src/lib.rs"),
        }
    }

    fn snapshot(files: Vec<ParsedFile>) -> ExtractedSnapshot {
        ExtractedSnapshot {
            role: SnapshotRole::Target,
            files: files
                .into_iter()
                .map(|file| (file.unit.clone(), file))
                .collect(),
            diagnostics: Vec::new(),
            complete: true,
        }
    }

    fn resolved_name(resolution: Resolution<'_>) -> String {
        match resolution {
            Resolution::Resolved(symbol) => symbol.qualified_name.clone(),
            Resolution::Unresolved(reason) => panic!("expected resolved symbol, got {reason:?}"),
        }
    }

    fn reason(resolution: Resolution<'_>) -> UnresolvedReason {
        match resolution {
            Resolution::Unresolved(reason) => reason,
            Resolution::Resolved(symbol) => panic!("unexpected resolved symbol: {}", symbol.name),
        }
    }

    #[test]
    fn lexical_recursion_modules_and_target_isolation() {
        let mut root = file("src/lib.rs", "Cargo.toml:lib:crate_a", "crate");
        let outer = function(&mut root, 0, "outer");
        function(&mut root, outer, "inner");
        let mut nested = file("src/deep.rs", "Cargo.toml:lib:crate_a", "crate::deep");
        function(&mut nested, 0, "leaf");
        let mut other_target = file("src/lib.rs", "Cargo.toml:bin:crate_a", "crate");
        function(&mut other_target, 0, "exclusive");
        let snapshot = snapshot(vec![root, nested, other_target]);
        let root = &snapshot.files[&SourceUnitKey {
            path: b"src/lib.rs".to_vec(),
            target: "Cargo.toml:lib:crate_a".to_owned(),
            module: "crate".to_owned(),
        }];
        let nested = &snapshot.files[&SourceUnitKey {
            path: b"src/deep.rs".to_vec(),
            target: "Cargo.toml:lib:crate_a".to_owned(),
            module: "crate::deep".to_owned(),
        }];
        let index = RustIndex::new(&snapshot);
        assert_eq!(
            resolved_name(index.resolve(
                root,
                &call(outer, CallTarget::Name("inner".into()), "inner")
            )),
            "Cargo.toml:lib:crate_a::crate::outer::inner"
        );
        assert_eq!(
            resolved_name(index.resolve(
                root,
                &call(outer, CallTarget::Name("outer".into()), "outer")
            )),
            "Cargo.toml:lib:crate_a::crate::outer"
        );
        assert_eq!(
            resolved_name(index.resolve(
                nested,
                &call(
                    0,
                    CallTarget::Qualified(vec!["super".into(), "outer".into()]),
                    "super::outer"
                )
            )),
            "Cargo.toml:lib:crate_a::crate::outer"
        );
        assert_eq!(
            resolved_name(index.resolve(
                root,
                &call(
                    0,
                    CallTarget::Qualified(vec!["crate".into(), "deep".into(), "leaf".into()]),
                    "crate::deep::leaf"
                )
            )),
            "Cargo.toml:lib:crate_a::crate::deep::leaf"
        );
        assert_eq!(
            resolved_name(index.resolve(
                nested,
                &call(
                    0,
                    CallTarget::Qualified(vec!["self".into(), "leaf".into()]),
                    "self::leaf"
                )
            )),
            "Cargo.toml:lib:crate_a::crate::deep::leaf"
        );
        assert_eq!(
            reason(index.resolve(
                nested,
                &call(
                    0,
                    CallTarget::Qualified(vec!["super".into(), "super".into(), "outer".into()]),
                    "super::super::outer"
                )
            )),
            UnresolvedReason::UnknownModule
        );
        assert_eq!(
            reason(index.resolve(
                root,
                &call(0, CallTarget::Name("exclusive".into()), "exclusive")
            )),
            UnresolvedReason::MissingTarget
        );
    }

    #[test]
    fn static_imports_shadowing_globs_and_attributes() {
        let mut root = file("src/lib.rs", "Cargo.toml:lib:a", "crate");
        let caller = function(&mut root, 0, "caller");
        let mut module = file("src/dep.rs", "Cargo.toml:lib:a", "crate::dep");
        function(&mut module, 0, "callee");
        let import = ImportBinding {
            scope_id: 0,
            module: "crate::dep".into(),
            name: Some("callee".into()),
            alias: Some("renamed".into()),
            relative_level: 0,
            wildcard: false,
            conditional: false,
            unknown_attribute: false,
            source: source(&root.path),
        };
        root.imports.push(import);
        root.bindings.push(Binding {
            scope_id: 0,
            name: "renamed".into(),
            kind: BindingKind::Import,
            source: source(&root.path),
            order: 0,
            conditional: false,
            unknown_attribute: false,
        });
        let snapshot = snapshot(vec![root.clone(), module.clone()]);
        let index = RustIndex::new(&snapshot);
        let root = &snapshot.files[&root.unit];
        assert!(matches!(
            index.resolve(root, &call(caller, CallTarget::Name("renamed".into()), "renamed")),
            Resolution::Resolved(symbol) if symbol.name == "callee"
        ));

        let mut shadowed = (*root).clone();
        shadowed.bindings.push(Binding {
            scope_id: caller,
            name: "renamed".into(),
            kind: BindingKind::Parameter,
            source: source(&shadowed.path),
            order: 1,
            conditional: false,
            unknown_attribute: false,
        });
        let snapshot = self::snapshot(vec![shadowed.clone(), module.clone()]);
        let index = RustIndex::new(&snapshot);
        assert_eq!(
            reason(index.resolve(
                &snapshot.files[&shadowed.unit],
                &call(caller, CallTarget::Name("renamed".into()), "renamed")
            )),
            UnresolvedReason::Shadowed
        );
        let mut globbed = (*root).clone();
        globbed.imports.push(ImportBinding {
            scope_id: caller,
            module: "crate::dep".into(),
            name: None,
            alias: None,
            relative_level: 0,
            wildcard: true,
            conditional: false,
            unknown_attribute: false,
            source: source(&globbed.path),
        });
        let snapshot = self::snapshot(vec![globbed.clone(), module.clone()]);
        let index = RustIndex::new(&snapshot);
        assert_eq!(
            reason(index.resolve(
                &snapshot.files[&globbed.unit],
                &call(caller, CallTarget::Name("renamed".into()), "renamed")
            )),
            UnresolvedReason::WildcardImport
        );
        let mut ambiguous = (*root).clone();
        ambiguous.bindings.push(Binding {
            scope_id: 0,
            name: "renamed".into(),
            kind: BindingKind::Definition,
            source: source(&ambiguous.path),
            order: 2,
            conditional: false,
            unknown_attribute: false,
        });
        let snapshot = self::snapshot(vec![ambiguous.clone(), module.clone()]);
        let index = RustIndex::new(&snapshot);
        assert_eq!(
            reason(index.resolve(
                &snapshot.files[&ambiguous.unit],
                &call(caller, CallTarget::Name("renamed".into()), "renamed")
            )),
            UnresolvedReason::AmbiguousBinding
        );

        module.symbols[0].conditional = true;
        let snapshot = self::snapshot(vec![root.clone(), module.clone()]);
        let index = RustIndex::new(&snapshot);
        assert_eq!(
            reason(index.resolve(
                &snapshot.files[&root.unit],
                &call(caller, CallTarget::Name("renamed".into()), "renamed")
            )),
            UnresolvedReason::ConditionalCompilation
        );
        module.symbols[0].conditional = false;
        module.symbols[0].unknown_attribute = true;
        let snapshot = self::snapshot(vec![root.clone(), module]);
        let index = RustIndex::new(&snapshot);
        assert_eq!(
            reason(index.resolve(
                &snapshot.files[&root.unit],
                &call(caller, CallTarget::Name("renamed".into()), "renamed")
            )),
            UnresolvedReason::UnsupportedDispatch
        );
        assert_eq!(
            reason(index.resolve(
                &snapshot.files[&root.unit],
                &call(caller, CallTarget::Dynamic, "vec!")
            )),
            UnresolvedReason::MacroExpansion
        );
    }

    #[test]
    fn inherent_calls_are_unique_and_trait_calls_are_not_proven() {
        let mut root = file("src/lib.rs", "Cargo.toml:lib:a", "crate");
        root.bindings.push(Binding {
            scope_id: 0,
            name: "Widget".into(),
            kind: BindingKind::Definition,
            source: source(&root.path),
            order: 0,
            conditional: false,
            unknown_attribute: false,
        });
        root.scopes.push(Scope {
            id: 1,
            parent: Some(0),
            name: "Widget".into(),
            qualified_name: "Cargo.toml:lib:a::crate::Widget".into(),
            kind: ScopeKind::RustType,
            source: source(&root.path),
        });
        function(&mut root, 1, "new");
        root.scopes.push(Scope {
            id: 3,
            parent: Some(0),
            name: "Trait".into(),
            qualified_name: "Cargo.toml:lib:a::crate::Trait".into(),
            kind: ScopeKind::RustTrait,
            source: source(&root.path),
        });
        function(&mut root, 3, "trait_method");
        let snapshot = snapshot(vec![root.clone()]);
        let index = RustIndex::new(&snapshot);
        let root = &snapshot.files[&root.unit];
        assert!(matches!(
            index.resolve(root, &call(0, CallTarget::Qualified(vec!["Widget".into(), "new".into()]), "Widget::new")),
            Resolution::Resolved(symbol) if symbol.name == "new"
        ));
        assert_eq!(
            reason(index.resolve(
                root,
                &call(
                    0,
                    CallTarget::Qualified(vec!["Trait".into(), "trait_method".into()]),
                    "Trait::trait_method"
                )
            )),
            UnresolvedReason::UnsupportedDispatch
        );
        assert_eq!(
            reason(index.resolve(root, &call(0, CallTarget::Dynamic, "widget.method"))),
            UnresolvedReason::DynamicTarget
        );
        let mut duplicated = (*root).clone();
        duplicated.scopes.push(Scope {
            id: 5,
            parent: Some(0),
            name: "Widget".into(),
            qualified_name: "Cargo.toml:lib:a::crate::Widget".into(),
            kind: ScopeKind::RustType,
            source: source(&root.path),
        });
        let snapshot = self::snapshot(vec![duplicated.clone()]);
        let index = RustIndex::new(&snapshot);
        assert_eq!(
            reason(index.resolve(
                &snapshot.files[&duplicated.unit],
                &call(
                    0,
                    CallTarget::Qualified(vec!["Widget".into(), "new".into()]),
                    "Widget::new"
                )
            )),
            UnresolvedReason::AmbiguousTarget
        );
    }
}
