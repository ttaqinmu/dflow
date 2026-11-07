use crate::analysis::{
    Binding, BindingKind, Diagnostic, DiagnosticKind, ExtractedCallSite, ImportBinding, LanguageId,
    ParsedFile, Position, Scope, ScopeKind, SourceRange, SourceRef, Symbol, SymbolKind,
    fingerprint,
};
use crate::git::{CapturedFile, CapturedSnapshot, SnapshotRole};
use crate::languages::LanguageAdapter;
use tree_sitter::{Node, Parser};

pub struct PythonAdapter;

fn configured_source_root(snapshot: &CapturedSnapshot) -> Option<Result<&'static [u8], ()>> {
    let project = snapshot.files.get(b"pyproject.toml".as_slice())?;
    let source = match std::str::from_utf8(&project.bytes) {
        Ok(source) => source,
        Err(_) => return Some(Err(())),
    };
    let value: toml::Value = match source.parse() {
        Ok(value) => value,
        Err(_) => return Some(Err(())),
    };
    let setuptools = value.get("tool").and_then(|v| v.get("setuptools"))?;
    let package_dir = setuptools.get("package-dir")?;
    let Some(root) = package_dir.get("").and_then(toml::Value::as_str) else {
        return Some(Err(()));
    };
    match root.trim_end_matches('/') {
        "" | "." => Some(Ok(b"")),
        "src" => Some(Ok(b"src/")),
        _ => Some(Err(())),
    }
}

