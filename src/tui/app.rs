use std::net::SocketAddr;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::Color;
use time::OffsetDateTime;

use crate::cli::TuiCommand;
use crate::config::AppConfig;
use crate::service::{
    DiscoveryEvent, FileTransferProgress, ServiceCommand, ServiceEvent, TransferDirection,
};

const MAX_MESSAGES: usize = 512;

/// High-level application state powering the TUI.
pub struct App {
    pub messages: Vec<ChatEntry>,
    pub input: String,
    pub mode: Mode,
    pub status_line: String,
    pub transfers: Vec<TransferState>,
    pub discovered: Vec<SocketAddr>,
    pub show_help: bool,
    pub should_quit: bool,
    pub accent: Color,
    pub connection: ConnectionStatus,
    default_bind: SocketAddr,
    default_peer: Option<SocketAddr>,
    pub discovery_enabled: bool,
}

impl App {
    pub fn new(config: &AppConfig, args: &TuiCommand) -> Self {
        Self {
            messages: Vec::new(),
            input: String::new(),
            mode: Mode::Chat,
            status_line: "Press Ctrl+K to connect or Ctrl+L to listen".to_string(),
            transfers: Vec::new(),
            discovered: Vec::new(),
            show_help: false,
            should_quit: false,
            accent: parse_color(&config.ui.accent),
            connection: ConnectionStatus::Disconnected,
            default_bind: args.bind.unwrap_or(config.listen.bind_addr),
            default_peer: args.connect.or(config.peer.default_peer),
            discovery_enabled: !args.disable_discovery && config.discovery.enabled,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ServiceCommand> {
        if key.kind != KeyEventKind::Press {
            return None;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if ctrl => {
                self.should_quit = true;
                return Some(ServiceCommand::Disconnect);
            }
            KeyCode::Char('q') if !ctrl => {
                self.should_quit = true;
                return Some(ServiceCommand::Disconnect);
            }
            KeyCode::Esc => {
                self.mode = Mode::Chat;
                self.input.clear();
                self.status_line.clear();
            }
            KeyCode::Tab => self.show_help = !self.show_help,
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char('l') if ctrl => {
                self.mode = Mode::Listen;
                self.input = self.default_bind.to_string();
                self.status_line = "Enter listen address".to_string();
            }
            KeyCode::Char('k') if ctrl => {
                self.mode = Mode::Connect;
                self.input = self
                    .default_peer
                    .map(|addr| addr.to_string())
                    .unwrap_or_default();
                self.status_line = "Enter peer address".to_string();
            }
            KeyCode::Char('f') if ctrl => {
                self.mode = Mode::File;
                self.input.clear();
                self.status_line = "Enter file path to send".to_string();
            }
            KeyCode::Char('d') if ctrl && self.discovery_enabled => {
                self.status_line = "Scanning for peers...".to_string();
                return Some(ServiceCommand::Discover);
            }
            KeyCode::Enter => return self.commit_input(),
            KeyCode::Char(ch) => {
                self.input.push(ch);
            }
            _ => {}
        }
        None
    }

    fn commit_input(&mut self) -> Option<ServiceCommand> {
        match self.mode {
            Mode::Chat => {
                if self.input.trim().is_empty() {
                    self.status_line = "Cannot send empty message".into();
                    return None;
                }
                let text = self.input.clone();
                self.input.clear();
                return Some(ServiceCommand::SendText { text });
            }
            Mode::File => {
                if self.input.trim().is_empty() {
                    self.status_line = "Provide a file path".into();
                    return None;
                }
                let path = PathBuf::from(self.input.trim());
                self.input.clear();
                self.mode = Mode::Chat;
                return Some(ServiceCommand::SendFile { path });
            }
            Mode::Listen => match self.input.trim().parse::<SocketAddr>() {
                Ok(addr) => {
                    self.mode = Mode::Chat;
                    self.status_line = format!("Listening on {addr}");
                    return Some(ServiceCommand::Listen { addr });
                }
                Err(_) => {
                    self.status_line = "Invalid listen address".into();
                }
            },
            Mode::Connect => match self.input.trim().parse::<SocketAddr>() {
                Ok(addr) => {
                    self.mode = Mode::Chat;
                    self.status_line = format!("Connecting to {addr}");
                    return Some(ServiceCommand::Connect { addr });
                }
                Err(_) => {
                    self.status_line = "Invalid peer address".into();
                }
            },
        }
        None
    }

    pub fn handle_service_event(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::Connected { peer } => {
                self.connection = ConnectionStatus::Connected(peer);
                self.push_system(format!("Connected to {peer}"));
            }
            ServiceEvent::Connecting { peer } => {
                self.connection = ConnectionStatus::Connecting(peer);
                self.status_line = format!("Connecting to {peer}...");
            }
            ServiceEvent::Listening { addr } => {
                self.connection = ConnectionStatus::Listening(addr);
                self.push_system(format!("Listening on {addr}"));
            }
            ServiceEvent::ListenerStopped => {
                self.connection = ConnectionStatus::Disconnected;
                self.push_system("Listener stopped");
            }
            ServiceEvent::Disconnected => {
                self.connection = ConnectionStatus::Disconnected;
                self.push_system("Disconnected");
            }
            ServiceEvent::MessageReceived { peer, text } => {
                self.push_message(MessageDirection::Incoming, format!("{peer}: {text}"));
            }
            ServiceEvent::MessageSent { text } => {
                self.push_message(MessageDirection::Outgoing, text);
            }
            ServiceEvent::FileTransfer(progress) => self.update_transfer(progress),
            ServiceEvent::Discovery(event) => match event {
                DiscoveryEvent::PeerFound(peer) => {
                    if !self.discovered.contains(&peer) {
                        self.discovered.push(peer);
                        self.discovered.sort();
                    }
                    self.status_line = format!("Found peer {peer}");
                }
                DiscoveryEvent::Completed => {
                    if self.discovered.is_empty() {
                        self.status_line = "No peers discovered".into();
                    } else {
                        self.status_line = format!("{} peers ready", self.discovered.len());
                    }
                }
            },
            ServiceEvent::Error { message } => {
                self.push_system(format!("Error: {message}"));
                self.status_line = message;
            }
        }
    }

    fn update_transfer(&mut self, progress: FileTransferProgress) {
        if let Some(existing) = self.transfers.iter_mut().find(|t| t.id == progress.id) {
            existing.transferred = progress.transferred;
            existing.total = progress.total;
            existing.completed = progress.completed;
            existing.path = progress.path.clone();
            return;
        }
        self.transfers.push(TransferState {
            id: progress.id,
            name: progress.name.clone(),
            direction: progress.direction,
            transferred: progress.transferred,
            total: progress.total,
            path: progress.path,
            completed: progress.completed,
        });
    }

    fn push_message(&mut self, direction: MessageDirection, text: String) {
        if self.messages.len() >= MAX_MESSAGES {
            self.messages.remove(0);
        }
        self.messages.push(ChatEntry {
            direction,
            text,
            timestamp: OffsetDateTime::now_utc(),
        });
    }

    fn push_system(&mut self, text: impl Into<String>) {
        self.push_message(MessageDirection::System, text.into());
    }
}

/// Rendering mode of the input area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Chat,
    File,
    Listen,
    Connect,
}

