use crate::analysis::{
    ExtractedSnapshot, LanguageId, ParsedFile, ScopeKind, SourceRef, SourceUnitKey, Symbol,
};
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
pub type SymbolKey = (SourceUnitKey, LanguageId, String, String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum EndpointIdentity {
    Symbol(SymbolKey),
    Ambiguous(SymbolKey, SnapshotRole, String),
    Module(SourceUnitKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EdgeChangeKind {
    Added,
    Deleted,
}

#[derive(Debug, Clone)]
pub struct EdgeChange {
    pub kind: EdgeChangeKind,
    pub caller_view: String,
    pub callee_view: String,
    pub before: Option<(String, String)>,
    pub after: Option<(String, String)>,
}

#[derive(Debug)]
pub struct EdgeComparison {
    pub native_to_view: BTreeMap<String, String>,
    pub changes: Vec<EdgeChange>,
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

pub fn compare_edges(
    base: &SnapshotGraph,
    target: &SnapshotGraph,
    base_source: &ExtractedSnapshot,
    target_source: &ExtractedSnapshot,
) -> EdgeComparison {
    let mut groups: BTreeMap<SymbolKey, (Vec<String>, Vec<String>)> = BTreeMap::new();
    let mut keys = BTreeMap::new();
    for graph in [base, target] {
        for node in graph.graph.node_weights() {
            if let Some(symbol) = &node.symbol {
                let key = (
                    symbol.unit.clone(),
                    symbol.language,
                    symbol.qualified_name.clone(),
                    symbol.name.clone(),
                );
                let group = groups.entry(key).or_default();
                match graph.role {
                    SnapshotRole::Base => group.0.push(node.id.clone()),
                    SnapshotRole::Target => group.1.push(node.id.clone()),
                }
            }
        }
        for (unit, id) in &graph.module_ids {
            keys.insert(id.clone(), EndpointIdentity::Module(unit.clone()));
        }
    }
    for (key, (before, after)) in groups {
        let ambiguous = before.len() > 1 || after.len() > 1;
        for (role, ids) in [(SnapshotRole::Base, before), (SnapshotRole::Target, after)] {
            for id in ids {
                keys.insert(
                    id.clone(),
                    if ambiguous {
                        EndpointIdentity::Ambiguous(key.clone(), role, id)
                    } else {
                        EndpointIdentity::Symbol(key.clone())
                    },
                );
            }
        }
    }
    let identities: BTreeSet<_> = keys.values().collect();
    let view_ids: BTreeMap<_, _> = identities
        .into_iter()
        .enumerate()
        .map(|(index, key)| (key, format!("v-{index:06}")))
        .collect();
    let native_to_view: BTreeMap<_, _> = keys
        .iter()
        .map(|(id, key)| (id.clone(), view_ids[key].clone()))
        .collect();
    let invalid: BTreeSet<_> = base_source
        .files
        .iter()
        .chain(&target_source.files)
        .filter(|(_, file)| !file.valid)
        .map(|(unit, _)| unit.clone())
        .collect();
    let pairs = |graph: &SnapshotGraph| {
        let mut pairs = BTreeMap::new();
        for edge in graph.graph.edge_references() {
            let caller = &graph.graph[edge.source()];
            let callee = &graph.graph[edge.target()];
            let eligible = |node: &GraphNode| match &keys[&node.id] {
                EndpointIdentity::Symbol((unit, ..)) | EndpointIdentity::Module(unit) => {
                    !invalid.contains(unit)
                }
                EndpointIdentity::Ambiguous(..) => false,
            };
            if eligible(caller) && eligible(callee) {
                pairs.insert(
                    (
                        native_to_view[&caller.id].clone(),
                        native_to_view[&callee.id].clone(),
                    ),
                    (caller.id.clone(), callee.id.clone()),
                );
            }
        }
        pairs
    };
    let before = pairs(base);
    let after = pairs(target);
    let mut changes = Vec::new();
    for ((caller_view, callee_view), pair) in &before {
        if !after.contains_key(&(caller_view.clone(), callee_view.clone())) {
            changes.push(EdgeChange {
                kind: EdgeChangeKind::Deleted,
                caller_view: caller_view.clone(),
                callee_view: callee_view.clone(),
                before: Some(pair.clone()),
                after: None,
            });
        }
    }
    for ((caller_view, callee_view), pair) in &after {
        if !before.contains_key(&(caller_view.clone(), callee_view.clone())) {
            changes.push(EdgeChange {
                kind: EdgeChangeKind::Added,
                caller_view: caller_view.clone(),
                callee_view: callee_view.clone(),
                before: None,
                after: Some(pair.clone()),
            });
        }
    }
    changes.sort_by(|a, b| {
        (&a.caller_view, &a.callee_view, a.kind).cmp(&(&b.caller_view, &b.callee_view, b.kind))
    });
    EdgeComparison {
        native_to_view,
        changes,
    }
}

pub fn call_retargets_covered(
    base: &SnapshotGraph,
    target: &SnapshotGraph,
    before_id: &str,
    after_id: &str,
    comparison: &EdgeComparison,
) -> bool {
    let before = base.calls.iter().filter(|call| call.caller == before_id);
    let after = target.calls.iter().filter(|call| call.caller == after_id);
    if before.clone().count() != after.clone().count() {
        return false;
    }
    let mut differing = false;
    let caller_view = &comparison.native_to_view[before_id];
    for (old, new) in before.zip(after) {
        if old.expression == new.expression {
            continue;
        }
        differing = true;
        let (Some(old_target), Some(new_target)) = (&old.target, &new.target) else {
            return false;
        };
        let old_view = &comparison.native_to_view[old_target];
        let new_view = &comparison.native_to_view[new_target];
        if old_view == new_view
            || !comparison.changes.iter().any(|change| {
                change.kind == EdgeChangeKind::Deleted
                    && &change.caller_view == caller_view
                    && &change.callee_view == old_view
            })
            || !comparison.changes.iter().any(|change| {
                change.kind == EdgeChangeKind::Added
                    && &change.caller_view == caller_view
                    && &change.callee_view == new_view
            })
        {
            return false;
        }
    }
    differing
}
