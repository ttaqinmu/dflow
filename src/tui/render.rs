use super::layout::BOX_WIDTH;
use super::state::{App, Focus, display_path, kind, role, safe_text};
use crate::cli::CompareMode;
use crate::git::{CapturedSnapshot, SnapshotRole};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

fn truncate(value: &str, width: usize) -> String {
    value
        .chars()
        .take(width)
        .map(|ch| {
            if ch.is_ascii_graphic() || ch == ' ' {
                ch
            } else {
                '?'
            }
        })
        .collect()
}
fn mode_label(mode: &CompareMode) -> String {
    match mode {
        CompareMode::Local(args) => format!(
            "local {}",
            if args.staged {
                "staged"
            } else if args.unstaged {
                "unstaged"
            } else {
                "combined"
            }
        ),
        CompareMode::Branch(args) => format!(
            "branch {} -> {}",
            safe_text(&args.base),
            safe_text(&args.target)
        ),
    }
}
fn snapshot_label(snapshot: &CapturedSnapshot) -> String {
    let identity = snapshot
        .commit_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| snapshot.content_id.clone());
    format!(
        "{} {:?} {}",
        role(snapshot.role),
        snapshot.kind,
        truncate(&identity, 8)
    )
}
fn panel<'a>(title: &'a str, focused: bool) -> Block<'a> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(if focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        })
}
fn put(buf: &mut Buffer, area: Rect, x: i32, y: i32, pan: (i32, i32), symbol: &str, style: Style) {
    let sx = x - pan.0;
    let sy = y - pan.1;
    if sx >= 0 && sy >= 0 && sx < i32::from(area.width) && sy < i32::from(area.height) {
        buf[(area.x + sx as u16, area.y + sy as u16)]
            .set_symbol(symbol)
            .set_style(style);
    }
}
fn draw_graph(app: &App<'_>, buf: &mut Buffer, area: Rect) {
    let normal = Style::default();
    for (from, to) in &app.layout.edges {
        let caller = &app.layout.components[*from];
        let callee = &app.layout.components[*to];
        let sx = caller.x + BOX_WIDTH;
        let sy = caller.y + 1;
        let tx = callee.x - 1;
        let ty = callee.y + 1;
        let lane = sx + 3;
        for x in sx..=lane {
            put(buf, area, x, sy, (app.pan_x, app.pan_y), "-", normal);
        }
        for y in sy.min(ty)..=sy.max(ty) {
            put(buf, area, lane, y, (app.pan_x, app.pan_y), "|", normal);
        }
        for x in lane..=tx {
            put(buf, area, x, ty, (app.pan_x, app.pan_y), "-", normal);
        }
        put(buf, area, lane, sy, (app.pan_x, app.pan_y), "+", normal);
        put(buf, area, lane, ty, (app.pan_x, app.pan_y), "+", normal);
        put(buf, area, tx, ty, (app.pan_x, app.pan_y), ">", normal);
    }
    for component in &app.layout.components {
        let selected = app
            .selected_node
            .as_ref()
            .is_some_and(|id| component.members.contains(id));
        let style = if selected {
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            normal
        };
        for x in component.x..component.x + BOX_WIDTH {
            let edge = if x == component.x || x == component.x + BOX_WIDTH - 1 {
                "+"
            } else {
                "-"
            };
            put(
                buf,
                area,
                x,
                component.y,
                (app.pan_x, app.pan_y),
                edge,
                style,
            );
            put(
                buf,
                area,
                x,
                component.y + 2,
                (app.pan_x, app.pan_y),
                edge,
                style,
            );
        }
        put(
            buf,
            area,
            component.x,
            component.y + 1,
            (app.pan_x, app.pan_y),
            "|",
            style,
        );
        put(
            buf,
            area,
            component.x + BOX_WIDTH - 1,
            component.y + 1,
            (app.pan_x, app.pan_y),
            "|",
            style,
        );
        let name = if component.cycle {
            format!(
                "cycle({}) {}",
                component.members.len(),
                app.name(&component.members[0])
            )
        } else {
            app.name(&component.members[0])
        };
        let root_kind = component
            .members
            .iter()
            .map(|id| app.root_kind(id))
            .find(|value| !value.is_empty())
            .unwrap_or("");
        let snapshot = app.root().map(|r| role(r.role)).unwrap_or("target");
        let label = truncate(
            &format!("[{}] {} {}", snapshot, root_kind, name),
            (BOX_WIDTH - 2) as usize,
        );
        for (offset, ch) in label.chars().enumerate() {
            put(
                buf,
                area,
                component.x + 1 + offset as i32,
                component.y + 1,
                (app.pan_x, app.pan_y),
                &ch.to_string(),
                style,
            );
        }
    }
}
fn details(app: &App<'_>) -> Vec<Line<'static>> {
    let Some(id) = app.selected_node.as_ref() else {
        return vec![Line::from("No changed symbols to inspect")];
    };
    let Some(graph) = app.graph() else {
        return vec![];
    };
    let node = &graph.graph[graph.ids[id]];
    let mut lines = vec![Line::from(format!(
        "{} [{}]",
        app.name(id),
        role(graph.role)
    ))];
    if !app.root_kind(id).is_empty() {
        lines.push(Line::from(format!("Change: {}", app.root_kind(id))));
    }
    if let Some(symbol) = &node.symbol {
        lines.push(Line::from(format!("Kind: {:?}", symbol.kind)));
        lines.push(Line::from(format!(
            "Path: {}",
            display_path(&symbol.source.path)
        )));
        lines.push(Line::from(format!(
            "Range: {}:{}-{}:{}",
            symbol.source.range.start.line,
            symbol.source.range.start.byte_column,
            symbol.source.range.end.line,
            symbol.source.range.end.byte_column
        )));
        let captured = match graph.role {
            SnapshotRole::Base => &app.session.capture.pair.base,
            SnapshotRole::Target => &app.session.capture.pair.target,
        };
        if let Some(file) = captured.files.get(&symbol.source.path) {
            lines.push(Line::from("Source:"));
            let text = String::from_utf8_lossy(&file.bytes);
            for (line_no, content) in text
                .lines()
                .enumerate()
                .skip(symbol.source.range.start.line.saturating_sub(2))
                .take(5)
            {
                lines.push(Line::from(format!(
                    "{:>4} {}",
                    line_no + 1,
                    safe_text(content)
                )));
            }
        }
    } else {
        lines.push(Line::from(format!(
            "Module: {}",
            display_path(node.module_path.as_deref().unwrap_or_default())
        )));
    }
    let mut related = 0;
    for call in &graph.calls {
        if call.caller == *id || call.target.as_deref() == Some(id) {
            if related == 0 {
                lines.push(Line::from("Call evidence:"));
            }
            let direction = if call.caller == *id {
                "calls"
            } else {
                "called by"
            };
            let outcome = call
                .target
                .as_ref()
                .map(|target| app.name(target))
                .unwrap_or_else(|| format!("unresolved ({:?})", call.reason));
            lines.push(Line::from(format!(
                "{} {} -> {} @ {}:{}",
                direction,
                safe_text(&call.expression),
                outcome,
                display_path(&call.source.path),
                call.source.range.start.line
            )));
            related += 1;
        }
    }
    if related == 0 {
        lines.push(Line::from("No call-site evidence"));
    }
    if let Some(component) = app
        .layout
        .components
        .iter()
        .find(|c| c.members.contains(id))
        .filter(|component| component.cycle)
    {
        lines.push(Line::from("Cycle members:"));
        for member in &component.members {
            lines.push(Line::from(format!("- {}", app.name(member))));
        }
    }
    lines
}
fn diagnostics(app: &App<'_>, lines: &mut Vec<Line<'static>>) {
    let capture = &app.session.capture.diagnostics;
    let extracted = app
        .session
        .base
        .diagnostics
        .iter()
        .chain(&app.session.target.diagnostics);
    let changed = &app.session.changes.diagnostics;
    if capture.is_empty()
        && app.session.base.diagnostics.is_empty()
        && app.session.target.diagnostics.is_empty()
        && changed.is_empty()
    {
        return;
    }
    lines.push(Line::from("Diagnostics:"));
    for item in capture {
        lines.push(Line::from(format!(
            "Git {}: {}",
            display_path(&item.path),
            safe_text(&item.message)
        )));
    }
    for item in extracted.chain(changed) {
        lines.push(Line::from(format!(
            "{:?} [{}] {}: {}",
            item.kind,
            role(item.snapshot),
            display_path(&item.path),
            safe_text(&item.message)
        )));
    }
}
pub fn render(app: &App<'_>, frame: &mut ratatui::Frame<'_>) {
    let area = frame.area();
    if app.help {
        let help = [
            "DFlow keyboard help",
            "",
            "Tab: cycle panels",
            "Arrows / j,k: select root or graph node",
            "Shift+arrows: pan graph",
            "Page Up/Down: scroll selected panel",
            "+/-: caller depth",
            "?: toggle help",
            "q / Ctrl-C: quit",
            "",
            "Arrows point caller -> callee. Deleted roots use base evidence.",
        ];
        frame.render_widget(
            Paragraph::new(help.join("\n"))
                .block(panel("Help", true))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if area.width < 100 || area.height < 16 {
        frame.render_widget(Paragraph::new("Terminal too small for graph. Resize to at least 100x16. Press q to quit or ? for help.").block(panel("DFlow", true)).wrap(Wrap { trim: false }), area);
        return;
    }
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(area);
    let snapshot = app.root().map(|r| role(r.role)).unwrap_or("target");
    let status = if app.session.complete() {
        "complete"
    } else {
        "PARTIAL analysis"
    };
    let identity = format!(
        "{} | {}",
        snapshot_label(&app.session.capture.pair.base),
        snapshot_label(&app.session.capture.pair.target)
    );
    frame.render_widget(
        Paragraph::new(format!(
            "{} | focused: {} | depth: {} | {}\n{}",
            mode_label(app.mode),
            snapshot,
            app.depth,
            status,
            identity
        ))
        .block(panel("DFlow", false)),
        vertical[0],
    );
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(25),
            Constraint::Min(25),
            Constraint::Length(31),
        ])
        .split(vertical[1]);
    let list_height = horizontal[0].height.saturating_sub(2) as usize;
    let from = app.root_index.saturating_sub(list_height.saturating_sub(1));
    let mut entries = Vec::new();
    for (index, root) in app.roots.iter().enumerate().skip(from).take(list_height) {
        entries.push(Line::from(format!(
            "{} [{}] {} {}",
            if index == app.root_index { ">" } else { " " },
            kind(root.kind),
            role(root.role),
            truncate(&app.name_for_root(root), 12)
        )));
    }
    if entries.is_empty() {
        entries.push(Line::from("No changed symbols"));
    }
    frame.render_widget(
        Paragraph::new(Text::from(entries)).block(panel("Changes", app.focus == Focus::Changes)),
        horizontal[0],
    );
    let block = panel("Graph: caller -> callee", app.focus == Focus::Graph);
    let inner = block.inner(horizontal[1]);
    frame.render_widget(block, horizontal[1]);
    if app.layout.components.is_empty() {
        frame.render_widget(Paragraph::new("No impact graph to display"), inner);
    } else {
        draw_graph(app, frame.buffer_mut(), inner);
    }
    let mut detail_lines = details(app);
    diagnostics(app, &mut detail_lines);
    let visible_details = horizontal[2].height.saturating_sub(2) as usize;
    let detail_width = horizontal[2].width.saturating_sub(2).max(1) as usize;
    let wrapped_lines: usize = detail_lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(detail_width))
        .sum();
    let scroll = app.details_scroll.min(
        wrapped_lines
            .saturating_sub(visible_details)
            .min(u16::MAX as usize) as u16,
    );
    frame.render_widget(
        Paragraph::new(Text::from(detail_lines))
            .block(panel("Details", app.focus == Focus::Details))
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        horizontal[2],
    );
    frame.render_widget(
        Paragraph::new(
            "Tab panels  arrows/j/k select  Shift+arrows pan  +/- depth  ? help  q quit",
        ),
        vertical[2],
    );
}
