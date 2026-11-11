use super::state::{App, kind, role_name, safe_text};
use crate::analysis::SymbolChangeKind;
use crate::cli::CompareMode;
use crate::git::CapturedSnapshot;
use crate::graph::EdgeChangeKind;
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
fn edge_style(change: EdgeChangeKind) -> Style {
    change_style(match change {
        EdgeChangeKind::Added => SymbolChangeKind::Added,
        EdgeChangeKind::Deleted => SymbolChangeKind::Deleted,
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
const ADDED_ROUTE: u8 = 1;
const DELETED_ROUTE: u8 = 2;
const UNCHANGED_ROUTE: u8 = 4;

fn route_style(statuses: u8) -> Style {
    match statuses {
        ADDED_ROUTE => edge_style(EdgeChangeKind::Added),
        DELETED_ROUTE => edge_style(EdgeChangeKind::Deleted),
        _ => Style::default(),
    }
}

fn connect(
    lines: &mut BTreeMap<(i32, i32), (u8, u8)>,
    from: (i32, i32),
    to: (i32, i32),
    status: u8,
) {
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
        let start = lines.entry((x, y)).or_default();
        start.0 |= direction;
        start.1 |= status;
        let end = lines.entry((nx, ny)).or_default();
        end.0 |= opposite;
        end.1 |= status;
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
    for edge in &app.layout.edges {
        let caller = &app.layout.components[edge.from];
        let callee = &app.layout.components[edge.to];
        let sx = caller.x + caller.width;
        let sy = caller.y + 1;
        let tx = callee.x - 1;
        let ty = callee.y + 1 + edge.port as i32;
        let lane = edge.lane_x;
        let status = match edge.status {
            Some(EdgeChangeKind::Added) => ADDED_ROUTE,
            Some(EdgeChangeKind::Deleted) => DELETED_ROUTE,
            None => UNCHANGED_ROUTE,
        };
        connect(&mut lines, (sx, sy), (lane, sy), status);
        connect(&mut lines, (lane, sy), (lane, edge.route_y), status);
        connect(
            &mut lines,
            (lane, edge.route_y),
            (tx - 2, edge.route_y),
            status,
        );
        connect(&mut lines, (tx - 2, edge.route_y), (tx - 2, ty), status);
        connect(&mut lines, (tx - 2, ty), (tx - 1, ty), status);
        arrows.push((tx, ty, edge.status));
    }
    for ((x, y), (directions, statuses)) in lines {
        put(
            buf,
            area,
            x,
            y,
            (app.pan_x, app.pan_y),
            line_glyph(directions),
            route_style(statuses),
        );
    }
    for (x, y, status) in arrows {
        let style = status.map(edge_style).unwrap_or(normal);
        put(buf, area, x, y, (app.pan_x, app.pan_y), "→", style);
        if let Some(status) = status {
            put(
                buf,
                area,
                x - 1,
                y,
                (app.pan_x, app.pan_y),
                if status == EdgeChangeKind::Added {
                    "+"
                } else {
                    "-"
                },
                style,
            );
        }
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
        let right = component.x + component.width - 1;
        let bottom = component.y + component.height - 1;
        let fill_left = (component.x + 1).max(app.pan_x);
        let fill_right = right.min(app.pan_x + i32::from(area.width));
        let fill_top = (component.y + 1).max(app.pan_y);
        let fill_bottom = bottom.min(app.pan_y + i32::from(area.height));
        for y in fill_top..fill_bottom {
            for x in fill_left..fill_right {
                put(buf, area, x, y, (app.pan_x, app.pan_y), " ", base_style);
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
                base_style,
            );
            put(
                buf,
                area,
                x,
                bottom,
                (app.pan_x, app.pan_y),
                lower,
                base_style,
            );
        }
        for y in component.y + 1..bottom {
            put(
                buf,
                area,
                component.x,
                y,
                (app.pan_x, app.pan_y),
                "│",
                base_style,
            );
            put(buf, area, right, y, (app.pan_x, app.pan_y), "│", base_style);
        }
        put_text(
            buf,
            area,
            component.x + 2,
            component.y + 1,
            (app.pan_x, app.pan_y),
            &component.title,
            if selected {
                base_style.add_modifier(Modifier::BOLD)
            } else {
                base_style
            },
        );
        for x in component.x + 1..right {
            put(
                buf,
                area,
                x,
                component.y + 2,
                (app.pan_x, app.pan_y),
                "─",
                base_style,
            );
        }
        put_text(
            buf,
            area,
            component.x + 2,
            component.y + 3,
            (app.pan_x, app.pan_y),
            &component.description,
            base_style.add_modifier(Modifier::DIM),
        );
        for (row, text) in component.edge_labels.iter().enumerate() {
            let style = if text.starts_with("+ ") {
                edge_style(EdgeChangeKind::Added)
            } else {
                edge_style(EdgeChangeKind::Deleted)
            };
            put_text(
                buf,
                area,
                component.x + 1,
                component.y + 4 + row as i32,
                (app.pan_x, app.pan_y),
                text,
                style,
            );
        }
        if !component.extra.is_empty() {
            let separator = component.y + 4 + component.edge_labels.len() as i32;
            for x in component.x + 1..right {
                put(
                    buf,
                    area,
                    x,
                    separator,
                    (app.pan_x, app.pan_y),
                    "─",
                    base_style,
                );
            }
            for (row, text) in component.extra.iter().enumerate() {
                put_text(
                    buf,
                    area,
                    component.x + 1,
                    separator + 1 + row as i32,
                    (app.pan_x, app.pan_y),
                    text,
                    base_style,
                );
            }
        }
    }
}
pub const HEADER_HEIGHT: u16 = 4;
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
            "Green +→ added call; red -→ removed call. Shared routes stay neutral.",
        ];
        frame.render_widget(
            Paragraph::new(help.join("\n"))
                .block(panel("Help"))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    // Only require room for a bordered graph cell below the status header.
    // Smaller viewports remain navigable by panning rather than hiding the graph.
    if area.width < 3 || area.height < HEADER_HEIGHT + 3 {
        frame.render_widget(Paragraph::new("Terminal too small for graph. Press q to quit or ? for help.").block(panel("")).wrap(Wrap { trim: false }), area);
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
            "{}{} | depth: {} | {} | {} symbols | {} edge changes | {} nodes\n{}",
            app.reload_error
                .as_ref()
                .map(|e| format!("RELOAD ERROR: {} | ", safe_text(e)))
                .unwrap_or_default(),
            mode_label(app.mode),
            app.depth,
            status,
            app.changed_roots.len(),
            app.session.edge_comparison.changes.len(),
            app.layout.order.len(),
            identity
        ))
        .block(panel("")),
        vertical[0],
    );
    let block = panel("");
    let inner = block.inner(vertical[1]);
    frame.render_widget(block, vertical[1]);
    if app.layout.components.is_empty() {
        let mut message = vec!["No symbol or edge changes".to_owned()];
        message.extend(app.diagnostic_lines());
        frame.render_widget(
            Paragraph::new(Text::from(message.join("\n"))).wrap(Wrap { trim: false }),
            inner,
        );
    } else {
        draw_graph(app, frame.buffer_mut(), inner);
    }
}
