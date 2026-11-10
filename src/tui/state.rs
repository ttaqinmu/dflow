use super::layout;
use crate::analysis::SymbolChangeKind;
use crate::cli::CompareMode;
use crate::git::SnapshotRole;
use crate::graph::{self, EdgeChangeKind, EndpointIdentity, GraphNode, Impact, SnapshotGraph};
use crate::report::AnalysisSession;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use std::collections::BTreeMap;

type EdgeContributor = (SnapshotRole, String, String);

#[derive(Default)]
pub struct NodeProvenance {
    pub base: Option<String>,
    pub target: Option<String>,
    pub changes: Vec<SymbolChangeKind>,
}

pub struct App<'a> {
    pub session: AnalysisSession,
    pub mode: &'a CompareMode,
    pub nodes: BTreeMap<String, NodeProvenance>,
    pub edges: BTreeMap<(String, String), Vec<EdgeContributor>>,
    pub edge_status: BTreeMap<(String, String), EdgeChangeKind>,
    pub changed_roots: Vec<String>,
    pub selected_node: Option<String>,
    pub impact: Impact,
    pub layout: layout::Layout,
    pub depth: usize,
    pub expanded: bool,
    pub pan_x: i32,
    pub pan_y: i32,
    pub help: bool,
    pub reload_error: Option<String>,
}
impl<'a> App<'a> {
    pub fn new(session: AnalysisSession, mode: &'a CompareMode, depth: usize) -> Self {
        let mut app = Self {
            session,
            mode,
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            edge_status: BTreeMap::new(),
            changed_roots: Vec::new(),
            selected_node: None,
            impact: Impact {
                root: String::new(),
                nodes: Vec::new(),
                edges: Vec::new(),
            },
            layout: layout::Layout::default(),
            depth,
            expanded: false,
            pan_x: 0,
            pan_y: 0,
            help: false,
            reload_error: None,
        };
        app.refresh();
        app
    }
    fn graph(&self, role: SnapshotRole) -> &SnapshotGraph {
        match role {
            SnapshotRole::Base => &self.session.base_graph,
            SnapshotRole::Target => &self.session.target_graph,
        }
    }
    fn native(&self, id: &str) -> (&SnapshotGraph, &GraphNode) {
        let provenance = &self.nodes[id];
        let (role, native) = if let Some(native) = &provenance.target {
            (SnapshotRole::Target, native)
        } else {
            (SnapshotRole::Base, provenance.base.as_ref().unwrap())
        };
        let graph = self.graph(role);
        (graph, &graph.graph[graph.ids[native]])
    }
    fn endpoint_key(&self, id: &str) -> Option<EndpointIdentity> {
        let (graph, node) = self.native(id);
        if let Some(symbol) = &node.symbol {
            let key = (
                symbol.unit.clone(),
                symbol.language,
                symbol.qualified_name.clone(),
                symbol.name.clone(),
            );
            let count = graph
                .graph
                .node_weights()
                .filter(|node| {
                    node.symbol.as_ref().is_some_and(|s| {
                        (
                            s.unit.clone(),
                            s.language,
                            s.qualified_name.clone(),
                            s.name.clone(),
                        ) == key
                    })
                })
                .take(2)
                .count();
            (count == 1).then_some(EndpointIdentity::Symbol(key))
        } else {
            graph
                .module_ids
                .iter()
                .find(|(_, native)| native.as_str() == node.id)
                .map(|(unit, _)| EndpointIdentity::Module(unit.clone()))
        }
    }
    pub fn replace_session(&mut self, next: AnalysisSession) {
        let selected = self
            .selected_node
            .as_ref()
            .and_then(|id| self.endpoint_key(id));
        let pan = (self.pan_x, self.pan_y);
        self.session = next;
        self.reload_error = None;
        self.selected_node = None;
        self.refresh();
        let mut matches = self
            .layout
            .order
            .iter()
            .filter(|id| self.endpoint_key(id).as_ref() == selected.as_ref());
        let match_id = if selected.is_some() {
            matches.next().cloned().filter(|_| matches.next().is_none())
        } else {
            None
        };
        if let Some(id) = match_id {
            if self.selected_node.as_ref() != Some(&id) {
                self.selected_node = Some(id);
                self.relayout();
            }
            (self.pan_x, self.pan_y) = pan;
        } else {
            self.expanded = false;
            self.relayout();
        }
    }
    fn refresh(&mut self) {
        let native_to_view = &self.session.edge_comparison.native_to_view;
        self.nodes.clear();
        for graph in [&self.session.base_graph, &self.session.target_graph] {
            for id in graph.ids.keys() {
                let entry = self.nodes.entry(native_to_view[id].clone()).or_default();
                match graph.role {
                    SnapshotRole::Base => entry.base = Some(id.clone()),
                    SnapshotRole::Target => entry.target = Some(id.clone()),
                }
            }
        }
        let mut distances: BTreeMap<String, usize> = BTreeMap::new();
        self.changed_roots.clear();
        self.edges.clear();
        self.edge_status.clear();
        for change in &self.session.changes.changes {
            let (role, symbol) = if change.kind == SymbolChangeKind::Deleted {
                (SnapshotRole::Base, change.before.as_ref())
            } else {
                (SnapshotRole::Target, change.after.as_ref())
            };
            let Some(symbol) = symbol else { continue };
            let graph = match role {
                SnapshotRole::Base => &self.session.base_graph,
                SnapshotRole::Target => &self.session.target_graph,
            };
            let Some(root) = graph
                .symbol_ids
                .get(&(symbol.unit.clone(), symbol.scope_id))
            else {
                continue;
            };
            let view_root = native_to_view[root].clone();
            self.nodes
                .get_mut(&view_root)
                .unwrap()
                .changes
                .push(change.kind);
            self.changed_roots.push(view_root);
            let impact = graph::callers(graph, root, self.depth);
            for (native, distance) in impact.nodes {
                distances
                    .entry(native_to_view[&native].clone())
                    .and_modify(|old| *old = (*old).min(distance))
                    .or_insert(distance);
            }
            for (caller, callee) in impact.edges {
                self.edges
                    .entry((
                        native_to_view[&caller].clone(),
                        native_to_view[&callee].clone(),
                    ))
                    .or_default()
                    .push((role, caller, callee));
            }
        }
        for change in &self.session.edge_comparison.changes {
            let (role, caller, callee) = if let Some((caller, callee)) = &change.before {
                (SnapshotRole::Base, caller, callee)
            } else if let Some((caller, callee)) = &change.after {
                (SnapshotRole::Target, caller, callee)
            } else {
                continue;
            };
            let graph = self.graph(role);
            let impact = graph::callers(graph, callee, self.depth);
            for (native, distance) in impact.nodes {
                distances
                    .entry(native_to_view[&native].clone())
                    .and_modify(|old| *old = (*old).min(distance))
                    .or_insert(distance);
            }
            for (from, to) in impact.edges {
                self.edges
                    .entry((native_to_view[&from].clone(), native_to_view[&to].clone()))
                    .or_default()
                    .push((role, from, to));
            }
            distances.entry(change.caller_view.clone()).or_insert(1);
            distances.entry(change.callee_view.clone()).or_insert(0);
            self.edges
                .entry((change.caller_view.clone(), change.callee_view.clone()))
                .or_default()
                .push((role, caller.clone(), callee.clone()));
            self.edge_status.insert(
                (change.caller_view.clone(), change.callee_view.clone()),
                change.kind,
            );
        }
        self.changed_roots.sort();
        self.changed_roots.dedup();
        for node in self.nodes.values_mut() {
            node.changes.sort_by_key(|kind| kind_name_order(*kind));
            node.changes.dedup();
        }
        for contributors in self.edges.values_mut() {
            contributors.sort();
            contributors.dedup();
        }
        self.impact = Impact {
            root: self.changed_roots.first().cloned().unwrap_or_default(),
            nodes: distances.into_iter().collect(),
            edges: self.edges.keys().cloned().collect(),
        };
        self.layout = layout::build(&self.impact);
        for edge in &mut self.layout.edges {
            edge.status = self
                .session
                .edge_comparison
                .changes
                .iter()
                .find(|change| {
                    change.caller_view == edge.caller && change.callee_view == edge.callee
                })
                .map(|change| change.kind);
        }
        if !self
            .selected_node
            .as_ref()
            .is_some_and(|id| self.layout.order.contains(id))
        {
            self.selected_node = self.changed_roots.first().cloned().or_else(|| {
                self.session
                    .edge_comparison
                    .changes
                    .first()
                    .and_then(|change| {
                        [change.callee_view.clone(), change.caller_view.clone()]
                            .into_iter()
                            .find(|id| self.layout.order.contains(id))
                    })
            });
        }
        self.relayout();
    }
    fn relayout(&mut self) {
        let mut view = std::mem::take(&mut self.layout);
        for component in &mut view.components {
            if component.title.is_empty() {
                let (_, first) = self.native(&component.members[0]);
                component.title = node_text(
                    &component
                        .members
                        .iter()
                        .map(|id| self.node_box_title(id))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                let path = first
                    .symbol
                    .as_ref()
                    .map(|symbol| symbol.source.path.as_slice())
                    .or(first.module_path.as_deref())
                    .unwrap_or_default();
                component.description = node_text(&display_path(path));
            }
            component.edge_labels = self
                .edge_status
                .iter()
                .filter(|((caller, callee), _)| {
                    component.members.contains(caller) && component.members.contains(callee)
                })
                .map(|((caller, callee), status)| {
                    node_text(&format!(
                        "{} {} -> {} [{}]",
                        if *status == EdgeChangeKind::Added {
                            "+"
                        } else {
                            "-"
                        },
                        self.node_title(caller),
                        self.node_title(callee),
                        if *status == EdgeChangeKind::Added {
                            "target"
                        } else {
                            "base"
                        }
                    ))
                })
                .collect();
            if self.expanded
                && self
                    .selected_node
                    .as_ref()
                    .is_some_and(|id| component.members.contains(id))
            {
                component.extra = self
                    .node_details(self.selected_node.as_ref().unwrap(), component)
                    .into_iter()
                    .map(|line| node_text(&line))
                    .collect();
            } else {
                component.extra.clear();
            }
        }
        layout::place(&mut view);
        self.layout = view;
        self.center_selected();
    }
    fn node_details(&self, id: &str, component: &layout::Component) -> Vec<String> {
        let provenance = &self.nodes[id];
        let mut lines = Vec::new();
        for change in &provenance.changes {
            lines.push(format!("Change: {}", kind(*change)));
        }
        for change in &self.session.edge_comparison.changes {
            if change.caller_view != id && change.callee_view != id {
                continue;
            }
            let (role, pair) = if let Some(pair) = &change.before {
                (SnapshotRole::Base, pair)
            } else if let Some(pair) = &change.after {
                (SnapshotRole::Target, pair)
            } else {
                continue;
            };
            let evidence = self
                .graph(role)
                .calls
                .iter()
                .find(|call| call.caller == pair.0 && call.target.as_deref() == Some(&pair.1));
            let where_at = evidence
                .map(|call| {
                    format!(
                        " @ {}:{}",
                        display_path(&call.source.path),
                        call.source.range.start.line
                    )
                })
                .unwrap_or_default();
            lines.push(format!(
                "Edge {} [{}]: {} -> {}{}",
                if change.kind == EdgeChangeKind::Added {
                    "added"
                } else {
                    "deleted"
                },
                role_name(role),
                self.node_title(&change.caller_view),
                self.node_title(&change.callee_view),
                where_at
            ));
        }
        for (role, native) in [
            (SnapshotRole::Base, &provenance.base),
            (SnapshotRole::Target, &provenance.target),
        ] {
            let Some(native) = native else { continue };
            let graph = self.graph(role);
            let node = &graph.graph[graph.ids[native]];
            lines.push(format!("Snapshot: {}", role_name(role)));
            if let Some(symbol) = &node.symbol {
                lines.push(format!("Kind: {:?}", symbol.kind));
                lines.push(format!("Path: {}", display_path(&symbol.source.path)));
                let range = symbol.source.range;
                lines.push(format!(
                    "Range: {}:{}-{}:{}",
                    range.start.line,
                    range.start.byte_column,
                    range.end.line,
                    range.end.byte_column
                ));
                let captured = match role {
                    SnapshotRole::Base => &self.session.capture.pair.base,
                    SnapshotRole::Target => &self.session.capture.pair.target,
                };
                if let Some(file) = captured.files.get(&symbol.source.path) {
                    lines.push(format!("Source [{}]:", role_name(role)));
                    for (line_no, content) in String::from_utf8_lossy(&file.bytes)
                        .lines()
                        .enumerate()
                        .skip(range.start.line.saturating_sub(2))
                        .take(5)
                    {
                        lines.push(format!("{:>4} {}", line_no + 1, safe_text(content)));
                    }
                }
            } else {
                lines.push(format!(
                    "Module: {}",
                    display_path(node.module_path.as_deref().unwrap_or_default())
                ));
            }
            let mut related = 0;
            for call in &graph.calls {
                if call.caller == *native || call.target.as_deref() == Some(native) {
                    if related == 0 {
                        lines.push(format!("Call evidence [{}]:", role_name(role)));
                    }
                    let direction = if call.caller == *native {
                        "calls"
                    } else {
                        "called by"
                    };
                    let outcome = call
                        .target
                        .as_ref()
                        .map(|target| {
                            graph.graph[graph.ids[target]]
                                .symbol
                                .as_ref()
                                .map(|s| s.name.clone())
                                .unwrap_or_else(|| {
                                    display_path(
                                        graph.graph[graph.ids[target]]
                                            .module_path
                                            .as_deref()
                                            .unwrap_or_default(),
                                    )
                                })
                        })
                        .unwrap_or_else(|| format!("unresolved ({:?})", call.reason));
                    lines.push(format!(
                        "{} {} -> {} @ {}:{}",
                        direction,
                        safe_text(&call.expression),
                        outcome,
                        display_path(&call.source.path),
                        call.source.range.start.line
                    ));
                    related += 1;
                }
            }
            if related == 0 {
                lines.push(format!("No call-site evidence [{}]", role_name(role)));
            }
        }
        if component.cycle {
            lines.push("Cycle members (combined view):".to_owned());
            for member in &component.members {
                lines.push(format!("- {}", self.node_title(member)));
            }
            for ((caller, callee), contributors) in &self.edges {
                if component.members.contains(caller) && component.members.contains(callee) {
                    for (role, _, _) in contributors {
                        lines.push(format!(
                            "Edge [{}]: {} -> {}",
                            role_name(*role),
                            self.node_title(caller),
                            self.node_title(callee)
                        ));
                    }
                }
            }
        }
        lines.extend(self.diagnostic_lines());
        lines
    }
    pub fn diagnostic_lines(&self) -> Vec<String> {
        let capture = &self.session.capture.diagnostics;
        let extracted = self
            .session
            .base
            .diagnostics
            .iter()
            .chain(&self.session.target.diagnostics);
        let changed = &self.session.changes.diagnostics;
        if capture.is_empty()
            && self.session.base.diagnostics.is_empty()
            && self.session.target.diagnostics.is_empty()
            && changed.is_empty()
        {
            return Vec::new();
        }
        let mut lines = vec!["Diagnostics:".to_owned()];
        for item in capture {
            lines.push(format!(
                "Git {}: {}",
                display_path(&item.path),
                safe_text(&item.message)
            ));
        }
        for item in extracted.chain(changed) {
            lines.push(format!(
                "{:?} [{}] {}: {}",
                item.kind,
                role_name(item.snapshot),
                display_path(&item.path),
                safe_text(&item.message)
            ));
        }
        lines
    }
    fn select_node(&mut self, delta: isize) {
        if self.layout.order.is_empty() {
            return;
        }
        let at = self
            .selected_node
            .as_ref()
            .and_then(|id| self.layout.order.iter().position(|n| n == id))
            .unwrap_or(0);
        let next = at
            .saturating_add_signed(delta)
            .min(self.layout.order.len() - 1);
        self.selected_node = Some(self.layout.order[next].clone());
        self.relayout();
    }
    fn center_selected(&mut self) {
        if let Some(component) = self.layout.components.iter().find(|component| {
            self.selected_node
                .as_ref()
                .is_some_and(|id| component.members.contains(id))
        }) {
            self.pan_x = component.x.saturating_sub(6);
            self.pan_y = component.y.saturating_sub(2);
        }
    }
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }
        if self.help {
            self.help = false;
            return key.code == KeyCode::Char('q');
        }
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('d') => {
                self.expanded = !self.expanded;
                self.relayout();
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.depth = self.depth.saturating_add(1);
                self.refresh();
            }
            KeyCode::Char('-') => {
                self.depth = self.depth.saturating_sub(1);
                self.refresh();
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.pan_y = self.pan_y.saturating_sub(2)
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.pan_y = self.pan_y.saturating_add(2)
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.pan_x = self.pan_x.saturating_sub(4)
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.pan_x = self.pan_x.saturating_add(4)
            }
            KeyCode::Up | KeyCode::Left | KeyCode::Char('k') => self.select_node(-1),
            KeyCode::Down | KeyCode::Right | KeyCode::Char('j') => self.select_node(1),
            KeyCode::PageUp => self.pan_y = self.pan_y.saturating_sub(8),
            KeyCode::PageDown => self.pan_y = self.pan_y.saturating_add(8),
            _ => {}
        }
        false
    }
    pub fn clamp(&mut self, viewport: Rect) {
        self.pan_x = self
            .pan_x
            .clamp(0, (self.layout.width - i32::from(viewport.width)).max(0));
        self.pan_y = self
            .pan_y
            .clamp(0, (self.layout.height - i32::from(viewport.height)).max(0));
    }
    fn node_box_title(&self, id: &str) -> String {
        let (_, node) = self.native(id);
        if let Some(symbol) = &node.symbol {
            return safe_text(&symbol.name);
        }
        display_path(
            node.module_path
                .as_deref()
                .unwrap_or_default()
                .rsplit(|byte| *byte == b'/')
                .next()
                .unwrap_or_default(),
        )
    }
    fn node_title(&self, id: &str) -> String {
        let (_, node) = self.native(id);
        let Some(path) = node
            .symbol
            .as_ref()
            .map(|s| s.source.path.as_slice())
            .or(node.module_path.as_deref())
        else {
            return id.to_owned();
        };
        let mut title = display_path(path.rsplit(|byte| *byte == b'/').next().unwrap());
        if let Some(symbol) = &node.symbol {
            title.push(':');
            title.push_str(&symbol.name);
        }
        title
    }
}
fn kind_name_order(kind: SymbolChangeKind) -> usize {
    match kind {
        SymbolChangeKind::Added => 0,
        SymbolChangeKind::Modified => 1,
        SymbolChangeKind::Deleted => 2,
    }
}
pub fn kind(kind: SymbolChangeKind) -> &'static str {
    match kind {
        SymbolChangeKind::Added => "added",
        SymbolChangeKind::Modified => "modified",
        SymbolChangeKind::Deleted => "deleted",
    }
}
pub fn role_name(role: SnapshotRole) -> &'static str {
    match role {
        SnapshotRole::Base => "base",
        SnapshotRole::Target => "target",
    }
}
pub fn display_path(path: &[u8]) -> String {
    safe_text(&String::from_utf8_lossy(path))
}
pub fn safe_text(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { '?' } else { ch })
        .collect()
}
fn node_text(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_ascii_graphic() || ch == ' ' {
                ch
            } else {
                '?'
            }
        })
        .collect()
}
