use dflow_core::analysis::SymbolChangeKind;
use dflow_core::comparison::{Comparison, LocalSelection};
use dflow_core::git::{CapturedSnapshot, SnapshotRole};
use dflow_core::graph::EdgeChangeKind;
use dflow_core::report::{AnalysisSession, FocusedGraph, FocusedNode};
use eframe::egui::{
    self, Color32, FontId, Pos2, Shape, Stroke, Vec2,
    epaint::{CircleShape, TextShape},
};
use egui_graphs::{
    DisplayNode, DrawContext, GraphChange, GraphView, LayoutHierarchical,
    LayoutHierarchicalOrientation, LayoutStateHierarchical, NodeProps, SettingsInteraction,
    SettingsNavigation, to_graph_custom,
};
use petgraph::Directed;
use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use std::collections::BTreeMap;
use std::sync::Arc;

const VIEW_ID: &str = "dflow-impact-graph";

type WidgetGraph = egui_graphs::Graph<NodeView, String, Directed, u32, BadgeNode>;

#[derive(Clone)]
struct NodeView {
    label: String,
    color: Color32,
}

// Keep graph topology, layout and interactions in egui_graphs; only the node
// glyph differs from its circle-only default so labels remain legible.
#[derive(Clone)]
struct BadgeNode {
    pos: Pos2,
    label: String,
    color: Color32,
    selected: bool,
    galley: Option<Arc<egui::Galley>>,
    text_color: Color32,
}

impl From<NodeProps<NodeView>> for BadgeNode {
    fn from(props: NodeProps<NodeView>) -> Self {
        Self {
            pos: props.location(),
            color: props.color().unwrap_or(Color32::GRAY),
            label: props.label,
            selected: props.selected,
            galley: None,
            text_color: Color32::TRANSPARENT,
        }
    }
}

impl DisplayNode<NodeView, String, Directed, u32> for BadgeNode {
    fn closest_boundary_point(&self, dir: Vec2) -> Pos2 {
        self.pos
            + if dir == Vec2::ZERO {
                Vec2::X
            } else {
                dir.normalized()
            } * 14.0
    }

    fn is_inside(&self, pos: Pos2) -> bool {
        self.pos.distance(pos) <= 14.0
    }

    fn update(&mut self, props: &NodeProps<NodeView>) {
        self.pos = props.location();
        if self.label != props.label {
            self.label.clone_from(&props.label);
            self.galley = None;
        }
        self.color = props.color().unwrap_or(Color32::GRAY);
        self.selected = props.selected;
    }

    fn shapes(&mut self, ctx: &DrawContext) -> Vec<Shape> {
        let center = ctx.meta.canvas_to_screen_pos(self.pos);
        let radius = ctx.meta.canvas_to_screen_size(14.0);
        let text_color = ctx.ctx.global_style().visuals.text_color();
        if self.galley.is_none() || self.text_color != text_color {
            self.galley = Some(ctx.ctx.fonts_mut(|fonts| {
                fonts.layout_no_wrap(self.label.clone(), FontId::monospace(14.0), text_color)
            }));
            self.text_color = text_color;
        }
        let galley = self.galley.as_ref().unwrap();
        let text_pos = Pos2::new(
            center.x - galley.size().x / 2.0,
            center.y - radius - galley.size().y - 8.0,
        );
        let background = egui::Rect::from_min_size(
            text_pos - Vec2::splat(4.0),
            galley.size() + Vec2::splat(8.0),
        );
        vec![
            Shape::rect_filled(background, 3.0, ctx.ctx.global_style().visuals.panel_fill),
            TextShape::new(text_pos, Arc::clone(galley), text_color).into(),
            CircleShape {
                center,
                radius,
                fill: self.color,
                stroke: Stroke::new(if self.selected { 3.0 } else { 1.0 }, text_color),
            }
            .into(),
        ]
    }
}
struct Projection {
    focused: FocusedGraph,
    graph: WidgetGraph,
    ids: BTreeMap<String, NodeIndex>,
    reverse: Vec<String>,
}

