use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap};
use time::macros::format_description;

use crate::service::TransferDirection;

use super::app::{App, ConnectionStatus, Mode};

const BANNER_LINE: &str = "dezap — retro LAN QUIC messenger — est. 2024";
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
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(10),
            Constraint::Min(8),
            Constraint::Length(5),
        ])
        .split(frame.area());

    draw_banner(frame, outer[0], app);

    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(5)])
        .split(outer[1]);

    draw_body(frame, body[0], app);
    draw_input(frame, outer[2], app);
}

fn draw_banner(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!("  {BANNER_LINE}  "),
        Style::default().fg(app.accent),
    )));
    for row in DEMON_LINES {
        lines.push(Line::from(Span::styled(
            row,
            Style::default().fg(Color::DarkGray),
        )));
    }
    let paragraph = Paragraph::new(lines).alignment(Alignment::Center).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled("DeZap", Style::default().fg(app.accent))),
    );
    frame.render_widget(paragraph, area);
}

fn draw_body(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
        .split(area);
    draw_messages(frame, columns[0], app);
    draw_sidebar(frame, columns[1], app);
}

fn draw_messages(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let fmt = format_description!("[hour]:[minute]:[second]");
    for entry in &app.messages {
        let ts = entry
            .timestamp
            .format(fmt)
            .unwrap_or_else(|_| "--:--".into());
        let prefix = format!("[{ts}] {} • ", entry.author);
        let mut line = Line::from(vec![
            Span::styled(prefix, Style::default().fg(Color::Gray)),
            Span::styled(
                entry.text.clone(),
                Style::default().fg(entry.direction.style()),
            ),
        ]);
        line.alignment = Some(Alignment::Left);
        lines.push(line);
    }

    let total_lines = lines.len();
    let visible = area.height as usize;
    let scroll = total_lines.saturating_sub(visible);

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0))
        .block(
            Block::default()
                .title("Chat")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.accent)),
        );
    frame.render_widget(paragraph, area);
}

fn draw_sidebar(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(4),
            Constraint::Min(5),
        ])
        .split(area);

    draw_status(frame, chunks[0], app);
    draw_transfers(frame, chunks[1], app);
    draw_discovery(frame, chunks[2], app);
    draw_help(frame, chunks[3], app);
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
            Span::styled("You: ", Style::default().fg(Color::Gray)),
            Span::styled(app.username.as_str(), Style::default().fg(app.accent)),
        ]),
        Line::from(status),
        Line::from(format!(
            "Discovery: {}",
            app.discovery_target
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "auto".into())
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

    let list = List::new(items).block(
        Block::default()
            .title("Peers (Ctrl+P cycles)")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.accent)),
    );
    frame.render_widget(list, area);
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
        ("Ctrl+P", "Cycle discovered peers"),
        ("Ctrl+F", "Send a file"),
        ("Ctrl+U", "Rename yourself"),
        ("Ctrl+D", "Discover peers"),
        ("Ctrl+R", "Set discovery network"),
        ("Esc", "Cancel focus"),
        ("Ctrl+C / q", "Quit"),
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
        .constraints([Constraint::Length(3), Constraint::Length(2)])
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
    };
    let input = Paragraph::new(app.input.as_str())
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
}
