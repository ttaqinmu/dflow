use super::layout;
use crate::analysis::SymbolChangeKind;
use crate::cli::CompareMode;
use crate::git::SnapshotRole;
use crate::graph::{self, Impact, SnapshotGraph};
use crate::report::AnalysisSession;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Changes,
    Graph,
}
impl Focus {
    fn next(self) -> Self {
        match self {
            Self::Changes => Self::Graph,
            Self::Graph => Self::Changes,
        }
    }
}
pub struct Root {
    pub id: String,
    pub kind: SymbolChangeKind,
    pub role: SnapshotRole,
}
pub struct App<'a> {
    pub session: &'a AnalysisSession,
    pub mode: &'a CompareMode,
    pub roots: Vec<Root>,
    pub root_index: usize,
    pub selected_node: Option<String>,
    pub impact: Option<Impact>,
    pub layout: layout::Layout,
    pub depth: usize,
    pub focus: Focus,
    pub expanded: bool,
    pub pan_x: i32,
    pub pan_y: i32,
    pub help: bool,
}
impl<'a> App<'a> {
    pub fn new(session: &'a AnalysisSession, mode: &'a CompareMode, depth: usize) -> Self {
        let roots = session
            .changes
            .changes
            .iter()
            .filter_map(|change| {
                let (symbol, graph, role) = if change.kind == SymbolChangeKind::Deleted {
                    (
                        change.before.as_ref()?,
                        &session.base_graph,
                        SnapshotRole::Base,
                    )
                } else {
                    (
                        change.after.as_ref()?,
                        &session.target_graph,
                        SnapshotRole::Target,
                    )
                };
                graph
                    .symbol_ids
                    .get(&(symbol.unit.clone(), symbol.scope_id))
                    .map(|id| Root {
                        id: id.clone(),
                        kind: change.kind,
                        role,
                    })
            })
            .collect();
        let mut app = Self {
            session,
            mode,
            roots,
            root_index: 0,
            selected_node: None,
            impact: None,
            layout: layout::Layout::default(),
            depth,
            focus: Focus::Changes,
            expanded: false,
            pan_x: 0,
            pan_y: 0,
            help: false,
        };
        app.refresh();
        app
    }
    pub fn root(&self) -> Option<&Root> {
        self.roots.get(self.root_index)
    }
    pub fn graph(&self) -> Option<&SnapshotGraph> {
        Some(match self.root()?.role {
            SnapshotRole::Base => &self.session.base_graph,
            SnapshotRole::Target => &self.session.target_graph,
        })
    }
    fn refresh(&mut self) {
        let Some(root) = self.root() else {
            self.impact = None;
            self.layout = layout::Layout::default();
            self.selected_node = None;
            return;
        };
        let graph = self.graph().unwrap();
        let impact = graph::callers(graph, &root.id, self.depth);
        let view = layout::build(&impact);
        if !self
            .selected_node
            .as_ref()
            .is_some_and(|id| view.order.contains(id))
        {
            self.selected_node = Some(root.id.clone());
        }
        self.impact = Some(impact);
        self.layout = view;
        self.relayout();
    }
    fn relayout(&mut self) {
        let mut view = std::mem::take(&mut self.layout);
        for component in &mut view.components {
            if component.title.is_empty() {
                let graph = self.graph().expect("graph exists for layout components");
                let first = &graph.graph[graph.ids[&component.members[0]]];
                let mut title = String::new();
                for id in &component.members {
                    if !title.is_empty() {
                        title.push_str(", ");
                    }
                    title.push_str(&self.node_title(id));
                }
                component.title = node_text(&title);
                let location = if let Some(symbol) = &first.symbol {
                    let range = symbol.source.range;
                    format!(
                        "Location: {}:{}:{}-{}:{}",
                        display_path(&symbol.source.path),
                        range.start.line,
                        range.start.byte_column,
                        range.end.line,
                        range.end.byte_column
                    )
                } else {
                    format!(
                        "Location: {}",
                        display_path(first.module_path.as_deref().unwrap_or_default())
                    )
                };
                component.description = node_text(&if component.cycle {
                    format!("Cycle ({}) | {location}", component.members.len())
                } else {
                    location
                });
            }
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
        let graph = self.graph().unwrap();
        let node = &graph.graph[graph.ids[id]];
        let mut lines = vec![format!("Snapshot: {}", role(graph.role))];
        if let Some(root) = self.root().filter(|root| root.id == id) {
            lines.push(format!("Change: {}", kind(root.kind)));
        }
        if let Some(symbol) = &node.symbol {
            lines.push(format!("Kind: {:?}", symbol.kind));
            lines.push(format!("Path: {}", display_path(&symbol.source.path)));
            let range = symbol.source.range;
            lines.push(format!(
                "Range: {}:{}-{}:{}",
                range.start.line, range.start.byte_column, range.end.line, range.end.byte_column
            ));
            let captured = match graph.role {
                SnapshotRole::Base => &self.session.capture.pair.base,
                SnapshotRole::Target => &self.session.capture.pair.target,
            };
            if let Some(file) = captured.files.get(&symbol.source.path) {
                lines.push("Source:".to_owned());
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
            if call.caller == id || call.target.as_deref() == Some(id) {
                if related == 0 {
                    lines.push("Call evidence:".to_owned());
                }
                let direction = if call.caller == id {
                    "calls"
                } else {
                    "called by"
                };
                let outcome = call
                    .target
                    .as_ref()
                    .map(|target| self.node_title(target))
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
            lines.push("No call-site evidence".to_owned());
        }
        if component.cycle {
            lines.push("Cycle members:".to_owned());
            for member in &component.members {
                lines.push(format!("- {}", self.node_title(member)));
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
                role(item.snapshot),
                display_path(&item.path),
                safe_text(&item.message)
            ));
        }
        lines
    }
    fn select_root(&mut self, next: usize) {
        if next < self.roots.len() && next != self.root_index {
            self.root_index = next;
            self.selected_node = None;
            self.refresh();
        }
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
            KeyCode::Tab => self.focus = self.focus.next(),
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
                self.pan_y = self.pan_y.saturating_sub(2);
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.pan_y = self.pan_y.saturating_add(2);
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.pan_x = self.pan_x.saturating_sub(4);
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.pan_x = self.pan_x.saturating_add(4);
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Left if self.focus == Focus::Graph => self.select_node(-1),
            KeyCode::Right if self.focus == Focus::Graph => self.select_node(1),
            KeyCode::PageUp => self.page(-1),
            KeyCode::PageDown => self.page(1),
            _ => {}
        }
        false
    }
    fn move_selection(&mut self, delta: isize) {
        match self.focus {
            Focus::Changes => self.select_root(self.root_index.saturating_add_signed(delta)),
            Focus::Graph => self.select_node(delta),
        }
    }
    fn page(&mut self, direction: isize) {
        match self.focus {
            Focus::Changes => {
                self.select_root(self.root_index.saturating_add_signed(direction * 8))
            }
            Focus::Graph => self.pan_y = self.pan_y.saturating_add((direction * 8) as i32),
        }
    }
    pub fn clamp(&mut self, viewport: Rect) {
        self.pan_x = self
            .pan_x
            .clamp(0, (self.layout.width - i32::from(viewport.width)).max(0));
        self.pan_y = self
            .pan_y
            .clamp(0, (self.layout.height - i32::from(viewport.height)).max(0));
    }
    fn node_title(&self, id: &str) -> String {
        let Some(graph) = self.graph() else {
            return id.to_owned();
        };
        let Some(index) = graph.ids.get(id) else {
            return id.to_owned();
        };
        let node = &graph.graph[*index];
        let Some(path) = node
            .symbol
            .as_ref()
            .map(|symbol| symbol.source.path.as_slice())
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
    pub fn name_for_root(&self, root: &Root) -> String {
        let graph = match root.role {
            SnapshotRole::Base => &self.session.base_graph,
            SnapshotRole::Target => &self.session.target_graph,
        };
        safe_text(
            &graph
                .ids
                .get(&root.id)
                .and_then(|index| graph.graph[*index].symbol.as_ref())
                .map(|symbol| symbol.qualified_name.clone())
                .unwrap_or_else(|| root.id.clone()),
        )
    }
}
pub fn kind(kind: SymbolChangeKind) -> &'static str {
    match kind {
        SymbolChangeKind::Added => "added",
        SymbolChangeKind::Modified => "modified",
        SymbolChangeKind::Deleted => "deleted",
    }
}
pub fn role(role: SnapshotRole) -> &'static str {
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
