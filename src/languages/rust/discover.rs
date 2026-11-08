use crate::analysis::{Diagnostic, DiagnosticKind, DiscoveredUnit, Discovery, SourceUnitKey};
use crate::git::CapturedSnapshot;
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::{Node, Parser};

/// Resolve only paths wholly inside the captured repository. In particular, never use the
/// process working directory to decide which module or Cargo target exists.
fn relative_path(base: &[u8], path: &str) -> Option<Vec<u8>> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return None;
    }
    let mut parts: Vec<&[u8]> = base
        .split(|&c| c == b'/')
        .filter(|p| !p.is_empty())
        .collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ if part.as_bytes().contains(&0) || part.contains(':') => return None,
            _ => parts.push(part.as_bytes()),
        }
    }
    (!parts.is_empty()).then(|| parts.join(&b'/'))
}

fn parent(path: &[u8]) -> &[u8] {
    path.iter()
        .rposition(|&c| c == b'/')
        .map_or(b"", |index| &path[..index])
}

fn append(dir: &[u8], suffix: &str) -> Vec<u8> {
    relative_path(dir, suffix).expect("known relative suffix")
}

fn manifest_identity(path: &[u8]) -> String {
    let mut result = String::new();
    for &byte in path {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'-' | b'_') {
            result.push(byte as char);
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}

fn source(file: &crate::git::CapturedFile) -> Result<&str, ()> {
    let bytes = file
        .bytes
        .strip_prefix(b"\xef\xbb\xbf")
        .unwrap_or(&file.bytes);
    std::str::from_utf8(bytes).map_err(|_| ())
}

type ModuleSources = BTreeMap<(String, String), (Vec<u8>, Option<(usize, usize)>)>;

struct Finder<'a> {
    snapshot: &'a CapturedSnapshot,
    result: Discovery,
    visited: BTreeSet<SourceUnitKey>,
    modules: ModuleSources,
}

