use crate::analysis::{
    BindingKind, CallTarget, ExtractedCallSite, ExtractedSnapshot, ImportBinding, ParsedFile,
    ScopeKind, Symbol,
};
use crate::languages::{Resolution, UnresolvedReason};
use std::collections::BTreeMap;

pub struct PythonIndex<'a> {
    modules: BTreeMap<&'a str, Vec<&'a ParsedFile>>,
}
impl<'a> PythonIndex<'a> {
    pub fn new(snapshot: &'a ExtractedSnapshot) -> Self {
        let mut modules: BTreeMap<&str, Vec<&ParsedFile>> = BTreeMap::new();
        for file in snapshot.files.values().filter(|f| f.valid) {
            if let Some(module) = &file.module {
                modules.entry(module).or_default().push(file);
            }
        }
        Self { modules }
    }
    fn find_module_function(&self, module: &str, name: &str) -> Resolution<'a> {
        let Some(files) = self.modules.get(module) else {
            return if self
                .modules
                .keys()
                .any(|m| m.split('.').next() == module.split('.').next())
            {
                Resolution::Unresolved(UnresolvedReason::MissingTarget)
            } else {
                Resolution::Unresolved(UnresolvedReason::ExternalTarget)
            };
        };
        if files.len() != 1 {
            return Resolution::Unresolved(UnresolvedReason::AmbiguousTarget);
        }
        let file = files[0];
        if file
            .imports
            .iter()
            .any(|import| import.scope_id == 0 && import.wildcard)
        {
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
        if bindings
            .first()
            .is_some_and(|binding| binding.kind != BindingKind::Definition)
        {
            return Resolution::Unresolved(UnresolvedReason::Shadowed);
        }
        let candidates: Vec<_> = file
            .symbols
            .iter()
            .filter(|s| s.parent_scope == 0 && s.name == name)
            .collect();
        if candidates.len() == 1 {
            self.safe_symbol(file, candidates[0])
        } else if candidates.is_empty() {
            Resolution::Unresolved(UnresolvedReason::MissingTarget)
        } else {
            Resolution::Unresolved(UnresolvedReason::AmbiguousTarget)
        }
    }
    fn safe_symbol(&self, _file: &'a ParsedFile, symbol: &'a Symbol) -> Resolution<'a> {
        // A decorator can replace the function object; syntax alone cannot prove its runtime target.
        if symbol.decorated {
            return Resolution::Unresolved(UnresolvedReason::DecoratedTarget);
        }
        Resolution::Resolved(symbol)
    }
    fn package_member_conflict(&self, module: &str, member: &str) -> Option<UnresolvedReason> {
        let files = self.modules.get(module)?;
        if files.len() != 1 {
            return Some(UnresolvedReason::AmbiguousTarget);
        }
        let package = files[0];
        if package
            .imports
            .iter()
            .any(|import| import.scope_id == 0 && import.wildcard)
        {
            return Some(UnresolvedReason::WildcardImport);
        }
        if package.bindings.iter().any(|binding| {
            binding.scope_id == 0 && (binding.name == member || binding.name == "__getattr__")
        }) {
            return Some(UnresolvedReason::AmbiguousBinding);
        }
        None
    }
    fn import_target(
        &self,
        file: &'a ParsedFile,
        binding: &ImportBinding,
        path: &[String],
    ) -> Resolution<'a> {
        let Some(mut module) = resolve_module(file, binding) else {
            return Resolution::Unresolved(UnresolvedReason::UnknownModule);
        };
        if let Some(member) = &binding.name {
            if path.len() == 1 {
                return self.find_module_function(&module, member);
            }
            for component in std::iter::once(member.as_str())
                .chain(path[1..path.len() - 1].iter().map(String::as_str))
            {
                if let Some(reason) = self.package_member_conflict(&module, component) {
                    return Resolution::Unresolved(reason);
                }
                module.push('.');
                module.push_str(component);
            }
            return self.find_module_function(&module, path.last().unwrap());
        }
        if path.len() < 2 {
            return Resolution::Unresolved(UnresolvedReason::UnsupportedDispatch);
        }
        if !import_matches_path(binding, path) {
            return Resolution::Unresolved(UnresolvedReason::UnsupportedDispatch);
        }
        self.find_module_function(&module, path.last().unwrap())
    }
    pub fn resolve(&self, file: &'a ParsedFile, call: &ExtractedCallSite) -> Resolution<'a> {
        if !file.valid {
            return Resolution::Unresolved(UnresolvedReason::ParseError);
        }
        let path: Vec<String> = match &call.target {
            CallTarget::Name(name) => vec![name.clone()],
            CallTarget::Qualified(parts) => parts.clone(),
            CallTarget::Dynamic => return Resolution::Unresolved(UnresolvedReason::DynamicTarget),
        };
        let name = &path[0];
        let mut scope = Some(call.scope_id);
        let mut wildcard_seen = false;
        while let Some(id) = scope {
            wildcard_seen |= file
                .imports
                .iter()
                .any(|import| import.scope_id == id && import.wildcard);
            let current = &file.scopes[id];
            // Methods cannot close over their class namespace.
            if current.kind == ScopeKind::Class && id != call.scope_id {
                scope = current.parent;
                continue;
            }
            let found: Vec<_> = file
                .bindings
                .iter()
                .filter(|b| b.scope_id == id && b.name == *name)
                .collect();
            if !found.is_empty() {
                if wildcard_seen {
                    return Resolution::Unresolved(UnresolvedReason::WildcardImport);
                }
                if found.iter().any(|b| b.kind == BindingKind::Global) {
                    scope = Some(0);
                    if id == 0 {
                        return Resolution::Unresolved(UnresolvedReason::AmbiguousBinding);
                    }
                    continue;
                }
                if found.iter().any(|b| b.kind == BindingKind::Nonlocal) {
                    scope = current.parent;
                    continue;
                }
                if found
                    .iter()
                    .all(|binding| binding.kind == BindingKind::Import)
                {
                    let imports: Vec<_> = file
                        .imports
                        .iter()
                        .filter(|import| {
                            import.scope_id == id
                                && (import.alias.as_deref() == Some(name)
                                    || (import.alias.is_none()
                                        && (import.name.as_deref() == Some(name)
                                            || import.module.split('.').next() == Some(name))))
                                && import_matches_path(import, &path)
                        })
                        .collect();
                    return match imports.as_slice() {
                        [import] => self.import_target(file, import, &path),
                        [] => Resolution::Unresolved(UnresolvedReason::UnsupportedDispatch),
                        _ => Resolution::Unresolved(UnresolvedReason::AmbiguousBinding),
                    };
                }
                if found.len() != 1 {
                    return Resolution::Unresolved(UnresolvedReason::AmbiguousBinding);
                }
                match found[0].kind {
                    BindingKind::Definition if path.len() == 1 => {
                        let candidates: Vec<_> = file
                            .symbols
                            .iter()
                            .filter(|s| s.parent_scope == id && s.name == *name)
                            .collect();
                        return if candidates.len() == 1 {
                            self.safe_symbol(file, candidates[0])
                        } else {
                            Resolution::Unresolved(UnresolvedReason::AmbiguousTarget)
                        };
                    }
                    BindingKind::Definition => {
                        return Resolution::Unresolved(UnresolvedReason::UnsupportedDispatch);
                    }
                    _ => return Resolution::Unresolved(UnresolvedReason::Shadowed),
                }
            }
            scope = current.parent;
        }
        Resolution::Unresolved(if wildcard_seen {
            UnresolvedReason::WildcardImport
        } else {
            UnresolvedReason::ExternalTarget
        })
    }
}
fn import_matches_path(binding: &ImportBinding, path: &[String]) -> bool {
    if binding.name.is_some() {
        return true;
    }
    if let Some(alias) = &binding.alias {
        return path.len() == 2 && path[0] == *alias;
    }
    path.len() >= 2 && path[..path.len() - 1].join(".") == binding.module
}
fn resolve_module(file: &ParsedFile, binding: &ImportBinding) -> Option<String> {
    if binding.relative_level == 0 {
        return Some(binding.module.clone());
    }
    let current = file.module.as_ref()?;
    let mut components: Vec<_> = current.split('.').collect();
    if !file.path.ends_with(b"/__init__.py") {
        components.pop();
    }
    for _ in 1..binding.relative_level {
        components.pop()?;
    }
    let prefix = components.join(".");
    if binding.module.is_empty() {
        Some(prefix)
    } else if prefix.is_empty() {
        None
    } else {
        Some(format!("{prefix}.{}", binding.module))
    }
}
