use super::state::{App, kind, role_name, safe_text};
use crate::analysis::SymbolChangeKind;
use crate::cli::CompareMode;
use crate::git::CapturedSnapshot;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::collections::BTreeMap;

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
        role_name(snapshot.role),
        snapshot.kind,
        truncate(&identity, 8)
    )
}
fn panel(title: &str) -> Block<'_> {
    Block::default().title(title).borders(Borders::ALL)
}
fn change_style(change: SymbolChangeKind) -> Style {
    Style::default().fg(match change {
        SymbolChangeKind::Added => Color::Green,
        SymbolChangeKind::Deleted => Color::Red,
        SymbolChangeKind::Modified => Color::Yellow,
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
fn put_text(
    buf: &mut Buffer,
    area: Rect,
    x: i32,
    y: i32,
    pan: (i32, i32),
    text: &str,
    style: Style,
) {
    for (offset, ch) in text.chars().enumerate() {
        let mut bytes = [0; 4];
        put(
            buf,
            area,
            x + offset as i32,
            y,
            pan,
            ch.encode_utf8(&mut bytes),
            style,
        );
    }
}

const NORTH: u8 = 1;
const EAST: u8 = 2;
const SOUTH: u8 = 4;
const WEST: u8 = 8;

fn connect(lines: &mut BTreeMap<(i32, i32), u8>, from: (i32, i32), to: (i32, i32)) {
    let (mut x, mut y) = from;
    while (x, y) != to {
        let (nx, ny, direction, opposite) = if x != to.0 {
            let step = (to.0 - x).signum();
            (
                x + step,
                y,
                if step > 0 { EAST } else { WEST },
                if step > 0 { WEST } else { EAST },
            )
        } else {
            let step = (to.1 - y).signum();
            (
                x,
                y + step,
                if step > 0 { SOUTH } else { NORTH },
                if step > 0 { NORTH } else { SOUTH },
            )
        };
        *lines.entry((x, y)).or_default() |= direction;
        *lines.entry((nx, ny)).or_default() |= opposite;
        (x, y) = (nx, ny);
    }
}

fn line_glyph(directions: u8) -> &'static str {
    match directions {
        1 | 4 | 5 => "│",
        2 | 8 | 10 => "─",
        3 => "└",  // north + east
        6 => "┌",  // east + south
        12 => "┐", // south + west
        9 => "┘",  // west + north
        7 => "├",  // north + east + south
        13 => "┤", // north + south + west
        14 => "┬", // east + south + west
        11 => "┴", // north + east + west
        15 => "┼",
        _ => unreachable!("connector direction bits must describe a line"),
    }
}

fn draw_graph(app: &App<'_>, buf: &mut Buffer, area: Rect) {
    let normal = Style::default();
    let mut lines = BTreeMap::new();
    let mut arrows = Vec::with_capacity(app.layout.edges.len());
    for (from, to) in &app.layout.edges {
        let caller = &app.layout.components[*from];
        let callee = &app.layout.components[*to];
        let sx = caller.x + caller.width;
        let sy = caller.y + 1;
        let tx = callee.x - 1;
        let ty = callee.y + 1;
        let lane = sx + 3;
        connect(&mut lines, (sx, sy), (lane, sy));
        connect(&mut lines, (lane, sy), (lane, ty));
        connect(&mut lines, (lane, ty), (tx - 1, ty));
        arrows.push((tx, ty));
    }
    for ((x, y), directions) in lines {
        put(
            buf,
            area,
            x,
            y,
            (app.pan_x, app.pan_y),
            line_glyph(directions),
            normal,
        );
    }
    for (x, y) in arrows {
        put(buf, area, x, y, (app.pan_x, app.pan_y), "→", normal);
    }
    for component in &app.layout.components {
        let selected = app
            .selected_node
            .as_ref()
            .is_some_and(|id| component.members.contains(id));
        let kinds: std::collections::BTreeSet<_> = component
            .members
            .iter()
            .flat_map(|id| app.nodes[id].changes.iter().map(|change| kind(*change)))
            .collect();
        let base_style = if kinds.len() > 1 {
            Style::default().fg(Color::Magenta)
        } else {
            component
                .members
                .iter()
                .flat_map(|id| &app.nodes[id].changes)
                .next()
                .copied()
                .map(change_style)
                .unwrap_or(normal)
        };
        let style = if selected {
            base_style.add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            base_style
        };
        let right = component.x + component.width - 1;
        let bottom = component.y + component.height - 1;
        let fill_left = (component.x + 1).max(app.pan_x);
        let fill_right = right.min(app.pan_x + i32::from(area.width));
        let fill_top = (component.y + 1).max(app.pan_y);
        let fill_bottom = bottom.min(app.pan_y + i32::from(area.height));
        for y in fill_top..fill_bottom {
            for x in fill_left..fill_right {
                put(buf, area, x, y, (app.pan_x, app.pan_y), " ", style);
            }
        }
        for x in component.x..=right {
            let (top, lower) = if x == component.x {
                ("┌", "└")
            } else if x == right {
                ("┐", "┘")
            } else {
                ("─", "─")
            };
            put(
                buf,
                area,
                x,
                component.y,
                (app.pan_x, app.pan_y),
                top,
                style,
            );
            put(buf, area, x, bottom, (app.pan_x, app.pan_y), lower, style);
        }
        for y in component.y + 1..bottom {
            put(
                buf,
                area,
                component.x,
                y,
                (app.pan_x, app.pan_y),
                "│",
                style,
            );
            put(buf, area, right, y, (app.pan_x, app.pan_y), "│", style);
        }
        put_text(
            buf,
            area,
            component.x + 1,
            component.y + 1,
            (app.pan_x, app.pan_y),
            &component.title,
            style,
        );
        for x in component.x + 1..right {
            put(
                buf,
                area,
                x,
                component.y + 2,
                (app.pan_x, app.pan_y),
                "─",
                style,
            );
        }
        put_text(
            buf,
            area,
            component.x + 1,
            component.y + 3,
            (app.pan_x, app.pan_y),
            &component.description,
            style,
        );
        if !component.extra.is_empty() {
            for x in component.x + 1..right {
                put(
                    buf,
                    area,
                    x,
                    component.y + 4,
                    (app.pan_x, app.pan_y),
                    "─",
                    style,
                );
            }
            for (row, text) in component.extra.iter().enumerate() {
                put_text(
                    buf,
                    area,
                    component.x + 1,
                    component.y + 5 + row as i32,
                    (app.pan_x, app.pan_y),
                    text,
                    style,
                );
            }
        }
    }
}
pub const HEADER_HEIGHT: u16 = 5;
pub fn render(app: &App<'_>, frame: &mut ratatui::Frame<'_>) {
    let area = frame.area();
    if app.help {
        let help = [
            "DFlow keyboard help",
            "",
            "Arrows / j,k: select graph node",
            "Shift+arrows: pan graph (including wide nodes)",
            "Page Up/Down: pan vertically",
            "+/-: caller depth",
            "d: expand/collapse selected node details",
            "?: toggle help",
            "q / Ctrl-C: quit",
            "",
            "Arrows point caller -> callee. Deleted roots use base evidence.",
        ];
        frame.render_widget(
            Paragraph::new(help.join("\n"))
                .block(panel("Help"))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if area.width < 60 || area.height < 12 {
        frame.render_widget(Paragraph::new("Terminal too small for graph. Resize to at least 60x12. Press q to quit or ? for help.").block(panel("DFlow")).wrap(Wrap { trim: false }), area);
        return;
    }
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(HEADER_HEIGHT), Constraint::Min(5)])
        .split(area);
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
            "{} | depth: {} | {} | {} changed | {} nodes\n{}\nArrows/j/k select  Shift+arrows/Page Up/Down pan  +/- depth  d expand  ? help  q quit",
            mode_label(app.mode), app.depth, status, app.changed_roots.len(), app.layout.order.len(), identity
        )).block(panel("DFlow")), vertical[0]);
    let block = panel("Graph: caller -> callee");
    let inner = block.inner(vertical[1]);
    frame.render_widget(block, vertical[1]);
    if app.layout.components.is_empty() {
        let mut message = vec!["No changed symbols".to_owned()];
        message.extend(app.diagnostic_lines());
        frame.render_widget(
            Paragraph::new(Text::from(message.join("\n"))).wrap(Wrap { trim: false }),
            inner,
        );
    } else {
        draw_graph(app, frame.buffer_mut(), inner);
    }
}
