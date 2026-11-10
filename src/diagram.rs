use crate::analysis::SymbolChangeKind;
use crate::git::SnapshotRole;
use crate::graph::EdgeChangeKind;
use crate::report::{AnalysisSession, FocusedNode};
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Clone, Copy)]
pub(crate) enum DiagramFormat {
    Mermaid,
    Dot,
}

pub(crate) fn render(session: &AnalysisSession, depth: usize, format: DiagramFormat) -> String {
    let focused = session.focused_graph(depth);
    let mut out = match format {
        DiagramFormat::Mermaid => "flowchart LR".to_owned(),
        DiagramFormat::Dot => "digraph dflow {\n  rankdir=LR;".to_owned(),
    };
    let ids: BTreeMap<&str, usize> = focused
        .impact
        .nodes
        .iter()
        .enumerate()
        .map(|(index, (view, _))| (view.as_str(), index))
        .collect();
    for (view, _) in &focused.impact.nodes {
        let provenance = &focused.nodes[view];
        let node = if let Some(native) = &provenance.target {
            &session.target_graph.graph[session.target_graph.ids[native]]
        } else {
            let native = provenance.base.as_ref().expect("focused node has provenance");
            &session.base_graph.graph[session.base_graph.ids[native]]
        };
        let mut label = String::new();
        if let Some(symbol) = &node.symbol {
            write!(label, "{} | {}", symbol.qualified_name, display_path(&symbol.source.path)).unwrap();
        } else if let Some(path) = &node.module_path {
            let basename = path.rsplit(|byte| *byte == b'/').next().unwrap_or(path);
            write!(label, "{} | {}", display_path(basename), display_path(path)).unwrap();
        }
        write!(label, " | {}", provenance_status(provenance)).unwrap();
        for change in &provenance.changes {
            let status = match change {
                SymbolChangeKind::Added => "added",
                SymbolChangeKind::Modified => "modified",
                SymbolChangeKind::Deleted => "deleted",
            };
            write!(label, " | {status}").unwrap();
        }
        node_line(&mut out, format, ids[view.as_str()], &label);
    }
    let mut changed_edges = focused.edge_status.iter().peekable();
    for ((caller, callee), contributors) in &focused.edges {
        let changed = if changed_edges.peek().is_some_and(|((from, to), _)| from == caller && to == callee) {
            changed_edges.next().map(|(_, kind)| kind)
        } else {
            None
        };
        let status = match changed {
            Some(EdgeChangeKind::Added) => "+ target",
            Some(EdgeChangeKind::Deleted) => "- base",
            None => {
                let base = contributors.iter().any(|(role, _, _)| *role == SnapshotRole::Base);
                let target = contributors.iter().any(|(role, _, _)| *role == SnapshotRole::Target);
                match (base, target) {
                    (true, true) => "both",
                    (true, false) => "base",
                    (false, true) => "target",
                    (false, false) => unreachable!("focused edge has contributors"),
                }
            }
        };
        match format {
            DiagramFormat::Mermaid => write!(out, "\n  n{} -->|{status}| n{}", ids[caller.as_str()], ids[callee.as_str()]).unwrap(),
            DiagramFormat::Dot => write!(out, "\n  n{} -> n{} [label=\"{status}\"];", ids[caller.as_str()], ids[callee.as_str()]).unwrap(),
        }
    }
    if ids.is_empty() {
        node_line(&mut out, format, 0, "No symbol or edge changes");
    }
    if !session.complete() {
        node_line(&mut out, format, ids.len() + usize::from(ids.is_empty()), "PARTIAL analysis");
    }
    if matches!(format, DiagramFormat::Dot) {
        out.push_str("\n}");
    }
    out
}

fn provenance_status(node: &FocusedNode) -> &'static str {
    match (node.base.is_some(), node.target.is_some()) {
        (true, true) => "both",
        (true, false) => "base",
        (false, true) => "target",
        (false, false) => unreachable!("focused node has provenance"),
    }
}

fn display_path(path: &[u8]) -> String {
    match std::str::from_utf8(path) {
        Ok(text) => text.to_owned(),
        Err(_) => {
            let mut result = String::from("hex:");
            for byte in path {
                write!(result, "{byte:02x}").unwrap();
            }
            result
        }
    }
}

fn node_line(out: &mut String, format: DiagramFormat, index: usize, label: &str) {
    match format {
        DiagramFormat::Mermaid => {
            write!(out, "\n  n{index}[\"").unwrap();
            escape_mermaid(out, label);
            out.push_str("\"]");
        }
        DiagramFormat::Dot => {
            write!(out, "\n  n{index} [label=\"").unwrap();
            escape_dot(out, label);
            out.push_str("\"];");
        }
    }
}

// Mermaid quoted-node labels accept decimal #NN; entities. Encode syntax characters,
// the entity introducer itself, and controls so user text stays on one declaration.
fn escape_mermaid(out: &mut String, label: &str) {
    for ch in label.chars() {
        if ch.is_control() || matches!(ch, '&' | '#' | '"' | '<' | '>' | '[' | ']' | '{' | '}' | '|' | '`' | '\\' | ';') {
            write!(out, "#{};", ch as u32).unwrap();
        } else {
            out.push(ch);
        }
    }
}

