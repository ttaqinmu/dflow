pub mod discover;
pub mod resolve;

use crate::analysis::{
    Binding, BindingKind, CallTarget, Diagnostic, DiagnosticKind, ExtractedCallSite,
    FingerprintBuilder, ImportBinding, LanguageId, ParsedFile, Position, Scope, ScopeKind,
    SourceRange, SourceRef, Symbol, SymbolKind,
};
use crate::git::{CapturedFile, CapturedSnapshot, SnapshotRole};
use crate::languages::{DiscoveredUnit, Discovery, LanguageAdapter};
use tree_sitter::{Node, Parser};

pub struct RustAdapter;

#[derive(Clone, Copy, Default)]
struct Availability {
    conditional: bool,
    unknown_attribute: bool,
}
impl Availability {
    fn combine(self, other: Self) -> Self {
        Self {
            conditional: self.conditional || other.conditional,
            unknown_attribute: self.unknown_attribute || other.unknown_attribute,
        }
    }
}

struct Walker<'a> {
    bytes: &'a [u8],
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
            path: self.parsed.path.clone(),
            range: self.range(node),
        }
    }
    fn text(&self, node: Node<'_>) -> &'_ str {
        // UTF-8 was checked before visiting the tree; node boundaries are byte offsets.
        std::str::from_utf8(&self.bytes[node.byte_range()]).expect("validated Rust UTF-8")
    }
    fn children<'a>(&self, node: Node<'a>) -> Vec<Node<'a>> {
        let mut cursor = node.walk();
        node.children(&mut cursor).collect()
    }
    fn named<'a>(&self, node: Node<'a>) -> Vec<Node<'a>> {
        let mut cursor = node.walk();
        node.named_children(&mut cursor).collect()
    }
    fn is_doc(&self, node: Node<'_>) -> bool {
        matches!(node.kind(), "line_comment" | "block_comment")
            && node.child_by_field_name("doc").is_some()
    }
    fn canonical(
        &self,
        node: Node<'_>,
        digest: &mut FingerprintBuilder,
        mode: u8,
        body: Option<usize>,
        in_body: bool,
    ) {
        if matches!(node.kind(), "line_comment" | "block_comment") && !self.is_doc(node) {
            return;
        }
        let in_body = in_body || body == Some(node.id());
        if mode != 0
            && in_body
            && matches!(node.kind(), "function_item" | "function_signature_item")
        {
            digest.push(b"nested_function");
            digest.push(node.kind().as_bytes());
            if let Some(name) = node.child_by_field_name("name") {
                digest.push(&self.bytes[name.byte_range()]);
            }
            return;
        }
        if self.is_doc(node) {
            digest.push(node.kind().as_bytes());
            digest.push(&self.bytes[node.byte_range()]);
            return;
        }
        let mut cursor = node.walk();
        let mut children = node.children(&mut cursor);
        if let Some(first) = children.next() {
            digest.push(node.kind().as_bytes());
            for child in std::iter::once(first).chain(children) {
                if mode == 2
                    && in_body
                    && node.kind() == "call_expression"
                    && node
                        .child_by_field_name("function")
                        .is_some_and(|n| n.id() == child.id())
                {
                    digest.push(b"callee");
                } else {
                    self.canonical(child, digest, mode, body, in_body);
                }
            }
        } else {
            let token = &self.bytes[node.byte_range()];
            if !token.iter().all(u8::is_ascii_whitespace) {
                digest.push(node.kind().as_bytes());
                digest.push(token);
            }
        }
    }
    fn availability(&self, attrs: &[Node<'_>]) -> Availability {
        let mut result = Availability::default();
        for attr in attrs {
            if !matches!(attr.kind(), "attribute_item" | "inner_attribute_item") {
                continue;
            }
            let text = self.text(*attr);
            let key = text
                .trim_start_matches(['#', '!', '[', ' '])
                .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                .next()
                .unwrap_or("");
            if matches!(key, "cfg" | "cfg_attr") {
                result.conditional = true;
            } else if !matches!(
                key,
                "inline"
                    | "cold"
                    | "must_use"
                    | "deprecated"
                    | "allow"
                    | "warn"
                    | "deny"
                    | "forbid"
                    | "doc"
            ) {
                result.unknown_attribute = true;
            }
        }
        result
    }
    fn add_binding(&mut self, scope: usize, name: &str, kind: BindingKind, node: Node<'_>) {
        self.order += 1;
        self.parsed.bindings.push(Binding {
            scope_id: scope,
            name: name.to_owned(),
            kind,
            source: self.source(node),
            order: self.order,
            conditional: false,
            unknown_attribute: false,
        });
    }
    fn pattern(&mut self, node: Node<'_>, scope: usize, kind: BindingKind) {
        match node.kind() {
            "identifier" | "shorthand_field_identifier" => {
                let name = self.text(node).to_owned();
                self.add_binding(scope, &name, kind, node);
            }
            "scoped_identifier" | "scoped_type_identifier" | "type_identifier" => {}
            _ => {
                for child in self.named(node) {
                    self.pattern(child, scope, kind.clone());
                }
            }
        }
    }
    fn params(&mut self, parameters: Node<'_>, scope: usize) {
        for child in self.named(parameters) {
            match child.kind() {
                "parameter" => {
                    if let Some(pattern) = child.child_by_field_name("pattern") {
                        self.pattern(pattern, scope, BindingKind::Parameter);
                    }
                }
                "self_parameter" => self.add_binding(scope, "self", BindingKind::Parameter, child),
                "identifier" | "tuple_pattern" | "reference_pattern" | "slice_pattern"
                | "struct_pattern" | "captured_pattern" => {
                    self.pattern(child, scope, BindingKind::Parameter);
                }
                _ => {}
            }
        }
    }
    fn new_scope(&mut self, parent: usize, name: &str, kind: ScopeKind, node: Node<'_>) -> usize {
        let id = self.parsed.scopes.len();
        let qualified_name = format!("{}::{name}", self.parsed.scopes[parent].qualified_name);
        self.parsed.scopes.push(Scope {
            id,
            parent: Some(parent),
            name: name.to_owned(),
            qualified_name,
            kind,
            source: self.source(node),
        });
        id
    }
    fn definition(
        &mut self,
        node: Node<'_>,
        attrs: &[Node<'_>],
        parent: usize,
        inherited: Availability,
    ) {
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        let name = self.text(name_node).to_owned();
        let scope = self.new_scope(parent, &name, ScopeKind::Function, node);
        let method = matches!(
            &self.parsed.scopes[parent].kind,
            ScopeKind::RustType | ScopeKind::RustTrait
        );
        let async_ = self.named(node).iter().any(|n| {
            n.kind() == "function_modifiers"
                && self.text(*n).split_whitespace().any(|word| word == "async")
        });
        let kind = match (method, async_) {
            (true, true) => SymbolKind::AsyncMethod,
            (true, false) => SymbolKind::Method,
            (false, true) => SymbolKind::AsyncFunction,
            (false, false) => SymbolKind::Function,
        };
        let body = node.child_by_field_name("body").map(|n| n.id());
        let mut full = FingerprintBuilder::new();
        let mut direct = FingerprintBuilder::new();
        let mut non_call = FingerprintBuilder::new();
        for attr in attrs {
            self.canonical(*attr, &mut full, 0, body, false);
            self.canonical(*attr, &mut direct, 1, body, false);
            self.canonical(*attr, &mut non_call, 2, body, false);
        }
        self.canonical(node, &mut full, 0, body, false);
        self.canonical(node, &mut direct, 1, body, false);
        self.canonical(node, &mut non_call, 2, body, false);
        let availability = inherited.combine(self.availability(attrs));
        self.parsed.symbols.push(Symbol {
            unit: self.parsed.unit.clone(),
            language: LanguageId::Rust,
            source: self.source(node),
            scope_id: scope,
            parent_scope: parent,
            name: name.clone(),
            qualified_name: self.parsed.scopes[scope].qualified_name.clone(),
            kind,
            fingerprint: full.finish(),
            direct_fingerprint: direct.finish(),
            non_call_fingerprint: non_call.finish(),
            decorated: false,
            conditional: availability.conditional,
            unknown_attribute: availability.unknown_attribute,
        });
        self.add_binding(parent, &name, BindingKind::Definition, name_node);
        if let Some(params) = node.child_by_field_name("parameters") {
            self.params(params, scope);
        }
        if let Some(body) = node.child_by_field_name("body") {
            self.visit(body, scope, availability);
        }
    }
    fn import_leaf(
        &mut self,
        node: Node<'_>,
        prefix: &str,
        scope: usize,
        availability: Availability,
    ) {
        match node.kind() {
            "use_list" => {
                for child in self.named(node) {
                    self.import_leaf(child, prefix, scope, availability);
                }
            }
            "scoped_use_list" => {
                let path = node.child_by_field_name("path").map(|p| self.text(p));
                let prefix = match path {
                    Some(path) if !prefix.is_empty() => format!("{prefix}::{path}"),
                    Some(path) => path.to_owned(),
                    None => prefix.to_owned(),
                };
                if let Some(list) = node.child_by_field_name("list") {
                    self.import_leaf(list, &prefix, scope, availability);
                }
            }
            "use_wildcard" => {
                let path = self.named(node).first().map(|n| self.text(*n));
                let module = match path {
                    Some(path) if !prefix.is_empty() => format!("{prefix}::{path}"),
                    Some(path) => path.to_owned(),
                    None => prefix.to_owned(),
                };
                self.parsed.imports.push(ImportBinding {
                    scope_id: scope,
                    module,
                    name: None,
                    alias: None,
                    relative_level: 0,
                    wildcard: true,
                    conditional: availability.conditional,
                    unknown_attribute: availability.unknown_attribute,
                    source: self.source(node),
                });
            }
            "use_as_clause" => {
                if let (Some(path), Some(alias)) = (
                    node.child_by_field_name("path"),
                    node.child_by_field_name("alias"),
                ) {
                    let path = self.text(path).to_owned();
                    let alias = self.text(alias).to_owned();
                    self.import_path(&path, Some(&alias), prefix, scope, node, availability);
                }
            }
            "identifier" | "scoped_identifier" | "crate" | "self" | "super" => {
                let path = self.text(node).to_owned();
                self.import_path(&path, None, prefix, scope, node, availability);
            }
            _ => {}
        }
    }
    fn import_path(
        &mut self,
        path: &str,
        alias: Option<&str>,
        prefix: &str,
        scope: usize,
        node: Node<'_>,
        availability: Availability,
    ) {
        let full = if prefix.is_empty() {
            path.to_owned()
        } else {
            format!("{prefix}::{path}")
        };
        let (module, name) = full.rsplit_once("::").unwrap_or(("", full.as_str()));
        // `use foo::{self}` imports foo under its own name.
        let (module, name) = if name == "self" && !module.is_empty() {
            module.rsplit_once("::").unwrap_or(("", module))
        } else {
            (module, name)
        };
        let binding = alias.unwrap_or(name);
        self.parsed.imports.push(ImportBinding {
            scope_id: scope,
            module: module.to_owned(),
            name: Some(name.to_owned()),
            alias: alias.map(str::to_owned),
            relative_level: 0,
            wildcard: false,
            conditional: availability.conditional,
            unknown_attribute: availability.unknown_attribute,
            source: self.source(node),
        });
        self.add_binding(scope, binding, BindingKind::Import, node);
    }
    fn target(&self, node: Node<'_>) -> CallTarget {
        match node.kind() {
            "identifier" | "self" => CallTarget::Name(self.text(node).to_owned()),
            "scoped_identifier" => {
                let text = self.text(node);
                if text
                    .bytes()
                    .any(|byte| matches!(byte, b'<' | b'>' | b'(' | b')'))
                {
                    return CallTarget::Dynamic;
                }
                CallTarget::Qualified(text.split("::").map(str::to_owned).collect())
            }
            "generic_function" => node
                .child_by_field_name("function")
                .map(|inner| self.target(inner))
                .unwrap_or(CallTarget::Dynamic),
            _ => CallTarget::Dynamic,
        }
    }
    fn call(&mut self, node: Node<'_>, scope: usize) {
        let function = node.child_by_field_name("function");
        let target = function
            .map(|f| self.target(f))
            .unwrap_or(CallTarget::Dynamic);
        let expression = function
            .map(|f| self.text(f))
            .unwrap_or_else(|| self.text(node))
            .to_owned();
        self.parsed.calls.push(ExtractedCallSite {
            scope_id: scope,
            expression,
            target,
            source: self.source(node),
        });
    }
    fn macro_call(&mut self, node: Node<'_>, scope: usize) {
        let expression = node
            .child_by_field_name("macro")
            .map(|n| format!("{}!", self.text(n)))
            .unwrap_or_else(|| format!("{}!", self.text(node)));
        self.parsed.calls.push(ExtractedCallSite {
            scope_id: scope,
            expression,
            target: CallTarget::Dynamic,
            source: self.source(node),
        });
        // Token trees are opaque: never infer a definition or call from macro input.
    }
    fn visit_children(&mut self, node: Node<'_>, scope: usize, inherited: Availability) {
        let mut prefix = Vec::new();
        for child in self.named(node) {
            if matches!(child.kind(), "attribute_item" | "inner_attribute_item")
                || self.is_doc(child)
            {
                prefix.push(child);
                continue;
            }
            self.visit_item(child, &prefix, scope, inherited);
            prefix.clear();
        }
    }
    fn visit_item(
        &mut self,
        node: Node<'_>,
        attrs: &[Node<'_>],
        scope: usize,
        inherited: Availability,
    ) {
        let availability = inherited.combine(self.availability(attrs));
        match node.kind() {
            "function_item" | "function_signature_item" => {
                self.definition(node, attrs, scope, inherited)
            }
            "impl_item" => {
                let Some(ty) = node.child_by_field_name("type") else {
                    return;
                };
                let trait_impl = node.child_by_field_name("trait");
                let name = match trait_impl {
                    Some(trait_node) => {
                        format!("impl {} for {}", self.text(trait_node), self.text(ty))
                    }
                    None => self.text(ty).to_owned(),
                };
                let kind = if trait_impl.is_some() {
                    ScopeKind::RustTrait
                } else {
                    ScopeKind::RustType
                };
                let inner = self.new_scope(scope, &name, kind, node);
                if let Some(body) = node.child_by_field_name("body") {
                    self.visit_children(body, inner, availability);
                }
            }
            "trait_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.text(name_node).to_owned();
                    self.add_binding(scope, &name, BindingKind::Definition, name_node);
                    let inner = self.new_scope(scope, &name, ScopeKind::RustTrait, node);
                    if let Some(body) = node.child_by_field_name("body") {
                        self.visit_children(body, inner, availability);
                    }
                }
            }
            "mod_item" => {
                // Discovery visits each inline/external module as its own source unit.
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.text(name_node).to_owned();
                    self.add_binding(scope, &name, BindingKind::Definition, name_node);
                }
            }
            "struct_item" | "enum_item" | "union_item" | "type_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.text(name_node).to_owned();
                    self.add_binding(scope, &name, BindingKind::Definition, name_node);
                    let binding = self.parsed.bindings.last_mut().unwrap();
                    binding.conditional = availability.conditional;
                    binding.unknown_attribute = availability.unknown_attribute;
                }
            }
            "use_declaration" => {
                if let Some(argument) = node.child_by_field_name("argument") {
                    self.import_leaf(argument, "", scope, availability);
                }
            }
            _ => self.visit(node, scope, availability),
        }
    }
    fn visit(&mut self, node: Node<'_>, scope: usize, inherited: Availability) {
        match node.kind() {
            "source_file" | "declaration_list" | "block" => {
                self.visit_children(node, scope, inherited)
            }
            "function_item"
            | "function_signature_item"
            | "impl_item"
            | "trait_item"
            | "mod_item"
            | "use_declaration" => self.visit_item(node, &[], scope, inherited),
            "let_declaration" => {
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    self.pattern(pattern, scope, BindingKind::Assignment);
                }
                for child in self.named(node) {
                    self.visit(child, scope, inherited);
                }
            }
            "closure_expression" => {
                let name = format!(
                    "<closure@{}:{}>",
                    node.start_position().row + 1,
                    node.start_position().column + 1
                );
                let inner = self.new_scope(scope, &name, ScopeKind::Function, node);
                if let Some(params) = node.child_by_field_name("parameters") {
                    self.params(params, inner);
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.visit(body, inner, inherited);
                }
            }
            "call_expression" => {
                self.call(node, scope);
                for child in self.named(node) {
                    self.visit(child, scope, inherited);
                }
            }
            "macro_invocation" => self.macro_call(node, scope),
            "for_expression" | "match_arm" => {
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    self.pattern(pattern, scope, BindingKind::Assignment);
                }
                for child in self.named(node) {
                    self.visit(child, scope, inherited);
                }
            }
            "token_tree" => {}
            _ => {
                for child in self.named(node) {
                    self.visit(child, scope, inherited);
                }
            }
        }
    }
    fn errors(&mut self, node: Node<'_>) {
        if node.is_error() || node.is_missing() {
            self.parsed.valid = false;
            self.parsed.diagnostics.push(Diagnostic {
                kind: DiagnosticKind::ParseError,
                message: format!("Rust syntax error near {}", node.kind()),
                snapshot: self.role,
                path: self.parsed.path.clone(),
                range: Some(self.range(node)),
            });
        }
        for child in self.children(node) {
            self.errors(child);
        }
    }
}

