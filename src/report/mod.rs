use crate::analysis::{self, ChangeAnalysis, ExtractedSnapshot, SourceRange, SymbolChangeKind};
use crate::cli::CompareMode;
use crate::git::{CaptureResult, CapturedSnapshot, ChangeKind, SnapshotKind, SnapshotRole};
use crate::graph::{self, SnapshotGraph};
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use serde_json::{Value, json};

pub struct AnalysisSession {
    pub capture: CaptureResult,
    pub base: ExtractedSnapshot,
    pub target: ExtractedSnapshot,
    pub changes: ChangeAnalysis,
    pub base_graph: SnapshotGraph,
    pub target_graph: SnapshotGraph,
}
impl AnalysisSession {
    pub fn new(capture: CaptureResult) -> Self {
        let base = analysis::extract_snapshot(&capture.pair.base);
        let target = analysis::extract_snapshot(&capture.pair.target);
        let changes = analysis::classify_symbols(&base, &target);
        let base_graph = graph::build(&base);
        let target_graph = graph::build(&target);
        Self {
            capture,
            base,
            target,
            changes,
            base_graph,
            target_graph,
        }
    }
    pub fn complete(&self) -> bool {
        self.changes.complete && self.capture.diagnostics.is_empty()
    }
    pub fn report(&self, mode: &CompareMode, depth: usize) -> Value {
        let comparison = match mode {
            CompareMode::Local(args) => {
                json!({"mode":"local", "selection": if args.staged { "staged" } else if args.unstaged { "unstaged" } else { "combined" }, "depth":depth})
            }
            CompareMode::Branch(args) => {
                json!({"mode":"branch", "base":args.base, "target":args.target, "depth":depth})
            }
        };
        let file_changes: Vec<_> = self.capture.changes.iter().map(|change| json!({"path":path(&change.path), "kind":match change.kind { ChangeKind::Added => "added", ChangeKind::Modified => "modified", ChangeKind::Deleted => "deleted" }})).collect();
        let module_contexts: Vec<_> = [&self.base_graph, &self.target_graph]
            .into_iter()
            .flat_map(|g| {
                g.graph
                    .node_weights()
                    .filter_map(|n| {
                        n.module_path
                            .as_ref()
                            .map(|p| json!({"id":n.id,"snapshot":role(g.role),"path":path(p)}))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let symbols: Vec<_> = [&self.base_graph, &self.target_graph].into_iter().flat_map(|g| g.graph.node_weights().filter_map(|n| n.symbol.as_ref().map(|s| json!({"id":n.id,"language":snake(&format!("{:?}",s.language)),"name":s.name,"qualified_name":s.qualified_name,"kind":snake(&format!("{:?}",s.kind)),"snapshot":role(s.source.snapshot),"path":path(&s.source.path),"range":range(s.source.range),"fingerprint":s.fingerprint}))).collect::<Vec<_>>()).collect();
        let mut symbol_changes = Vec::new();
        let mut impacts = Vec::new();
        for change in &self.changes.changes {
            let before = change.before.as_ref().and_then(|s| {
                self.base_graph
                    .symbol_ids
                    .get(&(s.unit.clone(), s.scope_id))
            });
            let after = change.after.as_ref().and_then(|s| {
                self.target_graph
                    .symbol_ids
                    .get(&(s.unit.clone(), s.scope_id))
            });
            let kind = match change.kind {
                SymbolChangeKind::Added => "added",
                SymbolChangeKind::Modified => "modified",
                SymbolChangeKind::Deleted => "deleted",
            };
            symbol_changes.push(json!({"kind":kind,"before":before,"after":after}));
            let (graph, root) = if change.kind == SymbolChangeKind::Deleted {
                (&self.base_graph, before)
            } else {
                (&self.target_graph, after)
            };
            if let Some(root) = root {
                let impact = graph::callers(graph, root, depth);
                impacts.push(json!({"root":impact.root,"snapshot":role(graph.role),"nodes":impact.nodes.iter().map(|(id,d)|json!({"id":id,"depth":d})).collect::<Vec<_>>(),"edges":impact.edges.iter().map(|(from,to)|json!({"caller":from,"callee":to})).collect::<Vec<_>>()}));
            }
        }
        let call_sites: Vec<_> = [&self.base_graph,&self.target_graph].into_iter().flat_map(|g| g.calls.iter().map(|c|json!({"id":c.id,"caller":c.caller,"target":c.target,"unresolved_reason":c.reason.map(|r|snake(&format!("{:?}",r))),"expression":c.expression,"snapshot":role(c.source.snapshot),"path":path(&c.source.path),"range":range(c.source.range)})).collect::<Vec<_>>()).collect();
        let mut edges: Vec<_> = [&self.base_graph,&self.target_graph].into_iter().flat_map(|g|g.graph.edge_references().map(|e|json!({"caller":g.graph[e.source()].id,"callee":g.graph[e.target()].id,"snapshot":role(g.role),"call_sites":e.weight()})).collect::<Vec<_>>()).collect();
        edges.sort_by_key(|edge| serde_json::to_string(edge).unwrap_or_default());
        let mut diagnostics: Vec<Value> = self.capture.diagnostics.iter().map(|d|json!({"category":"git","message":d.message,"path":path(&d.path),"snapshot":null,"range":null,"affects_completeness":true})).collect();
        diagnostics.extend(self.base.diagnostics.iter().chain(&self.target.diagnostics).map(|d| {
            let rust_layout = matches!(d.kind, analysis::DiagnosticKind::UnsupportedLayout | analysis::DiagnosticKind::ModuleAmbiguity)
                && (d.path.ends_with(b".rs") || d.path.ends_with(b"Cargo.toml"));
            json!({"category":snake(&format!("{:?}",d.kind)),"message":d.message,"path":path(&d.path),"snapshot":role(d.snapshot),"range":d.range.map(range),"affects_completeness":matches!(d.kind,analysis::DiagnosticKind::ParseError|analysis::DiagnosticKind::InvalidEncoding) || rust_layout})
        }));
        diagnostics.extend(self.changes.diagnostics.iter().map(|d| json!({"category":snake(&format!("{:?}",d.kind)),"message":d.message,"path":path(&d.path),"snapshot":role(d.snapshot),"range":d.range.map(range),"affects_completeness":false})));
        diagnostics.sort_by_key(|d| serde_json::to_string(d).unwrap_or_default());
        json!({"schema_version":1,"tool_version":env!("CARGO_PKG_VERSION"),"comparison":comparison,"snapshots":{"base":snapshot(&self.capture.pair.base),"target":snapshot(&self.capture.pair.target)},"file_changes":file_changes,"symbols":symbols,"module_contexts":module_contexts,"symbol_changes":symbol_changes,"call_sites":call_sites,"edges":edges,"impacts":impacts,"diagnostics":diagnostics,"complete":self.complete()})
    }
}
fn snake(input: &str) -> String {
    let mut result = String::new();
    for (i, ch) in input.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.extend(ch.to_lowercase());
        } else {
            result.push(ch);
        }
    }
    result
}
fn path(bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(text) => json!({"text":text}),
        Err(_) => json!({"hex":bytes.iter().map(|b|format!("{b:02x}")).collect::<String>()}),
    }
}
fn role(role: SnapshotRole) -> &'static str {
    match role {
        SnapshotRole::Base => "base",
        SnapshotRole::Target => "target",
    }
}
fn range(r: SourceRange) -> Value {
    json!({"start":{"line":r.start.line,"byte_column":r.start.byte_column,"byte_offset":r.start_byte},"end":{"line":r.end.line,"byte_column":r.end.byte_column,"byte_offset":r.end_byte}})
}
fn snapshot(s: &CapturedSnapshot) -> Value {
    json!({"kind":match s.kind { SnapshotKind::Commit=>"commit",SnapshotKind::Index=>"index",SnapshotKind::WorkingTree=>"working_tree",SnapshotKind::Empty=>"empty"},"role":role(s.role),"commit_id":s.commit_id.map(|o|o.to_string()),"branch":s.branch,"content_id":s.content_id})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{AnalyzeOptions, LocalArgs};
    use crate::git::{CapturedFile, FileChange, SnapshotPair};
    use std::collections::BTreeMap;
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
            .collect::<BTreeMap<_, _>>();
        CapturedSnapshot {
            kind: SnapshotKind::WorkingTree,
            role,
            commit_id: None,
            branch: None,
            files,
            content_id: format!("{role:?}"),
        }
    }
    fn session(base: &[(&[u8], &[u8])], target: &[(&[u8], &[u8])]) -> AnalysisSession {
        AnalysisSession::new(CaptureResult {
            pair: SnapshotPair {
                base: snapshot(SnapshotRole::Base, base),
                target: snapshot(SnapshotRole::Target, target),
            },
            changes: Vec::<FileChange>::new(),
            diagnostics: vec![],
        })
    }
    fn mode() -> CompareMode {
        CompareMode::Local(LocalArgs {
            staged: false,
            unstaged: false,
            options: AnalyzeOptions {
                format: None,
                depth: 2,
            },
        })
    }
    #[test]
    fn modified_root_finds_callers_in_unchanged_files() {
        let a = b"def process(): return 1\n";
        let b = b"def process(): return 2\n";
        let entry = b"from changed import process\ndef entry(): process()\n";
        let main = b"from entry import entry\ndef main(): entry()\n";
        let before = [
            (b"changed.py".as_slice(), a.as_slice()),
            (b"entry.py".as_slice(), entry.as_slice()),
            (b"main.py".as_slice(), main.as_slice()),
        ];
        let after = [
            (b"changed.py".as_slice(), b.as_slice()),
            (b"entry.py".as_slice(), entry.as_slice()),
            (b"main.py".as_slice(), main.as_slice()),
        ];
        let session = session(&before, &after);
        let report = session.report(&mode(), 2);
        assert!(session.complete());
        let nodes = report["impacts"][0]["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3);
        let resolved = report["call_sites"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["target"].is_string())
            .count();
        assert_eq!(resolved, 4);
        let changed_target = report["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["snapshot"] == "target" && s["path"]["text"] == "changed.py")
            .unwrap()["id"]
            .as_str()
            .unwrap();
        let entry_call = report["call_sites"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["snapshot"] == "target" && c["path"]["text"] == "entry.py")
            .unwrap();
        assert_eq!(entry_call["target"], changed_target);
        assert_eq!(entry_call["range"]["start"]["line"], 2);
        assert_eq!(report["impacts"][0]["edges"].as_array().unwrap().len(), 2);
    }
    #[test]
    fn deleted_root_uses_historical_graph() {
        let base = [(
            b"a.py".as_slice(),
            b"def gone(): pass\ndef legacy(): gone()\n".as_slice(),
        )];
        let target = [(b"a.py".as_slice(), b"def legacy(): pass\n".as_slice())];
        let session = session(&base, &target);
        let report = session.report(&mode(), 1);
        let deleted = report["symbol_changes"]
            .as_array()
            .unwrap()
            .iter()
            .position(|c| c["kind"] == "deleted")
            .unwrap();
        assert_eq!(report["impacts"][deleted]["snapshot"], "base");
        assert_eq!(
            report["impacts"][deleted]["nodes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
    #[test]
    fn relative_alias_resolves_but_shadowed_call_does_not() {
        let target = [(b"pkg/__init__.py".as_slice(),b"".as_slice()),(b"pkg/tools.py".as_slice(),b"def notify(): pass\n".as_slice()),(b"pkg/entry.py".as_slice(),b"from .tools import notify as send\ndef entry(): send()\ndef other(send): send()\n".as_slice())];
        let session = session(&[], &target);
        let report = session.report(&mode(), 1);
        let calls = report["call_sites"].as_array().unwrap();
        assert!(
            calls
                .iter()
                .any(|c| c["expression"] == "send" && c["target"].is_string())
        );
        assert!(calls.iter().any(|c| c["unresolved_reason"] == "shadowed"));
    }
    #[test]
    fn partial_parse_keeps_json_and_marks_incomplete() {
        let session = session(&[], &[(b"bad.py", b"def f(:\n".as_slice())]);
        let report = session.report(&mode(), 1);
        assert_eq!(report["complete"], false);
        assert!(
            report["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["affects_completeness"] == true)
        );
    }
    #[test]
    fn qualified_module_alias_and_cycles_are_resolved() {
        let target = [
            (b"pkg/__init__.py".as_slice(), b"".as_slice()),
            (b"pkg/tools.py".as_slice(), b"def run(): pass\n".as_slice()),
            (
                b"entry.py".as_slice(),
                b"import pkg.tools as helper\ndef a(): b()\ndef b(): a(); helper.run()\n"
                    .as_slice(),
            ),
        ];
        let session = session(&[], &target);
        let report = session.report(&mode(), 3);
        let calls = report["call_sites"].as_array().unwrap();
        assert!(
            calls
                .iter()
                .any(|c| c["expression"] == "helper.run" && c["target"].is_string())
        );
        let graph = &session.target_graph;
        let b = graph
            .symbol_ids
            .iter()
            .find(|(_, id)| {
                graph.graph[graph.ids[*id]]
                    .symbol
                    .as_ref()
                    .is_some_and(|s| s.qualified_name == "b")
            })
            .unwrap()
            .1;
        let impact = crate::graph::callers(graph, b, 3);
        assert_eq!(impact.nodes.len(), 2);
        assert_eq!(impact.edges.len(), 2);
    }
    #[test]
    fn imported_function_replaced_in_its_module_has_no_edge() {
        let target = [
            (
                b"a.py".as_slice(),
                b"def f(): pass\nf = replacement\n".as_slice(),
            ),
            (
                b"entry.py".as_slice(),
                b"from a import f\ndef entry(): f()\n".as_slice(),
            ),
        ];
        let report = session(&[], &target).report(&mode(), 1);
        let call = report["call_sites"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["expression"] == "f")
            .unwrap();
        assert!(call["target"].is_null());
        assert_eq!(call["unresolved_reason"], "ambiguous_binding");
        assert!(report["edges"].as_array().unwrap().is_empty());
    }
    #[test]
    fn wildcard_in_exporting_module_prevents_certain_import_edge() {
        let target = [
            (
                b"a.py".as_slice(),
                b"def f(): pass\nfrom foreign import *\n".as_slice(),
            ),
            (
                b"entry.py".as_slice(),
                b"from a import f\ndef entry(): f()\n".as_slice(),
            ),
        ];
        let report = session(&[], &target).report(&mode(), 1);
        let call = report["call_sites"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["expression"] == "f")
            .unwrap();
        assert!(call["target"].is_null());
        assert_eq!(call["unresolved_reason"], "wildcard_import");
        assert!(report["edges"].as_array().unwrap().is_empty());
    }
    #[test]
    fn qualified_call_requires_the_imported_module_path() {
        let target = [
            (b"pkg/__init__.py".as_slice(), b"".as_slice()),
            (b"pkg/a.py".as_slice(), b"def run(): pass\n".as_slice()),
            (b"pkg/b.py".as_slice(), b"def run(): pass\n".as_slice()),
            (
                b"entry.py".as_slice(),
                b"import pkg.a\ndef entry(): pkg.a.run(); pkg.b.run()\n".as_slice(),
            ),
        ];
        let report = session(&[], &target).report(&mode(), 1);
        let calls = report["call_sites"].as_array().unwrap();
        assert!(
            calls
                .iter()
                .any(|c| c["expression"] == "pkg.a.run" && c["target"].is_string())
        );
        assert!(
            calls
                .iter()
                .any(|c| c["expression"] == "pkg.b.run" && c["target"].is_null())
        );
        assert_eq!(report["edges"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn distinct_imports_with_one_package_binding_resolve_separately() {
        let target = [
            (b"pkg/__init__.py".as_slice(), b"".as_slice()),
            (b"pkg/a.py".as_slice(), b"def run(): pass\n".as_slice()),
            (b"pkg/b.py".as_slice(), b"def run(): pass\n".as_slice()),
            (
                b"entry.py".as_slice(),
                b"import pkg.a\nimport pkg.b\ndef entry(): pkg.a.run(); pkg.b.run()\n".as_slice(),
            ),
        ];
        let report = session(&[], &target).report(&mode(), 1);
        let calls = report["call_sites"].as_array().unwrap();
        for expression in ["pkg.a.run", "pkg.b.run"] {
            assert!(
                calls
                    .iter()
                    .any(|c| c["expression"] == expression && c["target"].is_string())
            );
        }
        assert_eq!(report["edges"].as_array().unwrap().len(), 2);
    }
    #[test]
    fn from_import_respects_package_attribute_before_submodule() {
        let entry = b"from pkg import tools\ndef entry(): tools.run()\n";
        let tools = b"def run(): pass\n";
        for (init, resolved) in [
            (b"".as_slice(), true),
            (b"tools = object()\n".as_slice(), false),
            (
                b"def __getattr__(name): return object()\n".as_slice(),
                false,
            ),
        ] {
            let target = [
                (b"pkg/__init__.py".as_slice(), init),
                (b"pkg/tools.py".as_slice(), tools.as_slice()),
                (b"entry.py".as_slice(), entry.as_slice()),
            ];
            let report = session(&[], &target).report(&mode(), 1);
            let call = report["call_sites"]
                .as_array()
                .unwrap()
                .iter()
                .find(|call| call["expression"] == "tools.run")
                .unwrap();
            assert_eq!(call["target"].is_string(), resolved);
            assert_eq!(
                report["edges"].as_array().unwrap().len(),
                usize::from(resolved)
            );
        }
    }
    #[test]
    fn uncertain_calls_never_create_edges() {
        let source = b"def target(): pass\n@decorate\ndef wrapped(): pass\ndef f(target, obj):\n    target()\n    obj.run()\n    wrapped()\n    getattr(obj, 'x')()\n    missing_name()\n";
        let session = session(&[], &[(b"a.py", source)]);
        let report = session.report(&mode(), 2);
        let calls = report["call_sites"].as_array().unwrap();
        assert!(
            calls
                .iter()
                .any(|c| c["expression"] == "target" && c["unresolved_reason"] == "shadowed")
        );
        assert!(
            calls
                .iter()
                .any(|c| c["expression"] == "obj.run" && c["target"].is_null())
        );
        assert!(
            calls
                .iter()
                .any(|c| c["expression"] == "wrapped"
                    && c["unresolved_reason"] == "decorated_target")
        );
        assert!(calls.iter().any(|c| c["expression"] == "getattr(obj, 'x')"
            && c["unresolved_reason"] == "dynamic_target"));
        assert!(
            calls.iter().any(|c| c["expression"] == "missing_name"
                && c["unresolved_reason"] == "external_target")
        );
        assert!(report["edges"].as_array().unwrap().is_empty());
    }
    #[test]
    fn report_references_and_bytes_are_deterministic() {
        let target = [(
            b"a.py".as_slice(),
            b"def f(): pass\ndef g(): f(); f()\ng()\n".as_slice(),
        )];
        let a = session(&[], &target);
        let b = session(&[], &target);
        let first = serde_json::to_vec(&a.report(&mode(), 1)).unwrap();
        assert_eq!(first, serde_json::to_vec(&b.report(&mode(), 1)).unwrap());
        let report: Value = serde_json::from_slice(&first).unwrap();
        let ids: std::collections::BTreeSet<_> = report["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .chain(report["module_contexts"].as_array().unwrap())
            .map(|n| n["id"].as_str().unwrap())
            .collect();
        let sites: std::collections::BTreeSet<_> = report["call_sites"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        for edge in report["edges"].as_array().unwrap() {
            assert!(ids.contains(edge["caller"].as_str().unwrap()));
            assert!(ids.contains(edge["callee"].as_str().unwrap()));
            for site in edge["call_sites"].as_array().unwrap() {
                assert!(sites.contains(site.as_str().unwrap()));
            }
        }
        assert_eq!(report["module_contexts"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn non_utf8_path_has_reversible_hex_representation() {
        let session = session(&[], &[(b"name-\xff.py", b"def f(): pass\n")]);
        let report = session.report(&mode(), 0);
        assert_eq!(report["symbols"][0]["path"]["hex"], "6e616d652dff2e7079");
    }

    #[test]
    fn depth_and_multiple_call_sites_keep_evidence() {
        let target = [(
            b"a.py".as_slice(),
            b"def f(): pass\ndef g(): f(); f()\ndef h(): g()\n".as_slice(),
        )];
        let session = session(&[], &target);
        let graph = &session.target_graph;
        let f = graph
            .graph
            .node_weights()
            .find(|n| n.symbol.as_ref().is_some_and(|s| s.name == "f"))
            .unwrap()
            .id
            .clone();
        let zero = crate::graph::callers(graph, &f, 0);
        let one = crate::graph::callers(graph, &f, 1);
        let two = crate::graph::callers(graph, &f, 2);
        assert_eq!(
            (zero.nodes.len(), one.nodes.len(), two.nodes.len()),
            (1, 2, 3)
        );
        let edge = graph
            .graph
            .edge_weights()
            .find(|sites| sites.len() == 2)
            .unwrap();
        assert_eq!(edge.len(), 2);
        let report = session.report(&mode(), 2);
        assert!(
            report["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["call_sites"].as_array().unwrap().len() == 2)
        );
    }
    #[test]
    fn wildcard_import_never_confirms_same_named_function() {
        let target = [(
            b"a.py".as_slice(),
            b"def f(): pass\nfrom foreign import *\ndef entry(): f()\n".as_slice(),
        )];
        let session = session(&[], &target);
        let report = session.report(&mode(), 1);
        assert!(
            report["call_sites"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["expression"] == "f" && c["unresolved_reason"] == "wildcard_import")
        );
        assert!(report["edges"].as_array().unwrap().is_empty());
    }
}
