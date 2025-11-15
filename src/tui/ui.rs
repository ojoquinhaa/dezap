use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Gauge, List, ListDirection, ListItem, Paragraph, Wrap};
use ratatui::Frame;
use time::macros::format_description;

use super::app::{App, ConnectionStatus, MessageDirection, Mode};
use crate::service::TransferDirection;

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let size = frame.area();
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(5)])
        .split(size);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(outer[0]);

    draw_messages(frame, columns[0], app);
    draw_sidebar(frame, columns[1], app);
    draw_input(frame, outer[1], app);
}

fn draw_messages(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mut items = Vec::with_capacity(app.messages.len());
    for entry in &app.messages {
        let ts = entry
            .timestamp
            .format(&format_description!("[hour]:[minute]"))
            .unwrap_or_else(|_| "--:--".into());
        let prefix = format!("[{ts}] {}", entry.text);
        let style = match entry.direction {
            MessageDirection::Incoming => Style::default().fg(Color::LightCyan),
            MessageDirection::Outgoing => Style::default().fg(Color::LightGreen),
            MessageDirection::System => Style::default().fg(Color::Gray),
        };
        items.push(ListItem::new(prefix).style(style));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .title("Chat")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.accent)),
        )
        .direction(ListDirection::BottomToTop);
    frame.render_widget(list, area);
}

fn draw_sidebar(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(3),
            Constraint::Length(8),
        ])
        .split(area);

    let status = Paragraph::new(connection_text(app))
        .block(
            Block::default()
                .title("Status")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.accent)),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(status, chunks[0]);

    draw_transfers(frame, chunks[1], app);
    draw_discovery(frame, chunks[2], app);
    draw_help(frame, chunks[3], app);
}

fn draw_transfers(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.transfers.is_empty() {
        let empty = Paragraph::new("No transfers")
            .block(Block::default().title("Transfers").borders(Borders::ALL));
        frame.render_widget(empty, area);
        return;
    }

    let mut offset = area.top();
    for transfer in &app.transfers {
        let ratio = if transfer.total == 0 {
            0.0
        } else {
            transfer.transferred as f64 / transfer.total as f64
        };
        let percent = (ratio * 100.0) as u16;
        let label = format!(
            "{} [{}] {}%",
            transfer.name,
            match transfer.direction {
                TransferDirection::Incoming => "in",
                TransferDirection::Outgoing => "out",
            },
            percent
        );
        let gauge = Gauge::default()
            .ratio(ratio.min(1.0))
            .label(label)
            .use_unicode(true)
            .style(if transfer.completed {
                Style::default().fg(Color::LightGreen)
            } else {
                Style::default().fg(app.accent)
            })
            .block(
                Block::default()
                    .title(transfer.name.clone())
                    .borders(Borders::ALL),
            );
        let height = 3;
        let remaining = area.bottom().saturating_sub(offset);
        if remaining == 0 {
            break;
        }
        let rect_height = height.min(remaining);
        let rect = Rect::new(area.x, offset, area.width, rect_height);
        frame.render_widget(gauge, rect);
        offset = offset.saturating_add(rect_height);
        if offset >= area.bottom() {
            break;
        }
    }
}

fn draw_discovery(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let peers = if app.discovered.is_empty() {
        vec![ListItem::new("No peers")]
    } else {
        app.discovered
            .iter()
            .map(|addr| ListItem::new(addr.to_string()))
            .collect()
    };
    let block = Block::default()
        .title("Peers")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.accent));
    let list = List::new(peers)
        .block(block)
        .highlight_style(Style::default().fg(Color::Yellow));
    frame.render_widget(list, area);
}

fn draw_help(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let content = if app.show_help {
        "Ctrl+K connect • Ctrl+L listen • Ctrl+F send file • Ctrl+D discover"
    } else {
        "Press Tab for help"
    };
    let block = Block::default()
        .title("Help")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.accent));
    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}
fn draw_input(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Length(2)])
        .split(area);
    let mode_label = match app.mode {
        Mode::Chat => "Message",
        Mode::File => "Send file",
        Mode::Listen => "Listen",
        Mode::Connect => "Connect",
    };
    let input = Paragraph::new(app.input.as_str())
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .title(mode_label)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.accent)),
        );
    frame.render_widget(input, rows[0]);
    let status = Paragraph::new(app.status_line.as_str())
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::NONE));
    frame.render_widget(status, rows[1]);
}

fn connection_text(app: &App) -> String {
    match app.connection {
        ConnectionStatus::Disconnected => "Disconnected".into(),
        ConnectionStatus::Listening(addr) => format!("Listening on {addr}"),
        ConnectionStatus::Connecting(addr) => format!("Connecting to {addr}"),
        ConnectionStatus::Connected(addr) => format!("Connected to {addr}"),
    }
}
