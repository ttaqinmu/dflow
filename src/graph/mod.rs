use crate::analysis::{ExtractedSnapshot, ParsedFile, ScopeKind, SourceRef, SourceUnitKey, Symbol};
use crate::git::SnapshotRole;
use crate::languages::{Resolution, ResolverRegistry, UnresolvedReason};
use petgraph::Direction::Incoming;
use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone)]
pub struct GraphNode {
    pub id: String,
    pub symbol: Option<Symbol>,
    pub module_path: Option<Vec<u8>>,
}
#[derive(Debug, Clone)]
pub struct CallRecord {
    pub id: String,
    pub caller: String,
    pub target: Option<String>,
    pub reason: Option<UnresolvedReason>,
    pub expression: String,
    pub source: SourceRef,
}
#[derive(Debug)]
pub struct SnapshotGraph {
    pub role: SnapshotRole,
    pub graph: StableDiGraph<GraphNode, Vec<String>>,
    pub ids: BTreeMap<String, NodeIndex>,
    pub symbol_ids: BTreeMap<(SourceUnitKey, usize), String>,
    pub module_ids: BTreeMap<SourceUnitKey, String>,
    pub calls: Vec<CallRecord>,
}
#[derive(Debug)]
pub struct Impact {
    pub root: String,
    pub nodes: Vec<(String, usize)>,
    pub edges: Vec<(String, String)>,
}
fn role_code(role: SnapshotRole) -> &'static str {
    match role {
        SnapshotRole::Base => "b",
        SnapshotRole::Target => "t",
    }
}
fn caller_scope(file: &ParsedFile, scope_id: usize) -> usize {
    let mut current = Some(scope_id);
    while let Some(id) = current {
        if file.scopes[id].kind == ScopeKind::Function {
            return id;
        }
        current = file.scopes[id].parent;
    }
    0
}
pub fn build(snapshot: &ExtractedSnapshot) -> SnapshotGraph {
    let mut graph: StableDiGraph<GraphNode, Vec<String>> = StableDiGraph::new();
    let mut ids = BTreeMap::new();
    let mut symbol_ids = BTreeMap::new();
    let mut module_ids = BTreeMap::new();
    let mut serial = 0usize;
    for (unit, file) in &snapshot.files {
        if !file.valid {
            continue;
        }
        let module_id = format!("m-{}-{serial:06}", role_code(snapshot.role));
        serial += 1;
        ids.insert(
            module_id.clone(),
            graph.add_node(GraphNode {
                id: module_id.clone(),
                symbol: None,
                module_path: Some(unit.path.clone()),
            }),
        );
        module_ids.insert(unit.clone(), module_id);
        for symbol in &file.symbols {
            let id = format!("s-{}-{serial:06}", role_code(snapshot.role));
            serial += 1;
            ids.insert(
                id.clone(),
                graph.add_node(GraphNode {
                    id: id.clone(),
                    symbol: Some(symbol.clone()),
                    module_path: None,
                }),
            );
            symbol_ids.insert((unit.clone(), symbol.scope_id), id);
        }
    }
    let resolver = ResolverRegistry::new(snapshot);
    let mut calls = Vec::new();
    let mut call_serial = 0usize;
    for (unit, file) in &snapshot.files {
        if !file.valid {
            continue;
        }
        for call in &file.calls {
            let owner_scope = caller_scope(file, call.scope_id);
            let caller = if owner_scope == 0 {
                module_ids[unit].clone()
            } else {
                symbol_ids
                    .get(&(unit.clone(), owner_scope))
                    .cloned()
                    .unwrap_or_else(|| module_ids[unit].clone())
            };
            let (target, reason) = match resolver.resolve(file, call) {
                Resolution::Resolved(symbol) => (
                    symbol_ids
                        .get(&(symbol.unit.clone(), symbol.scope_id))
                        .cloned(),
                    None,
                ),
                Resolution::Unresolved(reason) => (None, Some(reason)),
            };
            let reason = if target.is_none() {
                reason.or(Some(UnresolvedReason::MissingTarget))
            } else {
                None
            };
            let id = format!("c-{}-{call_serial:06}", role_code(snapshot.role));
            call_serial += 1;
            if let Some(target) = &target {
                let from = ids[&caller];
                let to = ids[target];
                if let Some(edge) = graph.find_edge(from, to) {
                    graph.edge_weight_mut(edge).unwrap().push(id.clone());
                } else {
                    graph.add_edge(from, to, vec![id.clone()]);
                }
            }
            calls.push(CallRecord {
                id,
                caller,
                target,
                reason,
                expression: call.expression.clone(),
                source: call.source.clone(),
            });
        }
    }
    for edge in graph.edge_weights_mut() {
        edge.sort();
    }
    SnapshotGraph {
        role: snapshot.role,
        graph,
        ids,
        symbol_ids,
        module_ids,
        calls,
    }
}
pub fn callers(graph: &SnapshotGraph, root: &str, depth: usize) -> Impact {
    let Some(&root_idx) = graph.ids.get(root) else {
        return Impact {
            root: root.into(),
            nodes: Vec::new(),
            edges: Vec::new(),
        };
    };
    let mut distance = BTreeMap::new();
    let mut queue = VecDeque::new();
    distance.insert(root_idx, 0usize);
    queue.push_back(root_idx);
    while let Some(node) = queue.pop_front() {
        let d = distance[&node];
        if d >= depth {
            continue;
        }
        for caller in graph.graph.neighbors_directed(node, Incoming) {
            if let std::collections::btree_map::Entry::Vacant(entry) = distance.entry(caller) {
                entry.insert(d + 1);
                queue.push_back(caller);
            }
        }
    }
    let mut nodes: Vec<_> = distance
        .iter()
        .map(|(idx, d)| (graph.graph[*idx].id.clone(), *d))
        .collect();
    nodes.sort();
    let known: BTreeSet<_> = distance.keys().copied().collect();
    let mut edges: Vec<_> = graph
        .graph
        .edge_references()
        .filter(|edge| known.contains(&edge.source()) && known.contains(&edge.target()))
        .map(|edge| {
            (
                graph.graph[edge.source()].id.clone(),
                graph.graph[edge.target()].id.clone(),
            )
        })
        .collect();
    edges.sort();
    Impact {
        root: root.into(),
        nodes,
        edges,
    }
}
