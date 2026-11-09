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
                size.width.saturating_sub(2),
                size.height.saturating_sub(render::HEADER_HEIGHT + 2),
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
        let x = (1 + component.x - app.pan_x) as usize;
        let y = (6 + component.y - app.pan_y + row) as usize;
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
    fn select_title(app: &mut state::App<'_>, title: &str) {
        let id = app
            .layout
            .components
            .iter()
            .find(|component| component.title == title)
            .unwrap_or_else(|| panic!("missing graph node {title}"))
            .members[0]
            .clone();
        let destination = app
            .layout
            .order
            .iter()
            .position(|member| member == &id)
            .unwrap();
        while app
            .layout
            .order
            .iter()
            .position(|member| Some(member) == app.selected_node.as_ref())
            .unwrap()
            < destination
        {
            app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        }
        while app
            .layout
            .order
            .iter()
            .position(|member| Some(member) == app.selected_node.as_ref())
            .unwrap()
            > destination
        {
            app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        }
        assert_eq!(app.selected_node.as_ref(), Some(&id));
    }
    fn graph_char(text: &str, x: i32, y: i32) -> char {
        text.lines()
            .nth((6 + y) as usize)
            .unwrap()
            .chars()
            .nth((1 + x) as usize)
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
        assert!(!text.contains("Call evidence ["));
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
        assert!(root.description.starts_with("Location: a.py:"));
        assert!(root.width >= root.title.chars().count() as i32 + 2);
        assert_eq!(root.height, 5);
        assert!(text.lines().skip(6).any(|line| line.contains('→')));
        let mut app = app;
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        let text = screen(&app, 120, 30);
        assert!(text.contains("Source [target]:"));
        assert!(text.contains("Call evidence [target]:"));
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
        assert!(screen(&cycle_app, 120, 30).contains("Cycle members (combined view):"));
    }
    #[test]
    fn module_caller_box_shows_its_name_without_type_prefix() {
        let target = [(b"a.py".as_slice(), b"def f(): pass\nf()\n".as_slice())];
        let fixture = session(&[], &target);
        let mode = mode();
        let mut app = state::App::new(&fixture, &mode, 1);
        app.pan_x = 0;
        app.pan_y = 0;
        let module = app
            .layout
            .components
            .iter()
            .find(|component| component.title == "a.py")
            .unwrap();
        assert_eq!(node_line(&app, &screen(&app, 120, 30), module, 1), "a.py");
    }

    #[test]
    fn reducer_changes_depth_selection_and_exits() {
        let base = [(
            b"a.py".as_slice(),
            b"def f(): return 1\ndef g(): return f()\n".as_slice(),
        )];
        let target = [(
            b"a.py".as_slice(),
            b"def f(): return 2\ndef g(): return f()\n".as_slice(),
        )];
        let session = session(&base, &target);
        let mode = mode();
        let mut app = state::App::new(&session, &mode, 0);
        assert_eq!(app.changed_roots.len(), 1);
        assert_eq!(app.layout.order.len(), 1);
        let root = app.selected_node.clone();
        app.handle_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
        assert_eq!(app.depth, 1);
        assert_eq!(app.layout.order.len(), 2);
        assert_eq!(app.selected_node, root);
        app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_ne!(app.selected_node, root);
        app.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));
        assert_eq!(app.depth, 0);
        assert_eq!(app.selected_node, root);
        assert_eq!(app.layout.order.len(), 1);
        let before = app.selected_node.clone();
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.selected_node, before);
        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(screen(&app, 120, 30).contains("DFlow keyboard help"));
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)));
    }
    #[test]
    fn expanding_selected_node_grows_box_without_overlap() {
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
        assert!(!hidden.contains("Source ["));
        assert_eq!(original_height, 5);
        assert!(original_width >= 26);
        assert_eq!(node_line(&app, &hidden, original, 3), original.description);
        assert_eq!(hidden.lines().nth(6).unwrap().chars().nth(179), Some('│'));

        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        let expanded = selected_component(&app, &selected);
        assert!(expanded.height > original_height);
        assert!(
            expanded
                .extra
                .iter()
                .any(|line| line.contains("Source [target]:"))
        );
        let shown = screen(&app, 180, 40);
        assert!(shown.contains("Source [target]:"));
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
        assert_eq!(app.selected_node.as_ref(), Some(&selected));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(selected_component(&app, &selected).height, original_height);
        assert!(selected_component(&app, &selected).extra.is_empty());
        assert_eq!(app.selected_node.as_ref(), Some(&selected));
        assert!(!screen(&app, 180, 40).contains("Source ["));
    }

    #[test]
    fn empty_partial_and_small_terminal_are_explicit() {
        let mode = mode();
        let empty = session(&[], &[]);
        let app = state::App::new(&empty, &mode, 1);
        assert!(screen(&app, 120, 30).contains("No changed symbols"));
        assert!(screen(&app, 50, 10).contains("Terminal too small"));
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
        select_title(&mut app, "a.py:removed");
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
        assert!(text.contains("Source [base]:"));
        assert!(text.contains("Change: deleted"));
        assert!(text.contains("Call evidence [base]:"));
        assert!(text.contains("called by"));
        assert!(app.layout.order.len() >= 2);
    }
    #[test]
    fn all_change_kinds_use_individual_graph_colors_and_snapshot_evidence() {
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
        let mut app = state::App::new(&fixture, &mode, 1);
        assert!(screen(&app, 180, 50).contains("base WorkingTree"));
        assert!(screen(&app, 180, 50).contains("target WorkingTree"));
        assert!(!screen(&app, 180, 50).contains("Changes"));
        assert!(!screen(&app, 180, 50).contains("focused:"));
        for (title, kind, color) in [
            ("a.py:fresh", SymbolChangeKind::Added, Color::Green),
            ("a.py:changed", SymbolChangeKind::Modified, Color::Yellow),
            ("a.py:gone", SymbolChangeKind::Deleted, Color::Red),
        ] {
            select_title(&mut app, title);
            app.pan_x = 0;
            app.pan_y = 0;
            let component = app
                .layout
                .components
                .iter()
                .find(|c| c.title == title)
                .unwrap();
            assert!(app.nodes[&component.members[0]].changes.contains(&kind));
            let mut terminal = Terminal::new(TestBackend::new(180, 50)).unwrap();
            terminal.draw(|frame| render::render(&app, frame)).unwrap();
            let box_cell =
                &terminal.backend().buffer()[((1 + component.x) as u16, (6 + component.y) as u16)];
            assert_eq!(box_cell.symbol(), "┌");
            assert_eq!(box_cell.fg, color);
            assert!(box_cell.modifier.contains(Modifier::REVERSED));
            app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
            let details = selected_component(&app, app.selected_node.as_ref().unwrap())
                .extra
                .join("\n");
            assert!(details.contains(&format!("Change: {}", state::kind(kind))));
            if kind == SymbolChangeKind::Modified {
                assert!(details.contains("Source [base]:"));
                assert!(details.contains("Source [target]:"));
            } else {
                let snapshot = if kind == SymbolChangeKind::Deleted {
                    "base"
                } else {
                    "target"
                };
                assert!(details.contains(&format!("Source [{snapshot}]:")));
            }
            app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
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
            let title_y = (7 + component.y - app.pan_y) as u16;
            let padding_start = (2 + component.x - app.pan_x) as u16 + component.title.len() as u16;
            let right = (1 + component.x + component.width - 1 - app.pan_x) as u16;
            assert!(padding_start < right);
            for x in padding_start..right {
                let cell = &terminal.backend().buffer()[(x, title_y)];
                assert_eq!(cell.symbol(), " ");
                assert_eq!(cell.fg, Color::Green);
                assert!(cell.modifier.contains(Modifier::REVERSED));
            }
            app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        }
        select_title(&mut app, "a.py:g");
        terminal.draw(|frame| render::render(&app, frame)).unwrap();
        let former = app
            .layout
            .components
            .iter()
            .find(|component| component.title == "a.py:f")
            .unwrap();
        let x = (2 + former.x - app.pan_x) as u16 + former.title.len() as u16;
        let y = (7 + former.y - app.pan_y) as u16;
        let cell = &terminal.backend().buffer()[(x, y)];
        assert_eq!(cell.symbol(), " ");
        assert_eq!(cell.fg, Color::Green);
        assert!(!cell.modifier.contains(Modifier::REVERSED));
        let selected = selected_component(&app, app.selected_node.as_ref().unwrap());
        let x = (2 + selected.x - app.pan_x) as u16 + selected.title.len() as u16;
        let y = (7 + selected.y - app.pan_y) as u16;
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
        select_title(&mut app, "a.py:root");
        assert_eq!(app.layout.components.len(), 4);
        assert_eq!(app.layout.edges.len(), 4);
        assert_eq!(app.layout.order.len(), 4);
        app.pan_x = 0;
        app.pan_y = 0;
        let text = screen(&app, 180, 30);
        assert!(text.contains("Graph: caller -> callee"));
        let graph_at = |text: &str, x: usize, y: usize| graph_char(text, x as i32, y as i32);
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
        assert_eq!(panned.lines().nth(6).unwrap().chars().next(), Some('│'));
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
        select_title(&mut app, "a.py:a");
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
        select_title(&mut app, "lib.rs:rust_root");
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
        assert!(root.description.starts_with("Location: src/lib.rs:"));
        assert!(changed_text.contains("Source [target]:"));
        assert!(changed_text.contains("Call evidence [target]:"));
        assert!(
            !root
                .extra
                .iter()
                .any(|line| line.contains("Cargo.toml:bin:"))
        );
        assert!(changed_text.contains("src/lib.rs"));
        assert!(app.layout.order.len() >= 3);
        select_title(&mut app, "lib.rs:gone");
        let deleted = selected_component(&app, app.selected_node.as_ref().unwrap());
        assert!(deleted.extra.iter().any(|line| line == "Change: deleted"));
        assert!(deleted.extra.iter().any(|line| line == "Source [base]:"));
        assert!(
            deleted
                .extra
                .iter()
                .any(|line| line == "Call evidence [base]:")
        );
        assert!(!deleted.extra.iter().any(|line| line == "Source [target]:"));
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
        assert!(component.width > 118);
        app.pan_x = i32::MAX;
        app.clamp(Rect::new(0, 0, 118, 23));
        let component = selected_component(&app, app.selected_node.as_ref().unwrap());
        let text = screen(&app, 120, 30);
        assert!(text.contains(&name[name.len() - 20..]));
        let right = (1 + component.x + component.width - 1 - app.pan_x) as usize;
        assert_eq!(text.lines().nth(7).unwrap().chars().nth(right), Some('│'));
    }

    #[test]
    fn unrelated_changed_roots_are_visible_at_zero_depth_as_separate_islands() {
        let fixture = session(
            &[
                (b"a.py", b"def alpha(): return 1\n"),
                (b"b.py", b"def beta(): return 1\n"),
            ],
            &[
                (b"a.py", b"def alpha(): return 2\n"),
                (b"b.py", b"def beta(): return 2\n"),
            ],
        );
        let mode = mode();
        let mut app = state::App::new(&fixture, &mode, 0);
        assert_eq!(app.changed_roots.len(), 2);
        assert_eq!(app.layout.components.len(), 2);
        assert!(app.layout.edges.is_empty());
        let a = app
            .layout
            .components
            .iter()
            .find(|c| c.title == "a.py:alpha")
            .unwrap();
        let b = app
            .layout
            .components
            .iter()
            .find(|c| c.title == "b.py:beta")
            .unwrap();
        assert_ne!(a.cluster, b.cluster);
        assert_eq!(a.x, b.x);
        assert!(b.y >= a.y + a.height + 3);
        app.pan_x = 0;
        app.pan_y = 0;
        let visible = screen(&app, 120, 30);
        assert_eq!(node_line(&app, &visible, a, 1), a.title);
        assert_eq!(node_line(&app, &visible, b, 1), b.title);
        let selected = app.selected_node.clone();
        app.handle_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
        assert_eq!(app.depth, 1);
        assert_eq!(app.selected_node, selected);
        app.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));
        assert_eq!(app.depth, 0);
        assert_eq!(app.selected_node, selected);
    }

    #[test]
    fn independent_caller_chains_stay_separate_and_reuse_unchanged_nodes() {
        let fixture = session(
            &[
                (
                    b"a.py",
                    b"def alpha(): return 1\ndef caller_a(): return alpha()\n",
                ),
                (
                    b"b.py",
                    b"def beta(): return 1\ndef caller_b(): return beta()\n",
                ),
            ],
            &[
                (
                    b"a.py",
                    b"def alpha(): return 2\ndef caller_a(): return alpha()\n",
                ),
                (
                    b"b.py",
                    b"def beta(): return 2\ndef caller_b(): return beta()\n",
                ),
            ],
        );
        let mode = mode();
        let app = state::App::new(&fixture, &mode, 1);
        assert_eq!(app.changed_roots.len(), 2);
        assert_eq!(app.layout.components.len(), 4);
        assert_eq!(app.impact.edges.len(), 2);
        let find = |title| {
            app.layout
                .components
                .iter()
                .find(|c| c.title == title)
                .unwrap()
        };
        for (root, caller) in [
            ("a.py:alpha", "a.py:caller_a"),
            ("b.py:beta", "b.py:caller_b"),
        ] {
            let root = find(root);
            let caller = find(caller);
            assert_eq!(root.cluster, caller.cluster);
            assert!(caller.x < root.x);
            assert!(app.nodes[&caller.members[0]].changes.is_empty());
            assert!(app.nodes[&caller.members[0]].base.is_some());
            assert!(app.nodes[&caller.members[0]].target.is_some());
            let contributors = &app.edges[&(caller.members[0].clone(), root.members[0].clone())];
            assert_eq!(contributors.len(), 1);
            assert_eq!(contributors[0].0, SnapshotRole::Target);
        }
        assert_ne!(find("a.py:alpha").cluster, find("b.py:beta").cluster);
    }

    #[test]
    fn shared_unchanged_caller_links_both_changed_roots_without_duplicate_edges() {
        let before =
            b"def alpha(): return 1\ndef beta(): return 1\ndef shared(): return alpha() + beta()\n";
        let after =
            b"def alpha(): return 2\ndef beta(): return 2\ndef shared(): return alpha() + beta()\n";
        let fixture = session(&[(b"a.py", before)], &[(b"a.py", after)]);
        let mode = mode();
        let mut app = state::App::new(&fixture, &mode, 0);
        assert_eq!(app.layout.components.len(), 2);
        assert!(app.impact.edges.is_empty());
        app.handle_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
        assert_eq!(app.layout.components.len(), 3);
        assert_eq!(app.impact.edges.len(), 2);
        let shared = app
            .layout
            .components
            .iter()
            .find(|c| c.title == "a.py:shared")
            .unwrap();
        assert!(app.nodes[&shared.members[0]].changes.is_empty());
        assert!(app.nodes[&shared.members[0]].base.is_some());
        assert!(app.nodes[&shared.members[0]].target.is_some());
        for title in ["a.py:alpha", "a.py:beta"] {
            let root = app
                .layout
                .components
                .iter()
                .find(|c| c.title == title)
                .unwrap();
            assert_eq!(root.cluster, shared.cluster);
            assert!(
                app.impact
                    .edges
                    .contains(&(shared.members[0].clone(), root.members[0].clone()))
            );
        }
        select_title(&mut app, "a.py:shared");
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        let evidence = selected_component(&app, app.selected_node.as_ref().unwrap())
            .extra
            .join("\n");
        assert!(evidence.contains("Source [base]:"));
        assert!(evidence.contains("Source [target]:"));
        assert!(evidence.contains("Call evidence [base]:"));
        assert!(evidence.contains("Call evidence [target]:"));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        app.pan_x = 0;
        app.pan_y = 0;
        let text = screen(&app, 180, 40);
        assert_eq!(text.matches("a.py:shared").count(), 1);
    }

    #[test]
    fn duplicate_definitions_remain_distinct_even_when_titles_match() {
        let fixture = session(
            &[(b"a.py", b"def f(): return 1\ndef f(): return 2\n")],
            &[(b"a.py", b"def f(): return 3\ndef f(): return 4\n")],
        );
        let mode = mode();
        let mut app = state::App::new(&fixture, &mode, 0);
        assert_eq!(app.changed_roots.len(), 4);
        assert_eq!(app.layout.components.len(), 4);
        assert!(app.layout.components.iter().all(|c| c.title == "a.py:f"));
        assert!(app.layout.components.iter().all(|c| c.members.len() == 1));
        let ids: std::collections::BTreeSet<_> = app
            .layout
            .components
            .iter()
            .flat_map(|component| &component.members)
            .collect();
        assert_eq!(ids.len(), 4);
        assert_eq!(
            ids.iter()
                .filter(|id| app.nodes[id.as_str()].base.is_some())
                .count(),
            2
        );
        assert_eq!(
            ids.iter()
                .filter(|id| app.nodes[id.as_str()].target.is_some())
                .count(),
            2
        );
        assert!(
            ids.iter().all(|id| app.nodes[id.as_str()].base.is_none()
                || app.nodes[id.as_str()].target.is_none())
        );
        app.pan_x = 0;
        app.pan_y = 0;
        let text = screen(&app, 120, 60);
        assert_eq!(text.matches("a.py:f").count(), 4);
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
