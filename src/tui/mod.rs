mod layout;
mod render;
mod state;

use crate::cli::CompareMode;
use crate::report::AnalysisSession;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use std::io::{self, Stdout};
use std::sync::{Arc, Mutex};

struct TerminalGuard {
    raw: bool,
    alternate: bool,
}
impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        let mut guard = Self {
            raw: false,
            alternate: false,
        };
        enable_raw_mode()?;
        guard.raw = true;
        execute!(io::stdout(), EnterAlternateScreen)?;
        guard.alternate = true;
        Ok(guard)
    }
    fn restore(&mut self) -> io::Result<()> {
        let mut failure = None;
        if self.alternate {
            if let Err(error) = execute!(io::stdout(), LeaveAlternateScreen) {
                failure = Some(error);
            }
            self.alternate = false;
        }
        if self.raw {
            if let Err(error) = disable_raw_mode() {
                failure.get_or_insert(error);
            }
            self.raw = false;
        }
        failure.map_or(Ok(()), Err)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

pub fn run(session: &AnalysisSession, mode: &CompareMode, depth: usize) -> io::Result<()> {
    let mut app = state::App::new(session, mode, depth);
    let guard = Arc::new(Mutex::new(TerminalGuard::enter()?));
    let previous = Arc::new(std::panic::take_hook());
    let panic_guard = Arc::clone(&guard);
    let panic_previous = Arc::clone(&previous);
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut guard) = panic_guard.lock() {
            let _ = guard.restore();
        }
        panic_previous(info);
    }));
    let result = (|| {
        // Debug-only terminal lifecycle probes for the pseudoterminal validation harness.
        #[cfg(debug_assertions)]
        match std::env::var("DFLOW_INTERNAL_TUI_FAULT").as_deref() {
            Ok("panic") => panic!("injected terminal panic"),
            Ok("error") => return Err(io::Error::other("injected terminal error")),
            _ => {}
        }
        let mut terminal = Terminal::new(CrosstermBackend::<Stdout>::new(io::stdout()))?;
        loop {
            let size = terminal.size()?;
            let viewport = Rect::new(
                0,
                0,
                size.width.saturating_sub(27),
                size.height.saturating_sub(8),
            );
            app.clamp(viewport);
            terminal.draw(|frame| render::render(&app, frame))?;
            match event::read()? {
                Event::Key(key) if app.handle_key(key) => break,
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
        Ok(())
    })();
    let cleanup = guard
        .lock()
        .map_err(|_| io::Error::other("terminal cleanup state poisoned"))
        .and_then(|mut guard| guard.restore());
    std::panic::set_hook(Box::new(move |info| previous(info)));
    result.and(cleanup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::SymbolChangeKind;
    use crate::cli::{AnalyzeOptions, LocalArgs};
    use crate::git::{
        CaptureResult, CapturedFile, CapturedSnapshot, SnapshotKind, SnapshotPair, SnapshotRole,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Modifier};
    use std::collections::BTreeMap;
    fn snapshot(role: SnapshotRole, files: &[(&[u8], &[u8])]) -> CapturedSnapshot {
        CapturedSnapshot {
            role,
            kind: SnapshotKind::WorkingTree,
            commit_id: None,
            branch: None,
            content_id: format!("{role:?}"),
            files: files
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
                .collect::<BTreeMap<_, _>>(),
        }
    }
    fn session(base: &[(&[u8], &[u8])], target: &[(&[u8], &[u8])]) -> AnalysisSession {
        AnalysisSession::new(CaptureResult {
            pair: SnapshotPair {
                base: snapshot(SnapshotRole::Base, base),
                target: snapshot(SnapshotRole::Target, target),
            },
            changes: vec![],
            diagnostics: vec![],
        })
    }
    fn mode() -> CompareMode {
        CompareMode::Local(LocalArgs {
            staged: false,
            unstaged: false,
            options: AnalyzeOptions {
                format: None,
                depth: 1,
            },
        })
    }
    fn screen(app: &state::App<'_>, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render::render(app, frame)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn node_line(
        app: &state::App<'_>,
        text: &str,
        component: &layout::Component,
        row: i32,
    ) -> String {
        let x = (26 + component.x - app.pan_x) as usize;
        let y = (5 + component.y - app.pan_y + row) as usize;
        text.lines()
            .nth(y)
            .unwrap()
            .chars()
            .skip(x + 1)
            .take((component.width - 2) as usize)
            .collect::<String>()
            .trim_end()
            .to_owned()
    }
    fn selected_component<'a>(app: &'a state::App<'_>, selected: &str) -> &'a layout::Component {
        app.layout
            .components
            .iter()
            .find(|component| component.members.iter().any(|id| id == selected))
            .unwrap()
    }

    #[test]
    fn graph_renders_arrow_labels_evidence_and_cycle_members() {
        let target = [(
            b"a.py".as_slice(),
            b"def f(): pass\ndef g(): f()\n".as_slice(),
        )];
        let fixture = session(&[], &target);
        let mode = mode();
        let app = state::App::new(&fixture, &mode, 1);
        let text = screen(&app, 120, 30);
        assert!(text.contains("Graph: caller -> callee"));
        assert!(!text.contains("Call evidence:"));
        assert!(!text.contains("Details"));
        let selected = app.selected_node.as_ref().unwrap();
        let root = app
            .layout
            .components
            .iter()
            .find(|component| component.members.contains(selected))
            .unwrap();
        assert_eq!(node_line(&app, &text, root, 1), root.title);
        assert_eq!(node_line(&app, &text, root, 3), root.description);
        assert!(root.width >= root.title.chars().count() as i32 + 2);
        assert_eq!(root.height, 5);
        assert!(
            text.lines().skip(5).take(23).any(|line| line
                .chars()
                .skip(26)
                .take(63)
                .any(|ch| ch == '→'))
        );
        let mut app = app;
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        let text = screen(&app, 120, 30);
        assert!(text.contains("Source:"));
        assert!(text.contains("Call evidence:"));
        let target = [(
            b"a.py".as_slice(),
            b"def a(): b()\ndef b(): a()\n".as_slice(),
        )];
        let cycle_session = session(&[], &target);
        let mut cycle_app = state::App::new(&cycle_session, &mode, 2);
        assert!(
            cycle_app
                .layout
                .components
                .iter()
                .any(|component| component.cycle)
        );
        let selected = cycle_app.selected_node.as_ref().unwrap();
        let cycle = cycle_app
            .layout
            .components
            .iter()
            .find(|component| component.members.contains(selected))
            .unwrap();
        let cycle_text = screen(&cycle_app, 120, 30);
        assert_eq!(node_line(&cycle_app, &cycle_text, cycle, 1), cycle.title);
        assert_eq!(
            node_line(&cycle_app, &cycle_text, cycle, 3),
            cycle.description
        );
        cycle_app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(screen(&cycle_app, 120, 30).contains("Cycle members:"));
    }
    #[test]
    fn module_caller_box_shows_its_name_without_type_prefix() {
        let target = [(b"a.py".as_slice(), b"def f(): pass\nf()\n".as_slice())];
        let fixture = session(&[], &target);
        let mode = mode();
        let mut app = state::App::new(&fixture, &mode, 1);
        app.pan_x = 0;
        app.pan_y = 0;
        let graph = app.graph().unwrap();
        let module = app
            .layout
            .components
            .iter()
            .find(|component| {
                graph.graph[graph.ids[&component.members[0]]]
                    .symbol
                    .is_none()
            })
            .unwrap();
        assert_eq!(node_line(&app, &screen(&app, 120, 30), module, 1), "a.py");
    }

    #[test]
    fn reducer_changes_depth_focus_selection_and_exits() {
        let target = [(
            b"a.py".as_slice(),
            b"def f(): pass\ndef g(): f()\n".as_slice(),
        )];
        let session = session(&[], &target);
        let mode = mode();
        let mut app = state::App::new(&session, &mode, 0);
        assert_eq!(app.layout.order.len(), 1);
        app.handle_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
        assert_eq!(app.depth, 1);
        assert_eq!(app.layout.order.len(), 2);
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(matches!(app.focus, state::Focus::Graph));
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(matches!(app.focus, state::Focus::Changes));
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert!(app.selected_node.is_some());
        app.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));
        assert_eq!(app.depth, 0);
        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(screen(&app, 120, 30).contains("DFlow keyboard help"));
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)));
    }
    #[test]
    fn expanding_selected_node_grows_box_without_overlap_and_tab_has_two_stops() {
        let target = [(
            b"a.py".as_slice(),
            b"def f(): pass\ndef g(): f()\n".as_slice(),
        )];
        let fixture = session(&[], &target);
        let mode = mode();
        let mut app = state::App::new(&fixture, &mode, 1);
        let selected = app.selected_node.clone().unwrap();
        let original = selected_component(&app, &selected);
        let (original_width, original_height) = (original.width, original.height);
        let hidden = screen(&app, 180, 40);
        assert!(!hidden.contains("Source:"));
        assert_eq!(original_height, 5);
        assert!(original_width >= 26);
        assert_eq!(node_line(&app, &hidden, original, 3), original.description);
        assert_eq!(hidden.lines().nth(6).unwrap().chars().nth(179), Some('│'));

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(matches!(app.focus, state::Focus::Graph));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        let expanded = selected_component(&app, &selected);
        assert!(expanded.height > original_height);
        assert!(expanded.extra.iter().any(|line| line.contains("Source:")));
        let shown = screen(&app, 180, 40);
        assert!(shown.contains("Source:"));
        assert!(shown.contains("Path:"));
        assert_eq!(node_line(&app, &shown, expanded, 1), expanded.title);
        for other in app
            .layout
            .components
            .iter()
            .filter(|other| !other.members.contains(&selected))
        {
            assert!(
                expanded.x + expanded.width <= other.x
                    || other.x + other.width <= expanded.x
                    || expanded.y + expanded.height <= other.y
                    || other.y + other.height <= expanded.y,
                "expanded box overlaps another node"
            );
        }
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(matches!(app.focus, state::Focus::Changes));
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(matches!(app.focus, state::Focus::Graph));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(selected_component(&app, &selected).height, original_height);
        assert!(selected_component(&app, &selected).extra.is_empty());
        assert_eq!(app.selected_node.as_ref(), Some(&selected));
        assert!(!screen(&app, 180, 40).contains("Source:"));
    }

    #[test]
    fn empty_partial_and_small_terminal_are_explicit() {
        let mode = mode();
        let empty = session(&[], &[]);
        let app = state::App::new(&empty, &mode, 1);
        assert!(screen(&app, 120, 30).contains("No changed symbols"));
        assert!(screen(&app, 70, 12).contains("Terminal too small"));
        let bad = session(&[], &[(b"bad.py", b"def f(:\n")]);
        let mut app = state::App::new(&bad, &mode, 1);
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        let text = screen(&app, 120, 30);
        assert!(text.contains("PARTIAL analysis"));
        assert!(text.contains("Diagnostics:"));
    }
    #[test]
    fn deleted_root_uses_base_graph_and_historical_evidence() {
        let base = [(
            b"a.py".as_slice(),
            b"def removed(): pass\ndef legacy(): removed()\n".as_slice(),
        )];
        let target = [(b"a.py".as_slice(), b"def legacy(): pass\n".as_slice())];
        let fixture = session(&base, &target);
        let mode = mode();
        let mut app = state::App::new(&fixture, &mode, 1);
        let deleted = app
            .roots
            .iter()
            .position(|root| root.kind == crate::analysis::SymbolChangeKind::Deleted)
            .unwrap();
        for _ in 0..deleted {
            app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        let selected = app.selected_node.as_ref().unwrap();
        let root = app
            .layout
            .components
            .iter()
            .find(|component| component.members.contains(selected))
            .unwrap();
        assert_eq!(
            node_line(&app, &screen(&app, 120, 30), root, 1),
            "a.py:removed"
        );
        let text = screen(&app, 120, 30);
        assert!(text.contains("[deleted] base"));
        assert!(text.contains("Change: deleted"));
        assert!(text.contains("called by"));
        assert_eq!(app.layout.order.len(), 2);
    }
    #[test]
    fn diff_colors_follow_kind_across_list_and_graph() {
        let base = [(
            b"a.py".as_slice(),
            b"def gone(): pass\ndef changed(): return 1\n".as_slice(),
        )];
        let target = [(
            b"a.py".as_slice(),
            b"def changed(): return 2\ndef fresh(): pass\ndef caller(): fresh()\n".as_slice(),
        )];
        let fixture = session(&base, &target);
        let mode = mode();
        for (kind, color) in [
            (SymbolChangeKind::Added, Color::Green),
            (SymbolChangeKind::Modified, Color::Yellow),
            (SymbolChangeKind::Deleted, Color::Red),
        ] {
            let mut app = state::App::new(&fixture, &mode, 1);
            let index = app
                .roots
                .iter()
                .position(|root| {
                    root.kind == kind
                        && (kind != SymbolChangeKind::Added || app.name_for_root(root) == "fresh")
                })
                .unwrap();
            for _ in 0..index {
                app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
            }
            app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
            let root = app.root().unwrap();
            assert_eq!(root.kind, kind);
            let component = app
                .layout
                .components
                .iter()
                .find(|component| component.members.contains(&root.id))
                .unwrap();
            let graph_x = (26 + component.x - app.pan_x) as u16;
            let graph_y = (5 + component.y - app.pan_y) as u16;
            let mut terminal = Terminal::new(TestBackend::new(180, 30)).unwrap();
            terminal.draw(|frame| render::render(&app, frame)).unwrap();
            let buffer = terminal.backend().buffer();
            for (row, entry) in app.roots.iter().enumerate() {
                let expected = match entry.kind {
                    SymbolChangeKind::Added => Color::Green,
                    SymbolChangeKind::Deleted => Color::Red,
                    SymbolChangeKind::Modified => Color::Yellow,
                };
                assert_eq!(buffer[(2, 5 + row as u16)].fg, expected);
            }
            let box_cell = &buffer[(graph_x, graph_y)];
            assert_eq!(box_cell.symbol(), "┌");
            assert_eq!(box_cell.fg, color);
            assert!(box_cell.modifier.contains(Modifier::REVERSED));
            for caller in app
                .layout
                .components
                .iter()
                .filter(|component| !component.members.contains(&root.id))
            {
                let x = (26 + caller.x - app.pan_x) as u16;
                let y = (5 + caller.y - app.pan_y) as u16;
                if x < 149 && y < 28 {
                    assert_eq!(buffer[(x, y)].fg, Color::Reset);
                }
            }
        }
    }

    #[test]
    fn selected_node_title_padding_uses_the_same_background_as_its_text() {
        let target = [(
            b"a.py".as_slice(),
            b"def f(): pass\ndef g(): f()\n".as_slice(),
        )];
        let fixture = session(&[], &target);
        let mode = mode();
        let mut app = state::App::new(&fixture, &mode, 1);
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        for _ in 0..2 {
            terminal.draw(|frame| render::render(&app, frame)).unwrap();
            let component = selected_component(&app, app.selected_node.as_ref().unwrap());
            let title_y = (6 + component.y - app.pan_y) as u16;
            let padding_start =
                (27 + component.x - app.pan_x) as u16 + component.title.len() as u16;
            let right = (26 + component.x + component.width - 1 - app.pan_x) as u16;
            assert!(padding_start < right);
            for x in padding_start..right {
                let cell = &terminal.backend().buffer()[(x, title_y)];
                assert_eq!(cell.symbol(), " ");
                assert_eq!(cell.fg, Color::Green);
                assert!(cell.modifier.contains(Modifier::REVERSED));
            }
            app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        terminal.draw(|frame| render::render(&app, frame)).unwrap();
        let former = app
            .layout
            .components
            .iter()
            .find(|component| component.title == "a.py:f")
            .unwrap();
        let x = (27 + former.x - app.pan_x) as u16 + former.title.len() as u16;
        let y = (6 + former.y - app.pan_y) as u16;
        let cell = &terminal.backend().buffer()[(x, y)];
        assert_eq!(cell.symbol(), " ");
        assert_eq!(cell.fg, Color::Green);
        assert!(!cell.modifier.contains(Modifier::REVERSED));
        let selected = selected_component(&app, app.selected_node.as_ref().unwrap());
        let x = (27 + selected.x - app.pan_x) as u16 + selected.title.len() as u16;
        let y = (6 + selected.y - app.pan_y) as u16;
        assert!(
            terminal.backend().buffer()[(x, y)]
                .modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn shared_callers_are_drawn_once_with_all_connectors() {
        let target = [(
            b"a.py".as_slice(),
            b"def root(): pass\ndef a(): root()\ndef b(): root()\ndef shared(): a(); b()\n"
                .as_slice(),
        )];
        let fixture = session(&[], &target);
        let mode = mode();
        let mut app = state::App::new(&fixture, &mode, 2);
        let root_index = app
            .roots
            .iter()
            .position(|root| app.name_for_root(root) == "root")
            .unwrap();
        for _ in 0..root_index {
            app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        }
        assert_eq!(app.layout.components.len(), 4);
        assert_eq!(app.layout.edges.len(), 4);
        assert_eq!(app.layout.order.len(), 4);
        app.pan_x = 0;
        app.pan_y = 0;
        let text = screen(&app, 180, 30);
        assert!(text.contains("Graph: caller -> callee"));
        let graph_at = |text: &str, x: usize, y: usize| {
            text.lines()
                .nth(y + 5)
                .unwrap()
                .chars()
                .nth(x + 26)
                .unwrap()
        };
        let shared = app
            .layout
            .components
            .iter()
            .find(|c| c.title == "a.py:shared")
            .unwrap();
        let first_lane = shared.x + shared.width + 3;
        let middle = app
            .layout
            .components
            .iter()
            .find(|c| c.title == "a.py:a")
            .unwrap();
        let second_lane = middle.x + middle.width + 3;
        let lower = app
            .layout
            .components
            .iter()
            .find(|c| c.title == "a.py:b")
            .unwrap();
        assert_eq!(
            graph_at(&text, first_lane as usize, (shared.y + 1) as usize),
            '┬'
        );
        assert_eq!(
            graph_at(&text, first_lane as usize, (lower.y + 1) as usize),
            '└'
        );
        assert_eq!(
            graph_at(&text, second_lane as usize, (middle.y + 1) as usize),
            '┬'
        );
        assert_eq!(
            graph_at(&text, second_lane as usize, (lower.y + 1) as usize),
            '┘'
        );
        assert_eq!(
            graph_at(&text, (middle.x - 1) as usize, (middle.y + 1) as usize),
            '→'
        );
        assert_eq!(
            graph_at(&text, (lower.x - 1) as usize, (lower.y + 1) as usize),
            '→'
        );
        let root = app
            .layout
            .components
            .iter()
            .find(|c| c.title == "a.py:root")
            .unwrap();
        assert_eq!(
            graph_at(&text, (root.x - 1) as usize, (root.y + 1) as usize),
            '→'
        );
        assert_eq!(
            text.lines()
                .skip(5)
                .take(23)
                .flat_map(|line| line.chars().skip(26).take(122).collect::<Vec<_>>())
                .filter(|&ch| ch == '→')
                .count(),
            3
        );
        for component in &app.layout.components {
            let (x, y) = (component.x as usize, component.y as usize);
            assert_eq!(graph_at(&text, x, y), '┌');
            assert_eq!(graph_at(&text, x + component.width as usize - 1, y), '┐');
            assert_eq!(graph_at(&text, x, y + component.height as usize - 1), '└');
            assert_eq!(
                graph_at(
                    &text,
                    x + component.width as usize - 1,
                    y + component.height as usize - 1
                ),
                '┘'
            );
            assert_eq!(node_line(&app, &text, component, 1), component.title);
        }
        assert!(text.contains("shared"));
        assert!(text.contains("root"));

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        for _ in 0..2 {
            app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
        }
        assert!(app.pan_x > 0);
        let panned = screen(&app, 180, 30);
        assert!(
            panned.contains(&selected_component(&app, app.selected_node.as_ref().unwrap()).title)
        );
        assert!(panned.contains('─'));
        assert!(panned.contains('→'));
        assert_eq!(panned.lines().nth(6).unwrap().chars().nth(25), Some('│'));
    }
    #[test]
    fn expanded_node_reflows_same_column_and_follows_graph_selection() {
        let target = [(
            b"a.py".as_slice(),
            b"def root(): pass\ndef a(): root()\ndef b(): root()\ndef shared(): a(); b()\n"
                .as_slice(),
        )];
        let fixture = session(&[], &target);
        let mode = mode();
        let mut app = state::App::new(&fixture, &mode, 2);
        let root_index = app
            .roots
            .iter()
            .position(|root| app.name_for_root(root) == "root")
            .unwrap();
        for _ in 0..root_index {
            app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        for _ in 0..2 {
            app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        }
        let first = selected_component(&app, app.selected_node.as_ref().unwrap());
        assert_eq!(first.title, "a.py:a");
        let other_y = app
            .layout
            .components
            .iter()
            .find(|component| component.title == "a.py:b")
            .unwrap()
            .y;
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        let first = selected_component(&app, app.selected_node.as_ref().unwrap());
        let other = app
            .layout
            .components
            .iter()
            .find(|component| component.title == "a.py:b")
            .unwrap();
        assert!(first.height > 5);
        assert!(other.y > other_y);
        assert!(other.y >= first.y + first.height + 3);

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        let second = selected_component(&app, app.selected_node.as_ref().unwrap());
        assert_eq!(second.title, "a.py:b");
        assert!(second.height > 5);
        assert_eq!(
            app.layout
                .components
                .iter()
                .find(|component| component.title == "a.py:a")
                .unwrap()
                .height,
            5
        );
    }

    #[test]
    fn rust_changed_and_deleted_roots_render_captured_source_and_callers() {
        let manifest = (
            b"Cargo.toml".as_slice(),
            b"[package]\nname='tui-rust'\nversion='0.1.0'\n".as_slice(),
        );
        let before = b"pub fn rust_root() -> i32 { 1 }\npub fn gone() {}\npub fn caller() -> i32 { gone(); rust_root() }\n";
        let after = b"pub fn rust_root() -> i32 { 2 }\npub fn caller() -> i32 { rust_root() }\n";
        let fixture = session(
            &[manifest, (b"src/lib.rs", before)],
            &[manifest, (b"src/lib.rs", after)],
        );
        let mode = mode();
        let mut app = state::App::new(&fixture, &mode, 1);
        let changed = app
            .roots
            .iter()
            .position(|root| app.name_for_root(root).ends_with("::rust_root"))
            .unwrap();
        for _ in 0..changed {
            app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        let selected = app.selected_node.as_ref().unwrap();
        let root = app
            .layout
            .components
            .iter()
            .find(|c| c.members.contains(selected))
            .unwrap();
        assert_eq!(root.title, "lib.rs:rust_root");
        assert!(root.width >= root.title.chars().count() as i32 + 2);
        let changed_text = screen(&app, 180, 40);
        assert_eq!(node_line(&app, &changed_text, root, 1), "lib.rs:rust_root");
        assert!(changed_text.contains("Source:"));
        assert!(changed_text.contains("Call evidence:"));
        assert!(
            !root
                .extra
                .iter()
                .any(|line| line.contains("Cargo.toml:bin:"))
        );
        assert!(changed_text.contains("src/lib.rs"));
        assert_eq!(app.layout.order.len(), 2);
        let deleted = app
            .roots
            .iter()
            .position(|root| app.name_for_root(root).ends_with("::gone"))
            .unwrap();
        while app.root_index > deleted {
            app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        }
        while app.root_index < deleted {
            app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        }
        let deleted_text = screen(&app, 120, 30);
        assert!(deleted_text.contains("[deleted] base"));
        assert!(deleted_text.contains("gone"));
        assert_eq!(app.layout.order.len(), 2);
    }

    #[test]
    fn long_rust_function_name_fits_box_and_is_reachable_by_panning() {
        let name = format!("function_{}", "identifying_segment_".repeat(8));
        let before = format!("pub fn {name}() -> i32 {{ 1 }}\n");
        let after = format!("pub fn {name}() -> i32 {{ 2 }}\n");
        let manifest = b"[package]\nname='long-title'\nversion='0.1.0'\n";
        let fixture = session(
            &[
                (b"Cargo.toml", manifest),
                (b"src/lib.rs", before.as_bytes()),
            ],
            &[(b"Cargo.toml", manifest), (b"src/lib.rs", after.as_bytes())],
        );
        let mode = mode();
        let mut app = state::App::new(&fixture, &mode, 1);
        let component = selected_component(&app, app.selected_node.as_ref().unwrap());
        assert_eq!(component.title, format!("lib.rs:{name}"));
        assert!(component.width >= component.title.len() as i32 + 2);
        assert!(component.width > 93);
        app.pan_x = i32::MAX;
        app.clamp(Rect::new(0, 0, 93, 22));
        let component = selected_component(&app, app.selected_node.as_ref().unwrap());
        let text = screen(&app, 120, 30);
        assert!(text.contains(&name[name.len() - 20..]));
        let right = (26 + component.x + component.width - 1 - app.pan_x) as usize;
        assert_eq!(text.lines().nth(6).unwrap().chars().nth(right), Some('│'));
    }

    #[test]
    fn repository_control_characters_are_not_rendered_as_terminal_codes() {
        assert_eq!(state::safe_text("name\u{1b}[31m\tend"), "name?[31m?end");
        let target = [(b"a.py".as_slice(), b"def f(): pass #\x1b[31m\n".as_slice())];
        let fixture = session(&[], &target);
        let mode = mode();
        let app = state::App::new(&fixture, &mode, 1);
        let text = screen(&app, 120, 30);
        assert!(!text.contains('\u{1b}'));
    }
}