fn path(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn changes(node: &FocusedNode) -> String {
    if node.changes.is_empty() {
        "caller".into()
    } else {
        node.changes
            .iter()
            .map(|kind| match kind {
                SymbolChangeKind::Added => "added",
                SymbolChangeKind::Modified => "modified",
                SymbolChangeKind::Deleted => "deleted",
            })
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

fn color(node: &FocusedNode) -> Color32 {
    match node.changes.as_slice() {
        [SymbolChangeKind::Added] => Color32::from_rgb(48, 166, 97),
        [SymbolChangeKind::Deleted] => Color32::from_rgb(206, 78, 78),
        [SymbolChangeKind::Modified] => Color32::from_rgb(210, 162, 54),
        [] => Color32::from_rgb(90, 124, 151),
        _ => Color32::from_rgb(165, 95, 173),
    }
}

fn edge_label(focused: &FocusedGraph, key: &(String, String)) -> &'static str {
    match focused.edge_status.get(key) {
        Some(EdgeChangeKind::Added) => "+ target",
        Some(EdgeChangeKind::Deleted) => "- base",
        None => {
            let contributors = &focused.edges[key];
            let base = contributors
                .iter()
                .any(|(role, _, _)| *role == SnapshotRole::Base);
            let target = contributors
                .iter()
                .any(|(role, _, _)| *role == SnapshotRole::Target);
            match (base, target) {
                (true, true) => "both",
                (true, false) => "base",
                (false, true) => "target",
                (false, false) => "unknown",
            }
        }
    }
}

fn project(session: &AnalysisSession, depth: usize) -> Projection {
    let focused = session.focused_graph(depth);
    let mut source = StableDiGraph::<NodeView, String>::new();
    let mut ids = BTreeMap::new();
    let mut reverse = Vec::new();
    for (id, _) in &focused.impact.nodes {
        let provenance = &focused.nodes[id];
        let (role, native) = if let Some(native) = &provenance.target {
            (SnapshotRole::Target, native)
        } else {
            (
                SnapshotRole::Base,
                provenance.base.as_ref().expect("focused node provenance"),
            )
        };
        let graph = if role == SnapshotRole::Base {
            &session.base_graph
        } else {
            &session.target_graph
        };
        let node = &graph.graph[graph.ids[native]];
        let (title, location) = if let Some(symbol) = &node.symbol {
            (symbol.qualified_name.clone(), path(&symbol.source.path))
        } else {
            let module = node.module_path.as_deref().unwrap_or_default();
            (
                path(module.rsplit(|byte| *byte == b'/').next().unwrap_or(module)),
                path(module),
            )
        };
        let provenance_label = match (&provenance.base, &provenance.target) {
            (Some(_), Some(_)) => "base+target",
            (Some(_), None) => "base",
            (None, Some(_)) => "target",
            (None, None) => unreachable!("focused node provenance"),
        };
        let label = format!(
            "{title}\n{location}\n{provenance_label}\n{}",
            changes(provenance)
        );
        let index = source.add_node(NodeView {
            label,
            color: color(provenance),
        });
        ids.insert(id.clone(), index);
        reverse.push(id.clone());
    }
    for key @ (caller, callee) in focused.edges.keys() {
        if let (Some(&from), Some(&to)) = (ids.get(caller), ids.get(callee)) {
            source.add_edge(from, to, edge_label(&focused, key).into());
        }
    }
    let graph: WidgetGraph = to_graph_custom(
        &source,
        |node: &mut egui_graphs::Node<NodeView, String, Directed, u32, BadgeNode>| {
            node.set_label(node.payload().label.clone());
            node.set_color(node.payload().color);
        },
        |edge| edge.set_label(edge.payload().clone()),
    );
    Projection {
        focused,
        graph,
        ids,
        reverse,
    }
}

fn snapshot_label(snapshot: &CapturedSnapshot) -> String {
    format!(
        "{:?} {} {}",
        snapshot.kind,
        snapshot.branch.as_deref().unwrap_or(""),
        snapshot
            .commit_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| snapshot.content_id.clone())
    )
}

fn mode_label(mode: &Comparison) -> String {
    match mode {
        Comparison::Local(selection) => format!(
            "local {}",
            match selection {
                LocalSelection::Combined => "combined",
                LocalSelection::Staged => "staged",
                LocalSelection::Unstaged => "unstaged",
            }
        ),
        Comparison::Branch { base, target } => format!("branch {base} → {target}"),
    }
}

fn node_details(session: &AnalysisSession, focused: &FocusedGraph, id: &str) -> Vec<String> {
    let provenance = &focused.nodes[id];
    let mut lines = vec![format!("Status: {}", changes(provenance))];
    for change in &session.edge_comparison.changes {
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
        let graph = if role == SnapshotRole::Base {
            &session.base_graph
        } else {
            &session.target_graph
        };
        let location = graph
            .calls
            .iter()
            .find(|call| call.caller == pair.0 && call.target.as_deref() == Some(&pair.1))
            .map(|call| {
                format!(
                    " @ {}:{}",
                    path(&call.source.path),
                    call.source.range.start.line
                )
            })
            .unwrap_or_default();
        lines.push(format!(
            "Edge {:?} [{role:?}]: {} → {}{location}",
            change.kind, change.caller_view, change.callee_view
        ));
    }
    for (role, native) in [
        (SnapshotRole::Base, &provenance.base),
        (SnapshotRole::Target, &provenance.target),
    ] {
        let Some(native) = native else {
            continue;
        };
        let (graph, capture) = if role == SnapshotRole::Base {
            (&session.base_graph, &session.capture.pair.base)
        } else {
            (&session.target_graph, &session.capture.pair.target)
        };
        let node = &graph.graph[graph.ids[native]];
        lines.push(format!("\n{role:?}: {}", snapshot_label(capture)));
        if let Some(symbol) = &node.symbol {
            let range = symbol.source.range;
            lines.push(format!("{:?} {}", symbol.kind, symbol.qualified_name));
            lines.push(format!(
                "{}:{}:{}–{}:{}",
                path(&symbol.source.path),
                range.start.line,
                range.start.byte_column,
                range.end.line,
                range.end.byte_column
            ));
            if let Some(file) = capture.files.get(&symbol.source.path) {
                lines.push(format!("Captured source [{role:?}]:"));
                for (line, text) in String::from_utf8_lossy(&file.bytes)
                    .lines()
                    .enumerate()
                    .skip(range.start.line.saturating_sub(2))
                    .take(5)
                {
                    lines.push(format!("{:>4} {text}", line + 1));
                }
            }
        } else {
            lines.push(format!(
                "Module: {}",
                path(node.module_path.as_deref().unwrap_or_default())
            ));
        }
        let mut related = false;
        for call in &graph.calls {
            if call.caller != *native && call.target.as_deref() != Some(native) {
                continue;
            }
            related = true;
            let target = call
                .target
                .as_ref()
                .map(|target| {
                    let node = &graph.graph[graph.ids[target]];
                    node.symbol
                        .as_ref()
                        .map(|s| s.qualified_name.clone())
                        .unwrap_or_else(|| path(node.module_path.as_deref().unwrap_or_default()))
                })
                .unwrap_or_else(|| format!("unresolved ({:?})", call.reason));
            lines.push(format!(
                "{} {} → {target} @ {}:{}",
                if call.caller == *native {
                    "calls"
                } else {
                    "called by"
                },
                call.expression,
                path(&call.source.path),
                call.source.range.start.line
            ));
        }
        if !related {
            lines.push(format!("No call-site evidence [{role:?}]"));
        }
    }
    lines
}

struct App {
    session: AnalysisSession,
    mode_text: String,
    depth: usize,
    projection: Projection,
    selected: Option<String>,
    details: Vec<String>,
    layout_pending: bool,
    diagnostics: Vec<String>,
    base_label: String,
    target_label: String,
    complete: bool,
}

impl App {
    fn new(session: AnalysisSession, mode: Comparison, depth: usize) -> Self {
        let diagnostics = session.report(&mode, depth)["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|diagnostic| {
                format!(
                    "{}: {}",
                    diagnostic["category"].as_str().unwrap_or("diagnostic"),
                    diagnostic["message"].as_str().unwrap_or("")
                )
            })
            .collect();
        let projection = project(&session, depth);
        Self {
            mode_text: mode_label(&mode),
            base_label: format!("Base: {}", snapshot_label(&session.capture.pair.base)),
            target_label: format!("Target: {}", snapshot_label(&session.capture.pair.target)),
            complete: session.complete(),
            session,
            depth,
            projection,
            selected: None,
            details: Vec::new(),
            layout_pending: true,
            diagnostics,
        }
    }

    fn change_depth(&mut self, depth: usize) {
        if depth == self.depth {
            return;
        }
        self.depth = depth;
        self.projection = project(&self.session, depth);
        self.selected = None;
        self.details.clear();
        self.layout_pending = true;
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        egui::Panel::top("status").min_size(76.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("DFlow");
                ui.label(self.mode_text.as_str());
                ui.separator();
                ui.label(format!("Depth {}", self.depth));
                if ui.button("−").clicked() {
                    self.change_depth(self.depth.saturating_sub(1));
                }
                if ui.button("+").clicked() {
                    self.change_depth(self.depth.saturating_add(1));
                }
                ui.separator();
                ui.label(if self.complete {
                    "Complete"
                } else {
                    "Partial analysis"
                });
            });
            ui.label(self.base_label.as_str());
            ui.label(self.target_label.as_str());
            for diagnostic in &self.diagnostics {
                ui.colored_label(Color32::YELLOW, diagnostic);
            }
        });
        egui::Panel::right("details")
            .min_size(300.0)
            .show(ui, |ui| {
                ui.heading("Details");
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if self.selected.is_some() {
                        for line in &self.details {
                            ui.label(line);
                        }
                    } else {
                        ui.label("Select a node to inspect captured source and call evidence.");
                    }
                });
            });
        egui::CentralPanel::default().show(ui, |ui| {
            if self.projection.ids.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label("No changed symbols or edges at this depth.");
                });
                return;
            }
            if self.layout_pending {
                egui_graphs::reset::<LayoutStateHierarchical>(ui, Some(VIEW_ID.into()));
                egui_graphs::set_layout_state(
                    ui,
                    LayoutStateHierarchical {
                        orientation: LayoutHierarchicalOrientation::LeftRight,
                        row_dist: 260.0,
                        col_dist: 180.0,
                        ..Default::default()
                    },
                    Some(VIEW_ID.into()),
                );
                self.layout_pending = false;
            }
            let changes = GraphView::<LayoutStateHierarchical, LayoutHierarchical>::new()
                .with_id(Some(VIEW_ID.into()))
                .with_styles(&egui_graphs::SettingsStyle::new().with_labels_always(true))
                .with_navigations(
                    &SettingsNavigation::new()
                        .with_zoom_and_pan_enabled(true)
                        .with_fit_to_screen_enabled(false)
                        .with_fit_to_screen_padding(1.0),
                )
                .with_interactions(
                    &SettingsInteraction::new()
                        .with_dragging_enabled(true)
                        .with_node_selection_enabled(true),
                )
                .show(ui, &mut self.projection.graph)
                .changes;
            for change in changes {
                match change {
                    GraphChange::NodeSelected { node } => {
                        let selected = self.projection.reverse.get(node.index()).cloned();
                        if selected != self.selected {
                            self.details = selected
                                .as_ref()
                                .map(|id| node_details(&self.session, &self.projection.focused, id))
                                .unwrap_or_default();
                            self.selected = selected;
                        }
                    }
                    GraphChange::NodeDeselected { node }
                        if self.selected.as_deref()
                            == self
                                .projection
                                .reverse
                                .get(node.index())
                                .map(String::as_str) =>
                    {
                        self.selected = None;
                        self.details.clear();
                    }
                    _ => {}
                }
            }
        });
    }
}

