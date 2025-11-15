use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph, Wrap};
use time::macros::format_description;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::service::TransferDirection;

use textwrap::wrap;

use super::app::{App, ConnectionStatus, Mode, PanelFocus};

const BANNER_LINE: &str = "Retro LAN QUIC Messenger";
const DEMON_LINES: [&str; 7] = [
    "⠀⠀⠀⠀⠀⠀⢀⣤⡄⠀⠀⠀⠀⠀⠀⠀⢤⣤⡀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⣰⣿⣿⠀⠀⠀⠀⠀⠀⠀⠀⠀⢻⣿⣆⠀⠀",
    "⠀⠀⠀⠀⣰⣿⣿⠃⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⣿⣿⣇⠀",
    "⠀⠀⠀⢀⣿⣿⣿⣧⠀⠀⠀⠀⠀⠀⠀⠀⠀⢼⣿⣿⣿⡄",
    "⠀⠀⠀⢸⣿⣿⣿⣿⣄⠀⠀⠀⠀⠀⠀⠀⣠⣿⣿⣿⣿⡇",
    "⠀⠀⠀⠘⣿⣿⣿⣿⣿⣦⠀⠀  ⣼⣿⣿⣿⣿⣿⠇",
    "⠀⠀⠀⠀⢿⣿⣿⣿⡿⠃⠀⠀⠀⠀⠀⠘⢿⣿⣿⣿⡟⠀",
];

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
        .split(frame.area());

    let chat = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(12), Constraint::Length(8)])
        .split(columns[0]);

    draw_messages(frame, chat[0], app);
    draw_input(frame, chat[1], app);
    draw_sidebar(frame, columns[1], app);
}

fn draw_messages(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let fmt = format_description!("[hour]:[minute]:[second]");
    let inner_width = area.width.saturating_sub(4).max(10) as usize;
    let items: Vec<ListItem<'static>> = app
        .messages
        .iter()
        .map(|entry| {
            let ts = entry
                .timestamp
                .format(fmt)
                .unwrap_or_else(|_| "--:--".into());
            let prefix = format!("[{ts}] {} • ", entry.author);
            let prefix_width = UnicodeWidthStr::width(prefix.as_str());
            let available = inner_width.saturating_sub(prefix_width).max(1);
            let wrapped = wrap(&entry.text, available)
                .into_iter()
                .map(|cow| cow.to_string())
                .collect::<Vec<_>>();
            let pieces = if wrapped.is_empty() {
                vec![String::new()]
            } else {
                wrapped
            };
            let indent = " ".repeat(prefix_width);
            let mut lines = Vec::new();
            if let Some(first) = pieces.first() {
                lines.push(Line::from(vec![
                    Span::styled(prefix.clone(), Style::default().fg(Color::Gray)),
                    Span::styled(first.clone(), Style::default().fg(entry.direction.style())),
                ]));
                for rest in pieces.iter().skip(1) {
                    lines.push(Line::from(vec![
                        Span::raw(indent.clone()),
                        Span::styled(rest.clone(), Style::default().fg(entry.direction.style())),
                    ]));
                }
            }
            ListItem::new(lines)
        })
        .collect();
    let title = if app.chat_focus {
        "Chat ▸ browse (Esc to exit, ↑/↓ move, C copy)"
    } else {
        "Chat"
    };
    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.accent)),
        )
        .highlight_symbol("› ")
        .highlight_style(Style::default().bg(app.accent).fg(Color::Black));
    let mut state = ListState::default();
    if app.chat_focus {
        state.select(app.selected_message);
    } else if !app.messages.is_empty() {
        let inner_height = area.height.saturating_sub(2) as usize;
        let visible = inner_height.max(1);
        *state.offset_mut() = app.messages.len().saturating_sub(visible);
        state.select(None);
    }
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_sidebar(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(11),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Min(5),
        ])
        .split(area);

    draw_header(frame, chunks[0], app);
    draw_status(frame, chunks[1], app);
    draw_transfers(frame, chunks[2], app);
    draw_discovery(frame, chunks[3], app);
    draw_saved_peers(frame, chunks[4], app);
    draw_help(frame, chunks[5], app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(Span::styled(
        BANNER_LINE,
        Style::default().fg(app.accent),
    )));
    for row in DEMON_LINES.iter() {
        lines.push(Line::from(Span::styled(
            *row,
            Style::default().fg(Color::DarkGray),
        )));
    }
    let paragraph = Paragraph::new(lines).alignment(Alignment::Center).block(
        Block::default()
            .title("Dezap - TheJohn")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.accent)),
    );
    frame.render_widget(paragraph, area);
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let status = match app.connection {
        ConnectionStatus::Disconnected => "Disconnected".into(),
        ConnectionStatus::Listening { addr, locked } => {
            if locked {
                format!("Listening on {addr} 🔒")
            } else {
                format!("Listening on {addr}")
            }
        }
        ConnectionStatus::Connecting(addr) => format!("Connecting to {addr}…"),
        ConnectionStatus::Connected { peer, ref name } => {
            format!("Connected to {name} ({peer})")
        }
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("Handle: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("@{}", app.username),
                Style::default().fg(app.accent),
            ),
        ]),
        Line::from(vec![
            Span::styled("Local IP: ", Style::default().fg(Color::Gray)),
            Span::raw(app.bind_address().to_string()),
        ]),
        Line::from(status),
        Line::from(format!(
            "Discovery: {} {}",
            if app.discovery_enabled { "on" } else { "off" },
            app.discovery_target
                .map(|ip| format!("({ip})"))
                .unwrap_or_default()
        )),
    ];

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
        Block::default()
            .title("Status")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.accent)),
    );
    frame.render_widget(paragraph, area);
}