/// Connection state summary.
#[derive(Debug, Clone, Copy)]
pub enum ConnectionStatus {
    Disconnected,
    Listening(SocketAddr),
    Connecting(SocketAddr),
    Connected(SocketAddr),
}

/// Direction of a chat entry.
#[derive(Debug, Clone, Copy)]
pub enum MessageDirection {
    Incoming,
    Outgoing,
    System,
}

/// Entry to render in the message list.
#[derive(Debug, Clone)]
pub struct ChatEntry {
    pub direction: MessageDirection,
    pub text: String,
    pub timestamp: OffsetDateTime,
}

/// Transfer progress representation for the sidebar.
#[derive(Debug, Clone)]
pub struct TransferState {
    pub id: u64,
    pub name: String,
    pub direction: TransferDirection,
    pub transferred: u64,
    pub total: u64,
    pub path: Option<PathBuf>,
    pub completed: bool,
}

fn parse_color(raw: &str) -> Color {
    match raw.to_ascii_lowercase().as_str() {
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "cyan" => Color::Cyan,
        "blue" => Color::Blue,
        "white" => Color::White,
        _ => Color::Magenta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_submit_requires_non_empty() {
        let config = AppConfig::default();
        let args = TuiCommand::default();
        let mut app = App::new(&config, &args);
        app.input = "".into();
        assert!(app.commit_input().is_none());
        app.mode = Mode::Chat;
        app.input = "hi".into();
        assert!(matches!(
            app.commit_input(),
            Some(ServiceCommand::SendText { text }) if text == "hi"
        ));
    }

    #[test]
    fn listen_mode_parses_socket() {
        let config = AppConfig::default();
        let args = TuiCommand::default();
        let mut app = App::new(&config, &args);
        app.mode = Mode::Listen;
        app.input = "127.0.0.1:5000".into();
        assert!(matches!(
            app.commit_input(),
            Some(ServiceCommand::Listen { addr }) if addr.port() == 5000
        ));
    }
}