impl Finder<'_> {
    fn issue(&mut self, kind: DiagnosticKind, path: &[u8], message: impl Into<String>) {
        self.result.incomplete = true;
        self.result.diagnostics.push(Diagnostic {
            kind,
            message: message.into(),
            snapshot: self.snapshot.role,
            path: path.to_vec(),
            range: None,
        });
    }

    fn manifest(&mut self, path: &[u8]) -> Option<toml::Value> {
        let Some(file) = self.snapshot.files.get(path) else {
            self.issue(
                DiagnosticKind::UnsupportedLayout,
                path,
                "workspace member manifest is missing",
            );
            return None;
        };
        let Ok(text) = source(file) else {
            self.issue(
                DiagnosticKind::InvalidEncoding,
                path,
                "manifest is not UTF-8",
            );
            return None;
        };
        match text.parse() {
            Ok(value) => Some(value),
            Err(err) => {
                self.issue(
                    DiagnosticKind::UnsupportedLayout,
                    path,
                    format!("invalid Cargo manifest: {err}"),
                );
                None
            }
        }
    }

    fn targets(&mut self, manifest_path: &[u8], value: &toml::Value) {
        let Some(package) = value.get("package") else {
            return;
        };
        let Some(package) = package.as_table() else {
            self.issue(
                DiagnosticKind::UnsupportedLayout,
                manifest_path,
                "[package] must be a table",
            );
            return;
        };
        let Some(package_name) = package
            .get("name")
            .and_then(toml::Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            self.issue(
                DiagnosticKind::UnsupportedLayout,
                manifest_path,
                "[package].name must be a concrete string (not workspace-inherited)",
            );
            return;
        };
        let base = parent(manifest_path);
        let package_name = package_name.replace('-', "_");
        let mut targets: Vec<(String, Vec<u8>, bool)> = Vec::new();
        if let Some(lib) = value.get("lib") {
            if let Some(lib) = lib.as_table() {
                let name = match lib.get("name") {
                    Some(v) => v.as_str().filter(|s| !s.is_empty()),
                    None => Some(package_name.as_str()),
                };
                let path = match lib.get("path") {
                    Some(v) => v.as_str(),
                    None => Some("src/lib.rs"),
                };
                match (name, path) {
                    (Some(name), Some(path)) => {
                        targets.push((format!("lib:{name}"), path.as_bytes().to_vec(), true))
                    }
                    _ => self.issue(
                        DiagnosticKind::UnsupportedLayout,
                        manifest_path,
                        "[lib] needs a concrete name and path",
                    ),
                }
            } else {
                self.issue(
                    DiagnosticKind::UnsupportedLayout,
                    manifest_path,
                    "[lib] must be a table",
                );
            }
        } else if self
            .snapshot
            .files
            .contains_key(&append(base, "src/lib.rs"))
        {
            targets.push((format!("lib:{package_name}"), b"src/lib.rs".to_vec(), false));
        }
        let bins = match value.get("bin") {
            Some(v) => match v.as_array() {
                Some(bins) => bins.as_slice(),
                None => {
                    self.issue(
                        DiagnosticKind::UnsupportedLayout,
                        manifest_path,
                        "[[bin]] must be an array of tables",
                    );
                    &[]
                }
            },
            None => &[],
        };
        let mut explicit_main = false;
        for bin in bins {
            let Some(bin) = bin.as_table() else {
                self.issue(
                    DiagnosticKind::UnsupportedLayout,
                    manifest_path,
                    "[[bin]] must be a table",
                );
                continue;
            };
            let path = bin.get("path").and_then(toml::Value::as_str);
            let name = bin
                .get("name")
                .and_then(toml::Value::as_str)
                .filter(|n| !n.is_empty())
                .or_else(|| path.and_then(|p| p.rsplit('/').next()?.strip_suffix(".rs")));
            let Some(name) = name else {
                self.issue(
                    DiagnosticKind::UnsupportedLayout,
                    manifest_path,
                    "[[bin]] needs a concrete name or .rs path",
                );
                continue;
            };
            let default = format!("src/bin/{name}.rs");
            let selected = match path {
                Some(p) => p,
                None if name == package_name => "src/main.rs",
                None => &default,
            };
            if selected == "src/main.rs" {
                explicit_main = true;
            }
            targets.push((format!("bin:{name}"), selected.as_bytes().to_vec(), true));
        }
        if !explicit_main
            && self
                .snapshot
                .files
                .contains_key(&append(base, "src/main.rs"))
        {
            // An explicit binary with the package name replaces its conventional main target.
            if !targets
                .iter()
                .any(|(name, _, _)| name == &format!("bin:{package_name}"))
            {
                targets.push((
                    format!("bin:{package_name}"),
                    b"src/main.rs".to_vec(),
                    false,
                ));
            }
        }
        let mut identities = BTreeSet::new();
        for (kind_name, path, explicit) in targets {
            if !identities.insert(kind_name.clone()) {
                self.issue(
                    DiagnosticKind::ModuleAmbiguity,
                    manifest_path,
                    format!("duplicate Cargo target {kind_name}"),
                );
                continue;
            }
            let Ok(relative) = std::str::from_utf8(&path) else {
                self.issue(
                    DiagnosticKind::InvalidEncoding,
                    manifest_path,
                    "target path is not UTF-8",
                );
                continue;
            };
            let Some(root) = relative_path(base, relative) else {
                self.issue(
                    DiagnosticKind::UnsupportedLayout,
                    manifest_path,
                    format!("target path escapes repository: {relative}"),
                );
                continue;
            };
            if !self.snapshot.files.contains_key(&root) {
                if explicit {
                    self.issue(
                        DiagnosticKind::UnsupportedLayout,
                        &root,
                        format!("missing Cargo target source for {kind_name}"),
                    );
                }
                continue;
            }
            let target = format!("{}:{kind_name}", manifest_identity(manifest_path));
            self.visit(
                root.clone(),
                target,
                "crate".into(),
                None,
                parent(&root).to_vec(),
                false,
                false,
                Vec::new(),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn visit(
        &mut self,
        path: Vec<u8>,
        target: String,
        module: String,
        subtree: Option<(usize, usize)>,
        directory: Vec<u8>,
        conditional: bool,
        unknown_attribute: bool,
        mut active_files: Vec<Vec<u8>>,
    ) {
        let key = SourceUnitKey {
            path: path.clone(),
            target: target.clone(),
            module: module.clone(),
        };
        if let Some((old_path, old_subtree)) = self.modules.get(&(target.clone(), module.clone())) {
            let message = format!(
                "module {module} declared more than once (previous source {}, subtree {old_subtree:?})",
                String::from_utf8_lossy(old_path)
            );
            self.issue(DiagnosticKind::ModuleAmbiguity, &path, message);
            return;
        }
        self.modules
            .insert((target.clone(), module.clone()), (path.clone(), subtree));
        if !self.visited.insert(key.clone()) {
            return;
        }
        self.result.units.push(DiscoveredUnit {
            key,
            module: Some(module.clone()),
            subtree,
            conditional,
            unknown_attribute,
        });
        // Inline modules share this file with their parent; external recursion through the same
        // file, however, would otherwise create an unbounded series of module identities.
        if subtree.is_none() {
            if active_files.contains(&path) {
                self.issue(
                    DiagnosticKind::UnsupportedLayout,
                    &path,
                    "cyclic external module path",
                );
                return;
            }
            active_files.push(path.clone());
        }
        let Some(file) = self.snapshot.files.get(&path) else {
            return;
        };
        if source(file).is_err() {
            self.issue(
                DiagnosticKind::InvalidEncoding,
                &path,
                "Rust source is not UTF-8",
            );
            return;
        }
        let bytes = &file.bytes;
        let mut parser = Parser::new();
        if parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .is_err()
        {
            self.issue(
                DiagnosticKind::UnsupportedLayout,
                &path,
                "Rust grammar could not be initialized",
            );
            return;
        }
        let Some(tree) = parser.parse(bytes, None) else {
            self.issue(
                DiagnosticKind::ParseError,
                &path,
                "Rust source could not be parsed",
            );
            return;
        };
        let root = tree.root_node();
        if root.has_error() {
            self.issue(
                DiagnosticKind::ParseError,
                &path,
                "Rust source contains syntax errors",
            );
        }
        let body = match subtree {
            None => root,
            Some((start, end)) => {
                let Some(node) = root.descendant_for_byte_range(start, end) else {
                    self.issue(
                        DiagnosticKind::ParseError,
                        &path,
                        "inline module body not found",
                    );
                    return;
                };
                if node.kind() != "declaration_list"
                    || node.start_byte() != start
                    || node.end_byte() != end
                {
                    self.issue(
                        DiagnosticKind::ParseError,
                        &path,
                        "inline module body is not a declaration list",
                    );
                    return;
                }
                node
            }
        };
        let mut inherited_conditional = conditional;
        let mut inherited_unknown_attribute = unknown_attribute;
        let mut cursor = body.walk();
        let mut pending = Vec::new();
        for item in body.named_children(&mut cursor) {
            match item.kind() {
                "attribute_item" => {
                    pending.push(item);
                    continue;
                }
                "line_comment" | "block_comment" => continue,
                "inner_attribute_item" => {
                    let name = attribute_name(item, bytes);
                    if matches!(name, Some("cfg" | "cfg_attr")) {
                        inherited_conditional = true;
                    }
                    if uncertain_attribute(name) {
                        inherited_unknown_attribute = true;
                    }
                    if let Some(unit) = self.result.units.iter_mut().rev().find(|u| {
                        u.key.path == path && u.key.target == target && u.key.module == module
                    }) {
                        unit.conditional = inherited_conditional;
                        unit.unknown_attribute = inherited_unknown_attribute;
                    }
                    continue;
                }
                "mod_item" => {
                    let module_conditional = inherited_conditional
                        || pending.iter().any(|attr| {
                            matches!(attribute_name(*attr, bytes), Some("cfg" | "cfg_attr"))
                        });
                    let module_unknown_attribute = inherited_unknown_attribute
                        || pending
                            .iter()
                            .any(|attr| uncertain_attribute(attribute_name(*attr, bytes)));
                    self.module_item(
                        item,
                        &pending,
                        bytes,
                        &directory,
                        &path,
                        &target,
                        &module,
                        module_conditional,
                        module_unknown_attribute,
                        &active_files,
                    );
                }
                _ => {}
            }
            pending.clear();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn module_item(
        &mut self,
        item: Node<'_>,
        attributes: &[Node<'_>],
        bytes: &[u8],
        directory: &[u8],
        source_path: &[u8],
        target: &str,
        parent_module: &str,
        conditional: bool,
        unknown_attribute: bool,
        active_files: &[Vec<u8>],
    ) {
        let Some(name_node) = item.child_by_field_name("name") else {
            self.issue(
                DiagnosticKind::ParseError,
                source_path,
                "module declaration has no name",
            );
            return;
        };
        let Ok(name) = std::str::from_utf8(&bytes[name_node.byte_range()]) else {
            self.issue(
                DiagnosticKind::InvalidEncoding,
                source_path,
                "module name is not UTF-8",
            );
            return;
        };
        let child_module = format!("{parent_module}::{name}");
        let mut path_attribute = None;
        for attr in attributes {
            match attribute_name(*attr, bytes) {
                Some("path") => {
                    if path_attribute.is_some() {
                        self.issue(
                            DiagnosticKind::ModuleAmbiguity,
                            source_path,
                            format!("multiple #[path] attributes on {child_module}"),
                        );
                        return;
                    }
                    let Some(attribute) = attr.named_child(0) else {
                        continue;
                    };
                    let Some(value) = attribute.child_by_field_name("value") else {
                        self.issue(
                            DiagnosticKind::UnsupportedLayout,
                            source_path,
                            format!("nonliteral #[path] for {child_module}"),
                        );
                        return;
                    };
                    let Some(path) = literal_path(&bytes[value.byte_range()]) else {
                        self.issue(
                            DiagnosticKind::UnsupportedLayout,
                            source_path,
                            format!("nonliteral #[path] for {child_module}"),
                        );
                        return;
                    };
                    path_attribute = Some(path);
                }
                Some("cfg_attr") if bytes[attr.byte_range()].windows(4).any(|w| w == b"path") => {
                    self.issue(
                        DiagnosticKind::UnsupportedLayout,
                        source_path,
                        format!("configuration-dependent #[path] for {child_module}"),
                    );
                    return;
                }
                _ => {}
            }
        }
        if let Some(body) = item.child_by_field_name("body") {
            if path_attribute.is_some() {
                self.issue(
                    DiagnosticKind::UnsupportedLayout,
                    source_path,
                    format!("inline module {child_module} has #[path]"),
                );
                return;
            }
            let inline_dir = append(directory, name);
            self.visit(
                source_path.to_vec(),
                target.to_owned(),
                child_module,
                Some((body.start_byte(), body.end_byte())),
                inline_dir,
                conditional,
                unknown_attribute,
                active_files.to_vec(),
            );
            return;
        }
        let child = if let Some(attribute) = path_attribute {
            let Some(path) = relative_path(directory, &attribute) else {
                self.issue(
                    DiagnosticKind::UnsupportedLayout,
                    source_path,
                    format!("module path escapes repository: {attribute}"),
                );
                return;
            };
            path
        } else {
            let direct = append(directory, &format!("{name}.rs"));
            let nested = append(directory, &format!("{name}/mod.rs"));
            match (
                self.snapshot.files.contains_key(&direct),
                self.snapshot.files.contains_key(&nested),
            ) {
                (true, true) => {
                    self.issue(
                        DiagnosticKind::ModuleAmbiguity,
                        source_path,
                        format!(
                            "both {} and {} declare {child_module}",
                            String::from_utf8_lossy(&direct),
                            String::from_utf8_lossy(&nested)
                        ),
                    );
                    return;
                }
                (true, false) => direct,
                (false, true) => nested,
                (false, false) => {
                    self.issue(
                        DiagnosticKind::UnsupportedLayout,
                        source_path,
                        format!(
                            "missing module {child_module}: neither {} nor {} captured",
                            String::from_utf8_lossy(&direct),
                            String::from_utf8_lossy(&nested)
                        ),
                    );
                    return;
                }
            }
        };
        if !self.snapshot.files.contains_key(&child) {
            self.issue(
                DiagnosticKind::UnsupportedLayout,
                source_path,
                format!("missing module source {}", String::from_utf8_lossy(&child)),
            );
            return;
        }
        let child_dir = if child.ends_with(b"/mod.rs") {
            parent(&child).to_vec()
        } else {
            child.strip_suffix(b".rs").unwrap_or(&child).to_vec()
        };
        self.visit(
            child,
            target.to_owned(),
            child_module,
            None,
            child_dir,
            conditional,
            unknown_attribute,
            active_files.to_vec(),
        );
    }
}
fn uncertain_attribute(name: Option<&str>) -> bool {
    !matches!(
        name,
        Some(
            "cfg"
                | "cfg_attr"
                | "path"
                | "inline"
                | "cold"
                | "must_use"
                | "deprecated"
                | "allow"
                | "warn"
                | "deny"
                | "forbid"
                | "doc"
        )
    )
}

fn attribute_name<'a>(item: Node<'_>, bytes: &'a [u8]) -> Option<&'a str> {
    let attr = item.named_child(0)?;
    let name = attr.named_child(0)?;
    std::str::from_utf8(&bytes[name.byte_range()]).ok()
}

fn literal_path(value: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(value).ok()?;
    let raw = if let Some(rest) = text.strip_prefix('r') {
        let hashes = rest.bytes().take_while(|&c| c == b'#').count();
        let close = format!("\"{}", "#".repeat(hashes));
        rest.get(hashes..)?
            .strip_prefix('"')?
            .strip_suffix(&close)?
    } else {
        let quoted = text.strip_prefix('"')?.strip_suffix('"')?;
        if quoted.contains('\\') {
            return None;
        }
        quoted
    };
    Some(raw.to_owned())
}

pub(super) fn discover(snapshot: &CapturedSnapshot) -> Discovery {
    if !snapshot
        .files
        .keys()
        .any(|path| path.ends_with(b"Cargo.toml") || path.ends_with(b".rs"))
    {
        return Discovery::default();
    }
    let mut finder = Finder {
        snapshot,
        result: Discovery::default(),
        visited: BTreeSet::new(),
        modules: BTreeMap::new(),
    };
    let mut manifests = BTreeSet::new();
    if let Some(root) = finder.manifest(b"Cargo.toml") {
        manifests.insert(b"Cargo.toml".to_vec());
        if let Some(workspace) = root.get("workspace")
            && let Some(members) = workspace.get("members")
        {
            if let Some(members) = members.as_array() {
                for member in members {
                    let Some(member) = member.as_str() else {
                        finder.issue(
                            DiagnosticKind::UnsupportedLayout,
                            b"Cargo.toml",
                            "workspace member must be a literal path",
                        );
                        continue;
                    };
                    if member.chars().any(|c| matches!(c, '*' | '?' | '[' | ']')) {
                        finder.issue(
                            DiagnosticKind::UnsupportedLayout,
                            b"Cargo.toml",
                            format!("workspace glob member is unsupported: {member}"),
                        );
                        continue;
                    }
                    let Some(directory) = relative_path(b"", member) else {
                        finder.issue(
                            DiagnosticKind::UnsupportedLayout,
                            b"Cargo.toml",
                            format!("workspace member escapes repository: {member}"),
                        );
                        continue;
                    };
                    manifests.insert(append(&directory, "Cargo.toml"));
                }
            } else {
                finder.issue(
                    DiagnosticKind::UnsupportedLayout,
                    b"Cargo.toml",
                    "workspace.members must be an array of literal paths",
                );
            }
        }
        finder.targets(b"Cargo.toml", &root);
    }
    for manifest in manifests
        .iter()
        .filter(|path| path.as_slice() != b"Cargo.toml")
    {
        if let Some(value) = finder.manifest(manifest) {
            finder.targets(manifest, &value);
        }
    }
    for path in snapshot.files.keys() {
        if path.ends_with(b"Cargo.toml") && path != b"Cargo.toml" && !manifests.contains(path) {
            finder.issue(
                DiagnosticKind::UnsupportedLayout,
                path,
                "Cargo manifest is not a literal root workspace member",
            );
        }
        if path.ends_with(b".rs") && !finder.visited.iter().any(|key| &key.path == path) {
            finder.issue(
                DiagnosticKind::UnsupportedLayout,
                path,
                "Rust source is not reachable from a captured Cargo target/module",
            );
        }
    }
    finder.result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{CapturedFile, SnapshotKind, SnapshotRole};

    fn captured(files: &[(&str, &str)]) -> CapturedSnapshot {
        CapturedSnapshot {
            kind: SnapshotKind::WorkingTree,
            role: SnapshotRole::Target,
            commit_id: None,
            branch: None,
            content_id: String::new(),
            files: files
                .iter()
                .map(|(path, text)| {
                    let path = path.as_bytes().to_vec();
                    (
                        path.clone(),
                        CapturedFile {
                            path,
                            bytes: text.as_bytes().to_vec(),
                            mode: 0o100644,
                        },
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn workspace_targets_shared_file_and_inline_external_modules() {
        let snapshot = captured(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"member\"]\n"),
            (
                "member/Cargo.toml",
                "[package]\nname = \"shared\"\nversion = \"0.1.0\"\n[lib]\npath = \"src/shared.rs\"\n[[bin]]\nname = \"cli\"\npath = \"src/shared.rs\"\n",
            ),
            (
                "member/src/shared.rs",
                "#[cfg(feature = \"x\")] mod hidden { mod child; }\n#[path = \"other.rs\"] mod other;\n",
            ),
            ("member/src/hidden/child.rs", "pub fn leaf() {}\n"),
            ("member/src/other.rs", "pub fn other() {}\n"),
        ]);
        let result = discover(&snapshot);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert!(!result.incomplete);
        assert_eq!(result.units.len(), 8);
        for target in ["member/Cargo.toml:lib:shared", "member/Cargo.toml:bin:cli"] {
            let modules: BTreeSet<_> = result
                .units
                .iter()
                .filter(|u| u.key.target == target)
                .map(|u| (u.key.module.as_str(), u.key.path.as_slice(), u.conditional))
                .collect();
            assert!(modules.contains(&("crate", b"member/src/shared.rs".as_slice(), false)));
            assert!(modules.contains(&("crate::hidden", b"member/src/shared.rs".as_slice(), true)));
            assert!(modules.contains(&(
                "crate::hidden::child",
                b"member/src/hidden/child.rs".as_slice(),
                true
            )));
            assert!(modules.contains(&("crate::other", b"member/src/other.rs".as_slice(), false)));
            let body = result
                .units
                .iter()
                .find(|u| u.key.target == target && u.key.module == "crate::hidden")
                .unwrap()
                .subtree
                .unwrap();
            assert_eq!(
                &snapshot.files[b"member/src/shared.rs".as_slice()].bytes[body.0..body.1],
                b"{ mod child; }"
            );
        }
    }

    #[test]
    fn ambiguous_missing_escaping_and_uncovered_sources_are_partial() {
        let snapshot = captured(&[
            (
                "Cargo.toml",
                "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
            ),
            (
                "src/lib.rs",
                "mod both; mod absent; #[path = \"../../escape.rs\"] mod outside;",
            ),
            ("src/both.rs", ""),
            ("src/both/mod.rs", ""),
            ("src/loose.rs", ""),
        ]);
        let result = discover(&snapshot);
        assert!(result.incomplete);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::ModuleAmbiguity)
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("escapes repository"))
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("missing module crate::absent"))
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.path.as_slice() == b"src/loose.rs")
        );
    }

    #[test]
    fn invalid_manifests_members_and_explicit_target_paths_are_partial() {
        let bad = captured(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"member*\"]\n"),
            ("src/lib.rs", "pub fn unrelated() {}"),
        ]);
        assert!(discover(&bad).incomplete);
        let bad = captured(&[
            (
                "Cargo.toml",
                "[package]\nname.workspace = true\n[lib]\npath = \"src/lib.rs\"",
            ),
            ("src/lib.rs", "pub fn unrelated() {}"),
        ]);
        assert!(
            discover(&bad)
                .diagnostics
                .iter()
                .any(|d| d.message.contains("concrete string"))
        );
        let bad = captured(&[
            (
                "Cargo.toml",
                "[package]\nname = \"x\"\n[lib]\npath = \"../outside.rs\"",
            ),
            ("src/lib.rs", "pub fn unrelated() {}"),
        ]);
        assert!(
            discover(&bad)
                .diagnostics
                .iter()
                .any(|d| d.message.contains("escapes repository"))
        );
        let bad = captured(&[("Cargo.toml", "[package\nname = \"x\""), ("src/lib.rs", "")]);
        assert!(
            discover(&bad)
                .diagnostics
                .iter()
                .any(|d| d.message.contains("invalid Cargo manifest"))
        );
    }
}