fn draw_transfers(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.transfers.is_empty() {
        let empty = Paragraph::new("No transfers")
            .block(Block::default().title("Transfers").borders(Borders::ALL));
        frame.render_widget(empty, area);
        return;
    }

    let mut offset = area.y;
    for transfer in &app.transfers {
        let ratio = if transfer.total == 0 {
            0.0
        } else {
            transfer.transferred as f64 / transfer.total as f64
        };
        let label = format!(
            "{} {}",
            transfer.name,
            match transfer.direction {
                TransferDirection::Incoming => "⬇",
                TransferDirection::Outgoing => "⬆",
            }
        );
        let gauge = Gauge::default()
            .ratio(ratio.min(1.0))
            .label(label)
            .style(if transfer.completed {
                Style::default().fg(Color::LightGreen)
            } else {
                Style::default().fg(app.accent)
            })
            .block(Block::default().borders(Borders::ALL));
        let chunk = Rect::new(
            area.x,
            offset,
            area.width,
            3.min(area.bottom().saturating_sub(offset)),
        );
        frame.render_widget(gauge, chunk);
        offset = offset.saturating_add(3);
        if offset >= area.bottom() {
            break;
        }
    }
}

fn draw_discovery(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items: Vec<ListItem<'static>> = if app.discovered.is_empty() {
        vec![ListItem::new("No peers")]
    } else {
        app.discovered
            .iter()
            .enumerate()
            .map(|(idx, addr)| {
                let label = if let Some(name) = app.peer_alias(addr) {
                    format!("{idx:>2}. {name} ({addr})")
                } else {
                    format!("{idx:>2}. {addr}")
                };
                ListItem::new(label)
            })
            .collect()
    };

    let mut state = ListState::default();
    if app.panel_focus() == PanelFocus::Discovered {
        if let Some(idx) = app.selected_discovered() {
            state.select(Some(idx));
        }
    }
    let list = List::new(items).block(
        Block::default()
            .title("Discovered Peers (Ctrl+P)")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.accent)),
    );
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_saved_peers(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items: Vec<ListItem<'static>> = if app.saved_peers.is_empty() {
        vec![ListItem::new("No saved peers yet")]
    } else {
        app.saved_peers
            .iter()
            .enumerate()
            .map(|(idx, peer)| ListItem::new(format!("{idx:>2}. {} ({})", peer.name, peer.addr)))
            .collect()
    };
    let list = List::new(items).block(
        Block::default()
            .title("Saved Peers (Ctrl+S)")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.accent)),
    );
    let mut state = ListState::default();
    if app.panel_focus() == PanelFocus::Saved {
        if let Some(idx) = app.selected_saved() {
            state.select(Some(idx));
        }
    }
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_help(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if !app.show_help {
        let paragraph = Paragraph::new("Press TAB for shortcuts")
            .block(Block::default().title("Help").borders(Borders::ALL));
        frame.render_widget(paragraph, area);
        return;
    }

    let entries = [
        ("Ctrl+L", "Host listener"),
        ("Ctrl+K", "Connect to peer"),
        ("Ctrl+P", "Focus discovered peers"),
        ("Ctrl+S", "Focus saved peers"),
        ("Ctrl+X", "Disconnect from peer"),
        ("Tab", "Toggle help / autocomplete paths"),
        ("Ctrl+G", "Focus chat history"),
        ("Arrows", "Navigate focused chat"),
        ("Enter (panel)", "Connect to highlighted peer"),
        ("C (browse)", "Copy highlighted message"),
        ("Ctrl+F", "Send a file"),
        ("Ctrl+U", "Rename yourself"),
        ("Ctrl+D", "Discover peers"),
        ("Ctrl+R", "Set discovery network"),
        ("Esc", "Cancel focus"),
        ("Ctrl+C", "Quit"),
    ];

    let items: Vec<ListItem<'static>> = entries
        .into_iter()
        .map(|(key, text)| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{key:<10}"),
                    Style::default().fg(Color::LightYellow),
                ),
                Span::raw(text),
            ]))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title("Help")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.accent)),
    );
    frame.render_widget(list, area);
}

