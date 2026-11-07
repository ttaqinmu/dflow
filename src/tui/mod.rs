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
                size.width.saturating_sub(58),
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
    use crate::cli::{AnalyzeOptions, LocalArgs};
    use crate::git::{
        CaptureResult, CapturedFile, CapturedSnapshot, SnapshotKind, SnapshotPair, SnapshotRole,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
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
        assert!(text.contains("[target] added"));
        assert!(text.contains("Source:"));
        assert!(text.contains("Call evidence:"));
        assert!(
            text.lines()
                .any(|line| line.chars().skip(26).take(63).any(|ch| ch == '>'))
        );
        let target = [(
            b"a.py".as_slice(),
            b"def a(): b()\ndef b(): a()\n".as_slice(),
        )];
        let cycle_session = session(&[], &target);
        let cycle_app = state::App::new(&cycle_session, &mode, 2);
        assert!(
            cycle_app
                .layout
                .components
                .iter()
                .any(|component| component.cycle)
        );
        assert!(screen(&cycle_app, 120, 30).contains("Cycle members:"));
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
    fn empty_partial_and_small_terminal_are_explicit() {
        let mode = mode();
        let empty = session(&[], &[]);
        let app = state::App::new(&empty, &mode, 1);
        assert!(screen(&app, 120, 30).contains("No changed symbols"));
        assert!(screen(&app, 70, 12).contains("Terminal too small"));
        let bad = session(&[], &[(b"bad.py", b"def f(:\n")]);
        let app = state::App::new(&bad, &mode, 1);
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
        let text = screen(&app, 120, 30);
        assert!(text.contains("[base] deleted"));
        assert!(text.contains("Change: deleted"));
        assert!(text.contains("called by"));
        assert_eq!(app.layout.order.len(), 2);
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
        assert!(screen(&app, 120, 30).contains("Graph: caller -> callee"));
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