pub fn run(
    session: AnalysisSession,
    mode: &Comparison,
    depth: usize,
) -> Result<bool, eframe::Error> {
    let complete = session.complete();
    let app = App::new(session, mode.clone(), depth);
    eframe::run_native(
        "DFlow graph",
        eframe::NativeOptions::default(),
        Box::new(move |_| Ok(Box::new(app))),
    )?;
    Ok(complete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dflow_core::git::{CaptureResult, CapturedFile, SnapshotKind, SnapshotPair};

    fn snapshot(role: SnapshotRole, source: &[u8]) -> CapturedSnapshot {
        let file = CapturedFile {
            path: b"a.py".to_vec(),
            bytes: source.to_vec(),
            mode: 0o100644,
        };
        CapturedSnapshot {
            kind: SnapshotKind::WorkingTree,
            role,
            commit_id: None,
            branch: None,
            files: [(b"a.py".to_vec(), file)].into(),
            content_id: format!("{role:?}"),
        }
    }
    fn session(base: &[u8], target: &[u8]) -> AnalysisSession {
        AnalysisSession::new(
            CaptureResult {
                pair: SnapshotPair {
                    base: snapshot(SnapshotRole::Base, base),
                    target: snapshot(SnapshotRole::Target, target),
                },
                changes: vec![],
                diagnostics: vec![],
            },
            true,
        )
    }
    #[test]
    fn retargeted_calls_keep_direction_status_and_captured_provenance() {
        let session = session(
            b"def B(): pass\ndef A(): B()\n",
            b"def C(): pass\ndef A(): C()\n",
        );
        let view = project(&session, 1);
        assert_eq!(view.focused.edges.len(), 2);
        let mut edges = view
            .focused
            .edges
            .keys()
            .map(|key| {
                let from = &view.graph.g()[view.ids[&key.0]];
                let to = &view.graph.g()[view.ids[&key.1]];
                (from.label(), to.label(), edge_label(&view.focused, key))
            })
            .collect::<Vec<_>>();
        edges.sort();
        assert!(
            edges
                .iter()
                .any(|(a, b, status)| a.contains("A") && b.contains("B") && *status == "- base")
        );
        assert!(
            edges
                .iter()
                .any(|(a, b, status)| a.contains("A") && b.contains("C") && *status == "+ target")
        );
        assert_eq!(view.graph.g().edge_count(), 2);
        for (name, expected, unexpected) in
            [("B", "def B()", "def C()"), ("C", "def C()", "def B()")]
        {
            let id = view
                .ids
                .keys()
                .find(|id| {
                    let label = view.graph.g()[view.ids[*id]].label();
                    label.starts_with(name)
                })
                .unwrap();
            let node = &view.graph.g()[view.ids[id]];
            let label = node.label();
            let parts = label.lines().collect::<Vec<_>>();
            assert_eq!(parts[2], if name == "B" { "base" } else { "target" });
            assert_eq!(parts[3], if name == "B" { "deleted" } else { "added" });
            let detail = node_details(&session, &view.focused, id).join("\n");
            assert!(detail.contains(expected), "{detail}");
            assert!(!detail.contains(unexpected), "{detail}");
        }
    }

    #[test]
    fn cycles_and_shared_callers_keep_unique_directed_edges() {
        let session = session(
            b"def A(): B()\ndef B(): A()\ndef X(): A()\n",
            b"def A(): B()\ndef B(): A()\ndef X(): B()\n",
        );
        let view = project(&session, 3);
        assert_eq!(view.graph.g().edge_count(), view.focused.edges.len());
        assert!(
            view.focused
                .edges
                .keys()
                .any(|(a, b)| a != b && view.focused.edges.contains_key(&(b.clone(), a.clone())))
        );
    }
}