fn escape_dot(out: &mut String, label: &str) {
    for ch in label.chars() {
        match ch {
            '"' | '\\' => { out.push('\\'); out.push(ch); }
            '\n' => out.push_str("\\n"),
            _ if ch.is_control() => write!(out, "U+{:04X}", ch as u32).unwrap(),
            _ => out.push(ch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{CaptureResult, CapturedFile, CapturedSnapshot, SnapshotKind, SnapshotPair};

    fn session(base: &[(&[u8], &[u8])], target: &[(&[u8], &[u8])]) -> AnalysisSession {
        let snapshot = |role, inputs: &[(&[u8], &[u8])]| CapturedSnapshot {
            kind: SnapshotKind::WorkingTree,
            role,
            commit_id: None,
            branch: None,
            files: inputs.iter().map(|(path, bytes)| (path.to_vec(), CapturedFile {
                path: path.to_vec(), bytes: bytes.to_vec(), mode: 0o100644,
            })).collect(),
            content_id: String::new(),
        };
        AnalysisSession::new(CaptureResult {
            pair: SnapshotPair {
                base: snapshot(SnapshotRole::Base, base),
                target: snapshot(SnapshotRole::Target, target),
            },
            changes: vec![], diagnostics: vec![],
        }, true)
    }

    #[test]
    fn retarget_at_zero_and_caller_at_two() {
        let old = b"def B(): pass\ndef C(): pass\ndef A(): B()\ndef Top(): A()\n";
        let new = b"def B(): pass\ndef C(): pass\ndef A(): C()\ndef Top(): A()\n";
        let session = session(&[(b"a.py", old)], &[(b"a.py", new)]);
        let zero = render(&session, 0, DiagramFormat::Mermaid);
        assert!(session.changes.changes.is_empty());
        assert!(zero.contains("A #124; a.py #124; both"));
        assert_eq!(zero.matches("-->|+ target|").count(), 1);
        assert_eq!(zero.matches("-->|- base|").count(), 1);
        assert!(!zero.contains("Top #124;"));
        let two = render(&session, 2, DiagramFormat::Dot);
        assert!(two.contains("Top | a.py | both"));
        assert!(two.contains("[label=\"both\"]"));
        assert!(two.contains("[label=\"+ target\"]"));
        assert!(two.contains("[label=\"- base\"]"));
    }

    #[test]
    fn deleted_symbol_and_cycle_are_focused() {
        let old = b"def A(): B()\ndef B(): A()\n";
        let new = b"def A(): pass\n";
        let session = session(&[(b"a.py", old)], &[(b"a.py", new)]);
        let output = render(&session, 2, DiagramFormat::Dot);
        assert!(output.contains("B | a.py | base | deleted"));
        let a = output.lines().find(|line| line.contains("A | a.py")).unwrap().trim().split_whitespace().next().unwrap();
        let b = output.lines().find(|line| line.contains("B | a.py")).unwrap().trim().split_whitespace().next().unwrap();
        assert!(output.contains(&format!("{a} -> {b} ")));
        assert!(output.contains(&format!("{b} -> {a} ")));
    }

    #[test]
    fn shared_caller_is_one_node_with_two_outgoing_edges() {
        let old = b"def B(): return 1\ndef C(): return 1\ndef shared(): B(); C()\n";
        let new = b"def B(): return 2\ndef C(): return 2\ndef shared(): B(); C()\n";
        let session = session(&[(b"a.py", old)], &[(b"a.py", new)]);
        let output = render(&session, 1, DiagramFormat::Dot);
        let shared: Vec<_> = output.lines().filter(|line| line.contains("shared | a.py")).collect();
        assert_eq!(shared.len(), 1);
        let id = shared[0].trim().split_whitespace().next().unwrap();
        assert_eq!(output.matches(&format!("{id} -> ")).count(), 2);
    }

    #[test]
    fn empty_and_invalid_source() {
        let empty = session(&[], &[]);
        assert_eq!(render(&empty, 0, DiagramFormat::Mermaid), "flowchart LR\n  n0[\"No symbol or edge changes\"]");
        let invalid = session(&[], &[(b"bad.py", b"def broken(:\n")]);
        assert!(!invalid.complete());
        let output = render(&invalid, 0, DiagramFormat::Dot);
        assert!(output.contains("No symbol or edge changes"));
        assert!(output.contains("PARTIAL analysis"));
    }

    #[test]
    fn hostile_labels_and_non_utf8_paths_stay_in_one_declaration() {
        let label = "a\"[x]{y}#&|`\\;<>\n\t";
        let mut mermaid = String::new();
        node_line(&mut mermaid, DiagramFormat::Mermaid, 0, label);
        assert_eq!(mermaid.lines().count(), 2);
        for entity in ["#34;", "#91;", "#93;", "#123;", "#125;", "#35;", "#38;", "#124;", "#96;", "#92;", "#59;", "#10;", "#9;"] {
            assert!(mermaid.contains(entity), "{entity}");
        }
        let mut dot = String::new();
        node_line(&mut dot, DiagramFormat::Dot, 0, label);
        assert_eq!(dot.lines().count(), 2);
        assert!(dot.contains("\\\""));
        assert!(dot.contains("\\\\"));
        assert!(dot.contains("\\nU+0009"));
        let session = session(&[], &[(b"weird\xff.py", b"def f(): pass\n")]);
        let output = render(&session, 0, DiagramFormat::Mermaid);
        assert!(output.contains("hex:7765697264ff2e7079"));
    }
}