fn python_module_candidates(snapshot: &CapturedSnapshot, path: &[u8]) -> Vec<String> {
    if !path.ends_with(b".py") {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    let configured = configured_source_root(snapshot);
    for prefix in [b"".as_slice(), b"src/".as_slice()] {
        if matches!(configured, Some(Err(()))) {
            return Vec::new();
        }
        if let Some(Ok(root)) = configured {
            if path.starts_with(b"src/") && root != prefix {
                continue;
            }
            if !path.starts_with(b"src/") && prefix == b"src/" {
                continue;
            }
        }
        let Some(rest) = path.strip_prefix(prefix) else {
            continue;
        };
        let Ok(rest) = std::str::from_utf8(rest) else {
            continue;
        };
        let segments: Vec<_> = rest.split('/').collect();
        if segments.is_empty() {
            continue;
        }
        let mut parent = prefix.to_vec();
        let mut valid = true;
        for segment in &segments[..segments.len() - 1] {
            parent.extend_from_slice(segment.as_bytes());
            parent.extend_from_slice(b"/__init__.py");
            if !snapshot.files.contains_key(&parent) {
                valid = false;
                break;
            }
            parent.truncate(parent.len() - b"__init__.py".len());
        }
        if !valid {
            continue;
        }
        let stem = segments.last().unwrap().strip_suffix(".py").unwrap_or("");
        let mut names: Vec<_> = segments[..segments.len() - 1]
            .iter()
            .map(|s| s.to_string())
            .collect();
        if stem != "__init__" {
            names.push(stem.to_string());
        }
        if names
            .iter()
            .all(|s| !s.is_empty() && s.chars().all(|c| c == '_' || c.is_alphanumeric()))
        {
            candidates.push(names.join("."));
        }
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

struct Walker<'a> {
    bytes: &'a [u8],
    path: Vec<u8>,
    role: SnapshotRole,
    parsed: ParsedFile,
    order: usize,
}
impl Walker<'_> {
    fn range(&self, node: Node<'_>) -> SourceRange {
        let start = node.start_position();
        let end = node.end_position();
        SourceRange {
            start: Position {
                line: start.row + 1,
                byte_column: start.column + 1,
            },
            end: Position {
                line: end.row + 1,
                byte_column: end.column + 1,
            },
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
        }
    }
    fn source(&self, node: Node<'_>) -> SourceRef {
        SourceRef {
            snapshot: self.role,
            path: self.path.clone(),
            range: self.range(node),
        }
    }
    fn text(&self, node: Node<'_>) -> String {
        String::from_utf8_lossy(&self.bytes[node.byte_range()]).into_owned()
    }
    fn named_children<'a>(&self, node: Node<'a>) -> Vec<Node<'a>> {
        let mut cursor = node.walk();
        node.named_children(&mut cursor).collect()
    }
    fn all_children<'a>(&self, node: Node<'a>) -> Vec<Node<'a>> {
        let mut cursor = node.walk();
        node.children(&mut cursor).collect()
    }
    fn canonical(&self, node: Node<'_>, parts: &mut Vec<Vec<u8>>) {
        if node.kind() == "comment" {
            return;
        }
        let children = self.all_children(node);
        if children.is_empty() {
            let token = &self.bytes[node.byte_range()];
            if !token.iter().all(u8::is_ascii_whitespace) {
                parts.push(node.kind().as_bytes().to_vec());
                parts.push(token.to_vec());
            }
        } else {
            parts.push(node.kind().as_bytes().to_vec());
            for child in children {
                self.canonical(child, parts);
            }
        }
    }
    fn add_binding(&mut self, scope_id: usize, name: String, kind: BindingKind, node: Node<'_>) {
        self.order += 1;
        self.parsed.bindings.push(Binding {
            scope_id,
            name,
            kind,
            source: self.source(node),
            order: self.order,
        });
    }
    fn collect_names(&self, node: Node<'_>, names: &mut Vec<String>) {
        if node.kind() == "identifier" {
            names.push(self.text(node));
            return;
        }
        if matches!(
            node.kind(),
            "pattern"
                | "pattern_list"
                | "tuple"
                | "list"
                | "list_pattern"
                | "tuple_pattern"
                | "list_splat_pattern"
                | "as_pattern_target"
                | "parenthesized_expression"
        ) {
            for child in self.named_children(node) {
                self.collect_names(child, names);
            }
        }
    }
    fn add_assignments(&mut self, node: Node<'_>, scope_id: usize) {
        if let Some(left) = node.child_by_field_name("left") {
            let mut names = Vec::new();
            self.collect_names(left, &mut names);
            for name in names {
                self.add_binding(scope_id, name, BindingKind::Assignment, left);
            }
        }
    }
    fn add_parameters(&mut self, node: Node<'_>, scope_id: usize) {
        if let Some(parameters) = node.child_by_field_name("parameters") {
            for param in self.named_children(parameters) {
                let name = if param.kind() == "identifier" {
                    Some(param)
                } else {
                    self.named_children(param)
                        .into_iter()
                        .find(|c| c.kind() == "identifier")
                };
                if let Some(name) = name {
                    self.add_binding(scope_id, self.text(name), BindingKind::Parameter, name);
                }
            }
        }
    }
    fn add_imports(&mut self, node: Node<'_>, scope_id: usize) {
        let from = node.kind() == "import_from_statement";
        let (module, level) = if from {
            let Some(module) = node.child_by_field_name("module_name") else {
                return;
            };
            let raw = self.text(module);
            let level = raw.chars().take_while(|c| *c == '.').count();
            (raw.trim_start_matches('.').to_string(), level)
        } else {
            (String::new(), 0)
        };
        for item in self.named_children(node) {
            if !matches!(
                item.kind(),
                "dotted_name" | "aliased_import" | "wildcard_import"
            ) || (from
                && node
                    .child_by_field_name("module_name")
                    .is_some_and(|m| m.id() == item.id()))
            {
                continue;
            }
            let wildcard = item.kind() == "wildcard_import";
            let (original, alias) = if item.kind() == "aliased_import" {
                let Some(name) = item.child_by_field_name("name") else {
                    continue;
                };
                (
                    self.text(name),
                    item.child_by_field_name("alias").map(|a| self.text(a)),
                )
            } else {
                (self.text(item), None)
            };
            let binding_name = alias.clone().unwrap_or_else(|| {
                if from {
                    original.clone()
                } else {
                    original.split('.').next().unwrap_or(&original).to_string()
                }
            });
            self.parsed.imports.push(ImportBinding {
                scope_id,
                module: if from {
                    module.clone()
                } else {
                    original.clone()
                },
                name: if from && !wildcard {
                    Some(original)
                } else {
                    None
                },
                alias,
                relative_level: level,
                wildcard,
                source: self.source(node),
            });
            if !wildcard {
                self.add_binding(scope_id, binding_name, BindingKind::Import, node);
            }
        }
    }
    fn add_directive(&mut self, node: Node<'_>, scope_id: usize) {
        let kind = if node.kind() == "global_statement" {
            BindingKind::Global
        } else {
            BindingKind::Nonlocal
        };
        for child in self.named_children(node) {
            if child.kind() == "identifier" {
                self.add_binding(scope_id, self.text(child), kind.clone(), child);
            }
        }
    }
    fn add_call(&mut self, node: Node<'_>, scope_id: usize) {
        let expression = node
            .child_by_field_name("function")
            .map(|n| self.text(n))
            .unwrap_or_else(|| self.text(node));
        self.parsed.calls.push(ExtractedCallSite {
            scope_id,
            expression,
            source: self.source(node),
        });
    }
    fn definition(&mut self, outer: Node<'_>, inner: Node<'_>, parent: usize) {
        let Some(name_node) = inner.child_by_field_name("name") else {
            return;
        };
        let name = self.text(name_node);
        let parent_name = self.parsed.scopes[parent].qualified_name.clone();
        let qualified = if parent_name.is_empty() {
            name.clone()
        } else {
            format!("{parent_name}.{name}")
        };
        let kind = if inner.kind() == "class_definition" {
            ScopeKind::Class
        } else {
            ScopeKind::Function
        };
        let scope_id = self.parsed.scopes.len();
        self.parsed.scopes.push(Scope {
            id: scope_id,
            parent: Some(parent),
            name: name.clone(),
            qualified_name: qualified.clone(),
            kind: kind.clone(),
            source: self.source(outer),
        });
        self.add_binding(parent, name.clone(), BindingKind::Definition, name_node);
        if kind == ScopeKind::Function {
            let method = self.parsed.scopes[parent].kind == ScopeKind::Class;
            let async_ = self.all_children(inner).iter().any(|n| n.kind() == "async");
            let symbol_kind = match (method, async_) {
                (false, false) => SymbolKind::Function,
                (false, true) => SymbolKind::AsyncFunction,
                (true, false) => SymbolKind::Method,
                (true, true) => SymbolKind::AsyncMethod,
            };
            let mut parts = Vec::new();
            self.canonical(outer, &mut parts);
            self.parsed.symbols.push(Symbol {
                language: LanguageId::Python,
                source: self.source(outer),
                scope_id,
                parent_scope: parent,
                name,
                qualified_name: qualified,
                kind: symbol_kind,
                fingerprint: fingerprint(parts),
            });
        }
        // Decorators, class superclasses, and parameter defaults execute in the enclosing scope.
        if outer.id() != inner.id() {
            for child in self.named_children(outer) {
                if child.id() != inner.id() {
                    self.visit(child, parent);
                }
            }
        }
        for child in self.named_children(inner) {
            if child.kind() == "block" {
                self.visit(child, scope_id);
            } else if child.kind() == "parameters" {
                self.add_parameters(inner, scope_id);
                self.visit(child, parent);
            } else if child.id() != name_node.id() {
                self.visit(child, parent);
            }
        }
    }
    fn visit(&mut self, node: Node<'_>, scope_id: usize) {
        match node.kind() {
            "decorated_definition" => {
                if let Some(def) = node.child_by_field_name("definition") {
                    self.definition(node, def, scope_id);
                }
            }
            "function_definition" | "class_definition" => self.definition(node, node, scope_id),
            "call" => {
                self.add_call(node, scope_id);
                for child in self.named_children(node) {
                    self.visit(child, scope_id);
                }
            }
            "import_statement" | "import_from_statement" => self.add_imports(node, scope_id),
            "assignment" | "augmented_assignment" | "annotated_assignment" | "for_statement" => {
                self.add_assignments(node, scope_id);
                for child in self.named_children(node) {
                    self.visit(child, scope_id);
                }
            }
            "global_statement" | "nonlocal_statement" => self.add_directive(node, scope_id),
            "named_expression" => {
                if let Some(name) = node.child_by_field_name("name") {
                    // Assignment expressions inside comprehensions bind in the nearest enclosing non-comprehension scope.
                    let mut binding_scope = scope_id;
                    while self.parsed.scopes[binding_scope].kind == ScopeKind::Comprehension {
                        binding_scope = self.parsed.scopes[binding_scope].parent.unwrap_or(0);
                    }
                    self.add_binding(
                        binding_scope,
                        self.text(name),
                        BindingKind::Assignment,
                        name,
                    );
                }
                for child in self.named_children(node) {
                    self.visit(child, scope_id);
                }
            }
            "except_clause" => {
                if let Some(alias) = node.child_by_field_name("alias") {
                    self.add_binding(scope_id, self.text(alias), BindingKind::Assignment, alias);
                }
                for child in self.named_children(node) {
                    self.visit(child, scope_id);
                }
            }
            "as_pattern" => {
                if let Some(alias) = node.child_by_field_name("alias") {
                    let mut names = Vec::new();
                    self.collect_names(alias, &mut names);
                    for name in names {
                        self.add_binding(scope_id, name, BindingKind::Assignment, alias);
                    }
                }
                for child in self.named_children(node) {
                    self.visit(child, scope_id);
                }
            }
            "for_in_clause" => {
                self.add_assignments(node, scope_id);
                for child in self.named_children(node) {
                    self.visit(child, scope_id);
                }
            }
            "list_comprehension"
            | "set_comprehension"
            | "dictionary_comprehension"
            | "generator_expression" => {
                let id = self.parsed.scopes.len();
                let parent_name = self.parsed.scopes[scope_id].qualified_name.clone();
                let name = format!(
                    "<comprehension@{}:{}>",
                    node.start_position().row + 1,
                    node.start_position().column + 1
                );
                let qualified_name = if parent_name.is_empty() {
                    name.clone()
                } else {
                    format!("{parent_name}.{name}")
                };
                self.parsed.scopes.push(Scope {
                    id,
                    parent: Some(scope_id),
                    name,
                    qualified_name,
                    kind: ScopeKind::Comprehension,
                    source: self.source(node),
                });
                for child in self.named_children(node) {
                    self.visit(child, id);
                }
            }
            _ => {
                for child in self.named_children(node) {
                    self.visit(child, scope_id);
                }
            }
        }
    }
    fn errors(&mut self, node: Node<'_>) {
        if node.is_error() || node.is_missing() {
            self.parsed.diagnostics.push(Diagnostic {
                kind: DiagnosticKind::ParseError,
                message: format!("Python syntax error near {}", node.kind()),
                snapshot: self.role,
                path: self.path.clone(),
                range: Some(self.range(node)),
            });
        }
        for child in self.all_children(node) {
            self.errors(child);
        }
    }
}
impl LanguageAdapter for PythonAdapter {
    fn accepts_source(&self, path: &[u8]) -> bool {
        path.ends_with(b".py")
    }
    fn accepts_metadata(&self, path: &[u8]) -> bool {
        path == b"pyproject.toml" || path.ends_with(b"/pyproject.toml")
    }
    fn module_candidates(&self, snapshot: &CapturedSnapshot, path: &[u8]) -> Vec<String> {
        python_module_candidates(snapshot, path)
    }
    fn extract(
        &self,
        file: &CapturedFile,
        role: SnapshotRole,
        module: Option<String>,
    ) -> ParsedFile {
        let mut parsed = ParsedFile {
            path: file.path.clone(),
            module,
            language: LanguageId::Python,
            valid: true,
            scopes: Vec::new(),
            symbols: Vec::new(),
            bindings: Vec::new(),
            imports: Vec::new(),
            calls: Vec::new(),
            diagnostics: Vec::new(),
        };
        let bytes = &file.bytes;
        if std::str::from_utf8(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)).is_err() {
            parsed.valid = false;
            parsed.diagnostics.push(Diagnostic {
                kind: DiagnosticKind::InvalidEncoding,
                message: "Python source must be UTF-8 or UTF-8 BOM".into(),
                snapshot: role,
                path: file.path.clone(),
                range: None,
            });
            return parsed;
        }
        let mut parser = Parser::new();
        if let Err(error) = parser.set_language(&tree_sitter_python::LANGUAGE.into()) {
            parsed.valid = false;
            parsed.diagnostics.push(Diagnostic {
                kind: DiagnosticKind::ParseError,
                message: format!("Python parser unavailable: {error}"),
                snapshot: role,
                path: file.path.clone(),
                range: None,
            });
            return parsed;
        }
        let Some(tree) = parser.parse(bytes, None) else {
            parsed.valid = false;
            parsed.diagnostics.push(Diagnostic {
                kind: DiagnosticKind::ParseError,
                message: "Python parse cancelled".into(),
                snapshot: role,
                path: file.path.clone(),
                range: None,
            });
            return parsed;
        };
        let root = tree.root_node();
        parsed.scopes.push(Scope {
            id: 0,
            parent: None,
            name: String::new(),
            qualified_name: String::new(),
            kind: ScopeKind::Module,
            source: SourceRef {
                snapshot: role,
                path: file.path.clone(),
                range: SourceRange {
                    start: Position {
                        line: 1,
                        byte_column: 1,
                    },
                    end: Position {
                        line: root.end_position().row + 1,
                        byte_column: root.end_position().column + 1,
                    },
                    start_byte: 0,
                    end_byte: bytes.len(),
                },
            },
        });
        let mut walker = Walker {
            bytes,
            path: file.path.clone(),
            role,
            parsed,
            order: 0,
        };
        walker.parsed.valid = !root.has_error();
        walker.errors(root);
        if root.has_error() && walker.parsed.diagnostics.is_empty() {
            walker.parsed.diagnostics.push(Diagnostic {
                kind: DiagnosticKind::ParseError,
                message: "Python syntax error".into(),
                snapshot: role,
                path: file.path.clone(),
                range: Some(walker.range(root)),
            });
        }
        walker.visit(root, 0);
        walker.parsed
    }
}