fn draw_input(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Length(2)])
        .split(area);
    let label = match app.mode {
        Mode::Chat => "Message",
        Mode::File => "Send file",
        Mode::ListenAddress => "Listen address",
        Mode::ListenPassword => "Listen password",
        Mode::ConnectAddress => "Peer address",
        Mode::ConnectPassword => "Peer password",
        Mode::Username => "Nickname",
        Mode::DiscoveryNetwork => "Discovery broadcast",
        Mode::IncomingFile(_) => "Save incoming file as",
    };
    let input_height = rows[0].height.max(1);
    let input_width = rows[0].width.max(1);
    let (total_lines, cursor_line, cursor_col) = measure_input(&app.input, input_width as usize);
    let scroll = total_lines.saturating_sub(input_height as usize);
    let visible_cursor_line = cursor_line.saturating_sub(scroll);
    let input = Paragraph::new(app.input.as_str())
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0))
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .title(label)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.accent)),
        );
    frame.render_widget(input, rows[0]);
    let status = Paragraph::new(app.status_line.as_str())
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::NONE));
    frame.render_widget(status, rows[1]);

    if !app.chat_focus {
        let cursor_x =
            rows[0].x + 1 + cursor_col.min((input_width as usize).saturating_sub(1)) as u16;
        let cursor_y = rows[0].y
            + 1
            + visible_cursor_line.min((input_height as usize).saturating_sub(1)) as u16;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn measure_input(text: &str, width: usize) -> (usize, usize, usize) {
    if width == 0 {
        return (1, 0, 0);
    }
    let mut total_lines = 1usize;
    let mut line_idx = 0usize;
    let mut col = 0usize;
    let mut cursor_line = 0usize;
    let mut cursor_col = 0usize;
    for ch in text.chars() {
        match ch {
            '\n' => {
                line_idx += 1;
                total_lines += 1;
                col = 0;
                cursor_line = line_idx;
                cursor_col = 0;
            }
            _ => {
                let w = ch.width().unwrap_or(1).max(1);
                if col + w > width {
                    line_idx += 1;
                    total_lines += 1;
                    col = 0;
                }
                col += w;
                cursor_line = line_idx;
                cursor_col = col;
            }
        }
    }
    (total_lines, cursor_line, cursor_col)
}