impl LanguageAdapter for RustAdapter {
    fn accepts_source(&self, path: &[u8]) -> bool {
        path.ends_with(b".rs")
    }
    fn accepts_metadata(&self, path: &[u8]) -> bool {
        path == b"Cargo.toml" || path.ends_with(b"/Cargo.toml")
    }
    fn discover(&self, snapshot: &CapturedSnapshot) -> Discovery {
        discover::discover(snapshot)
    }
    fn extract(
        &self,
        file: &CapturedFile,
        role: SnapshotRole,
        unit: &DiscoveredUnit,
    ) -> ParsedFile {
        let mut parsed = ParsedFile {
            path: file.path.clone(),
            unit: unit.key.clone(),
            module: unit.module.clone(),
            language: LanguageId::Rust,
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
                message: "Rust source must be UTF-8 or UTF-8 BOM".into(),
                snapshot: role,
                path: file.path.clone(),
                range: None,
            });
            return parsed;
        }
        let mut parser = Parser::new();
        if let Err(error) = parser.set_language(&tree_sitter_rust::LANGUAGE.into()) {
            parsed.valid = false;
            parsed.diagnostics.push(Diagnostic {
                kind: DiagnosticKind::ParseError,
                message: format!("Rust parser unavailable: {error}"),
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
                message: "Rust parse cancelled".into(),
                snapshot: role,
                path: file.path.clone(),
                range: None,
            });
            return parsed;
        };
        let root = tree.root_node();
        let selected = if let Some((start, end)) = unit.subtree {
            fn find(node: Node<'_>, start: usize, end: usize) -> Option<Node<'_>> {
                if node.kind() == "declaration_list"
                    && node.start_byte() == start
                    && node.end_byte() == end
                {
                    return Some(node);
                }
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if child.start_byte() <= start
                        && end <= child.end_byte()
                        && let Some(found) = find(child, start, end)
                    {
                        return Some(found);
                    }
                }
                None
            }
            find(root, start, end)
        } else {
            Some(root)
        };
        let Some(selected) = selected else {
            parsed.valid = false;
            parsed.diagnostics.push(Diagnostic {
                kind: DiagnosticKind::UnsupportedLayout,
                message: "Inline module body not found in captured Rust source".into(),
                snapshot: role,
                path: file.path.clone(),
                range: None,
            });
            return parsed;
        };
        let module = unit.module.as_deref().unwrap_or(&unit.key.module);
        let qualified_name = format!("{}::{module}", unit.key.target);
        parsed.scopes.push(Scope {
            id: 0,
            parent: None,
            name: module.to_owned(),
            qualified_name,
            kind: ScopeKind::Module,
            source: SourceRef {
                snapshot: role,
                path: file.path.clone(),
                range: SourceRange {
                    start: Position {
                        line: selected.start_position().row + 1,
                        byte_column: selected.start_position().column + 1,
                    },
                    end: Position {
                        line: selected.end_position().row + 1,
                        byte_column: selected.end_position().column + 1,
                    },
                    start_byte: selected.start_byte(),
                    end_byte: selected.end_byte(),
                },
            },
        });
        let mut walker = Walker {
            bytes,
            role,
            parsed,
            order: 0,
        };
        walker.errors(selected);
        if selected.has_error() && walker.parsed.diagnostics.is_empty() {
            walker.parsed.valid = false;
            walker.parsed.diagnostics.push(Diagnostic {
                kind: DiagnosticKind::ParseError,
                message: "Rust syntax error".into(),
                snapshot: role,
                path: file.path.clone(),
                range: Some(walker.range(selected)),
            });
        }
        walker.visit(
            selected,
            0,
            Availability {
                conditional: unit.conditional,
                unknown_attribute: unit.unknown_attribute,
            },
        );
        walker.parsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::SourceUnitKey;
    use crate::git::SnapshotRole;

    fn extract_unit(
        source: &str,
        module: &str,
        target: &str,
        subtree: Option<(usize, usize)>,
    ) -> ParsedFile {
        let file = CapturedFile {
            path: b"src/lib.rs".to_vec(),
            bytes: source.as_bytes().to_vec(),
            mode: 0o100644,
        };
        let unit = DiscoveredUnit {
            key: SourceUnitKey {
                path: file.path.clone(),
                target: target.into(),
                module: module.into(),
            },
            module: Some(module.into()),
            subtree,
            conditional: false,
            unknown_attribute: false,
        };
        RustAdapter.extract(&file, SnapshotRole::Target, &unit)
    }
    fn extract(source: &str, subtree: Option<(usize, usize)>) -> ParsedFile {
        extract_unit(source, "crate", "Cargo.toml:lib:demo", subtree)
    }
    #[test]
    fn parser_fields_cover_items_calls_and_modules() {
        let source = "mod inner { fn nested() {} } impl Thing { async fn make(&self) { crate::free(); thing.method(); println!(\"x\"); } } trait Trait { fn required(&self); } fn free() {}";
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        let mut seen = Vec::new();
        fn inspect(node: Node<'_>, seen: &mut Vec<&'static str>) {
            match node.kind() {
                "mod_item" => {
                    assert!(node.child_by_field_name("name").is_some());
                    assert!(node.child_by_field_name("body").is_some());
                    seen.push("module");
                }
                "impl_item" => {
                    assert!(node.child_by_field_name("type").is_some());
                    assert!(node.child_by_field_name("body").is_some());
                    seen.push("impl");
                }
                "function_item" => {
                    assert!(node.child_by_field_name("name").is_some());
                    assert!(node.child_by_field_name("parameters").is_some());
                    seen.push("function");
                }
                "function_signature_item" => {
                    assert!(node.child_by_field_name("name").is_some());
                    seen.push("signature");
                }
                "call_expression" => {
                    assert!(node.child_by_field_name("function").is_some());
                    seen.push("call");
                }
                "macro_invocation" => {
                    assert!(node.child_by_field_name("macro").is_some());
                    seen.push("macro");
                }
                _ => {}
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                inspect(child, seen);
            }
        }
        inspect(tree.root_node(), &mut seen);
        for expected in ["module", "impl", "function", "signature", "call", "macro"] {
            assert!(seen.contains(&expected), "missing {expected}");
        }
    }
    #[test]
    fn fixture_has_independent_names_scopes_ranges_and_calls() {
        let text = include_str!("../../../tests/fixtures/rust/analysis.rs");
        let expected: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/rust/analysis.expected.json"
        ))
        .unwrap();
        let target = expected["target"].as_str().unwrap();
        let root = extract_unit(text, "crate", target, None);
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(text, None).unwrap();
        let mut cursor = tree.root_node().walk();
        let body = tree
            .root_node()
            .named_children(&mut cursor)
            .find(|n| n.kind() == "mod_item")
            .unwrap()
            .child_by_field_name("body")
            .unwrap();
        let inline = extract_unit(
            text,
            "crate::inline",
            target,
            Some((body.start_byte(), body.end_byte())),
        );
        assert!(
            root.valid && inline.valid,
            "{:?} {:?}",
            root.diagnostics,
            inline.diagnostics
        );
        let symbols = root
            .symbols
            .iter()
            .chain(inline.symbols.iter())
            .collect::<Vec<_>>();
        assert_eq!(symbols.len(), expected["symbols"].as_array().unwrap().len());
        for entry in expected["symbols"].as_array().unwrap() {
            let name = entry["name"].as_str().unwrap();
            let module = entry["module"].as_str().unwrap();
            let symbol = symbols
                .iter()
                .find(|s| s.name == name && s.unit.module == module)
                .unwrap();
            let kind = match symbol.kind {
                SymbolKind::Function => "function",
                SymbolKind::AsyncFunction => "async_function",
                SymbolKind::Method => "method",
                SymbolKind::AsyncMethod => "async_method",
            };
            assert_eq!(kind, entry["kind"].as_str().unwrap());
            assert_eq!(
                symbol.source.range.start.line,
                entry["start_line"].as_u64().unwrap() as usize
            );
            assert_eq!(
                symbol.source.range.start.byte_column,
                entry["start_byte_column"].as_u64().unwrap() as usize
            );
            assert!(symbol.qualified_name.starts_with(target));
        }
        let expressions = root
            .calls
            .iter()
            .chain(inline.calls.iter())
            .map(|c| c.expression.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            expressions,
            expected["calls"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn extracts_separate_inline_module_and_preserves_byte_columns() {
        let text = "fn outer() {}\nmod child { fn inner() { crate::outer(); } }";
        let root = extract(text, None);
        assert_eq!(
            root.symbols
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["outer"]
        );
        let start = text.find('{').unwrap();
        let start = text[start + 1..].find('{').unwrap() + start + 1;
        let end = text.len();
        let inner = extract(text, Some((start, end)));
        assert_eq!(
            inner
                .symbols
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["inner"]
        );
        assert_eq!(inner.calls[0].source.range.start.line, 2);
        assert_eq!(
            inner.calls[0].target,
            CallTarget::Qualified(vec!["crate".into(), "outer".into()])
        );
    }
    #[test]
    fn fingerprint_keeps_documentation_attributes_and_literals_not_layout() {
        let a = extract("/// docs\n#[inline]\nfn f() { let n = 1; }", None);
        let b = extract(
            "/// docs\n#[inline]\nfn f(){ /* ordinary */ let n=1; }",
            None,
        );
        let c = extract("/// changed\n#[inline]\nfn f() { let n = 1; }", None);
        let d = extract("/// docs\n#[inline]\nfn f() { let n = 2; }", None);
        assert_eq!(a.symbols[0].fingerprint, b.symbols[0].fingerprint);
        assert_ne!(a.symbols[0].fingerprint, c.symbols[0].fingerprint);
        assert_ne!(a.symbols[0].fingerprint, d.symbols[0].fingerprint);
        assert!(!a.symbols[0].unknown_attribute);
    }
    #[test]
    fn direct_and_non_call_fingerprints_separate_rust_edits() {
        let symbol = |source| extract(source, None).symbols[0].clone();
        let original = symbol("/// docs\nfn f(x: i32) -> i32 { fn inner() -> i32 { 1 } B(x) }");
        let nested = symbol("/// docs\nfn f(x: i32) -> i32 { fn inner() -> i32 { 2 } B(x) }");
        assert_eq!(original.direct_fingerprint, nested.direct_fingerprint);
        let retarget = symbol("/// docs\nfn f(x: i32) -> i32 { fn inner() -> i32 { 1 } C(x) }");
        assert_ne!(original.direct_fingerprint, retarget.direct_fingerprint);
        assert_eq!(original.non_call_fingerprint, retarget.non_call_fingerprint);
        for changed in [
            "/// new\nfn f(x: i32) -> i32 { fn inner() -> i32 { 1 } B(x) }",
            "/// docs\nfn f(x: u32) -> i32 { fn inner() -> i32 { 1 } B(x) }",
            "/// docs\nfn f(x: i32) -> i32 { fn inner() -> i32 { 1 } B(2) }",
            "/// docs\nfn f(x: i32) -> i32 { fn inner() -> i32 { 1 } return B(x); }",
        ] {
            assert_ne!(
                original.non_call_fingerprint,
                symbol(changed).non_call_fingerprint
            );
        }
        assert_eq!(
            original.direct_fingerprint,
            symbol("/// docs\nfn f( x : i32 )->i32{/* ordinary */fn inner()->i32{1}B(x)}")
                .direct_fingerprint
        );
    }
}
