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
    Details,
}
impl Focus {
    fn next(self) -> Self {
        match self {
            Self::Changes => Self::Graph,
            Self::Graph => Self::Details,
            Self::Details => Self::Changes,
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
    pub pan_x: i32,
    pub pan_y: i32,
    pub details_scroll: u16,
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
                    .get(&(symbol.source.path.clone(), symbol.scope_id))
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
            pan_x: 0,
            pan_y: 0,
            details_scroll: 0,
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
        self.center_selected();
        self.details_scroll = 0;
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
        self.center_selected();
        self.details_scroll = 0;
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
            Focus::Details => {
                self.details_scroll = self.details_scroll.saturating_add_signed(delta as i16)
            }
        }
    }
    fn page(&mut self, direction: isize) {
        match self.focus {
            Focus::Changes => {
                self.select_root(self.root_index.saturating_add_signed(direction * 8))
            }
            Focus::Graph => self.pan_y = self.pan_y.saturating_add((direction * 8) as i32),
            Focus::Details => {
                self.details_scroll = self
                    .details_scroll
                    .saturating_add_signed((direction * 8) as i16)
            }
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
    pub fn name(&self, id: &str) -> String {
        let Some(graph) = self.graph() else {
            return id.to_owned();
        };
        let Some(index) = graph.ids.get(id) else {
            return id.to_owned();
        };
        let node = &graph.graph[*index];
        safe_text(
            &node
                .symbol
                .as_ref()
                .map(|s| s.qualified_name.clone())
                .or_else(|| {
                    node.module_path
                        .as_ref()
                        .map(|p| format!("module {}", display_path(p)))
                })
                .unwrap_or_else(|| id.to_owned()),
        )
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
    pub fn root_kind(&self, id: &str) -> &'static str {
        self.root()
            .filter(|root| root.id == id)
            .map(|root| kind(root.kind))
            .unwrap_or("")
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
