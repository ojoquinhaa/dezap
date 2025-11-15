use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
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
    pub username: String,
    peer_names: HashMap<SocketAddr, String>,
    default_bind: SocketAddr,
    default_peer: Option<SocketAddr>,
    pub discovery_enabled: bool,
    pub discovery_target: Option<Ipv4Addr>,
    pending_listen_addr: Option<SocketAddr>,
    pending_connect_addr: Option<SocketAddr>,
    selected_peer: usize,
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
            username: config.identity.username.clone(),
            peer_names: HashMap::new(),
            default_bind: args.bind.unwrap_or(config.listen.bind_addr),
            default_peer: args.connect.or(config.peer.default_peer),
            discovery_enabled: !args.disable_discovery && config.discovery.enabled,
            discovery_target: config.discovery.broadcast,
            pending_listen_addr: None,
            pending_connect_addr: None,
            selected_peer: 0,
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
                self.mode = Mode::ListenAddress;
                self.input = self.default_bind.to_string();
                self.status_line = "Enter listen address".into();
            }
            KeyCode::Char('k') if ctrl => {
                self.mode = Mode::ConnectAddress;
                self.input = self
                    .default_peer
                    .map(|addr| addr.to_string())
                    .unwrap_or_default();
                self.status_line = "Enter peer address".into();
            }
            KeyCode::Char('f') if ctrl => {
                self.mode = Mode::File;
                self.input.clear();
                self.status_line = "Enter file path to send".into();
            }
            KeyCode::Char('d') if ctrl && self.discovery_enabled => {
                self.status_line = "Scanning for peers...".into();
                return Some(ServiceCommand::Discover);
            }
            KeyCode::Char('p') if ctrl => {
                self.shortcut_to_peer();
            }
            KeyCode::Char('u') if ctrl => {
                self.mode = Mode::Username;
                self.input = self.username.clone();
                self.status_line = "Choose a nickname".into();
            }
            KeyCode::Char('r') if ctrl => {
                self.mode = Mode::DiscoveryNetwork;
                self.input = self
                    .discovery_target
                    .map(|ip| ip.to_string())
                    .unwrap_or_else(|| "255.255.255.255".into());
                self.status_line = "Discovery broadcast IP (blank = auto)".into();
            }
            KeyCode::Enter => return self.commit_input(),
            KeyCode::Char(ch) => {
                self.input.push(ch);
            }
            _ => {}
        }
        None
    }

    fn shortcut_to_peer(&mut self) {
        if self.discovered.is_empty() {
            self.status_line = "No peers discovered yet.".into();
            return;
        }
        self.selected_peer = (self.selected_peer + 1) % self.discovered.len();
        let addr = self.discovered[self.selected_peer];
        self.pending_connect_addr = Some(addr);
        self.mode = Mode::ConnectPassword;
        self.input.clear();
        self.status_line =
            format!("Preparing to connect to {addr}. Enter password (blank if none).");
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
            Mode::ListenAddress => match self.input.trim().parse::<SocketAddr>() {
                Ok(addr) => {
                    self.pending_listen_addr = Some(addr);
                    self.mode = Mode::ListenPassword;
                    self.input.clear();
                    self.status_line = "Password for peers (blank = open)".into();
                }
                Err(_) => self.status_line = "Invalid listen address".into(),
            },
            Mode::ListenPassword => {
                if let Some(addr) = self.pending_listen_addr.take() {
                    let password = if self.input.trim().is_empty() {
                        None
                    } else {
                        Some(self.input.trim().to_string())
                    };
                    self.input.clear();
                    self.mode = Mode::Chat;
                    return Some(ServiceCommand::Listen { addr, password });
                }
            }
            Mode::ConnectAddress => match self.input.trim().parse::<SocketAddr>() {
                Ok(addr) => {
                    self.pending_connect_addr = Some(addr);
                    self.mode = Mode::ConnectPassword;
                    self.input.clear();
                    self.status_line = "Peer password (blank if none)".into();
                }
                Err(_) => self.status_line = "Invalid peer address".into(),
            },
            Mode::ConnectPassword => {
                if let Some(addr) = self.pending_connect_addr.take() {
                    let password = if self.input.trim().is_empty() {
                        None
                    } else {
                        Some(self.input.trim().to_string())
                    };
                    self.input.clear();
                    self.mode = Mode::Chat;
                    return Some(ServiceCommand::Connect { addr, password });
                }
            }
            Mode::Username => {
                if self.input.trim().is_empty() {
                    self.status_line = "Nickname cannot be empty".into();
                    return None;
                }
                self.username = self.input.trim().to_string();
                self.input.clear();
                self.mode = Mode::Chat;
                return Some(ServiceCommand::SetUsername {
                    username: self.username.clone(),
                });
            }
            Mode::DiscoveryNetwork => {
                let trimmed = self.input.trim();
                let target = if trimmed.is_empty() {
                    None
                } else {
                    match trimmed.parse::<Ipv4Addr>() {
                        Ok(ip) => Some(ip),
                        Err(_) => {
                            self.status_line = "Enter a valid IPv4, e.g. 192.168.0.255".into();
                            return None;
                        }
                    }
                };
                self.input.clear();
                self.mode = Mode::Chat;
                self.discovery_target = target;
                self.status_line = target
                    .map(|ip| format!("Discovery bound to {ip}"))
                    .unwrap_or_else(|| "Discovery reset to default broadcast".into());
                return Some(ServiceCommand::SetDiscoveryTarget { target });
            }
        }
        None
    }

    pub fn handle_service_event(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::Connected { peer, name } => {
                self.peer_names.insert(peer, name.clone());
                self.connection = ConnectionStatus::Connected {
                    peer,
                    name: name.clone(),
                };
                self.push_system(format!("Connected to {name} ({peer})"));
            }
            ServiceEvent::Connecting { peer } => {
                self.connection = ConnectionStatus::Connecting(peer);
                self.status_line = format!("Connecting to {peer}…");
            }
            ServiceEvent::Listening {
                addr,
                password_protected,
            } => {
                self.connection = ConnectionStatus::Listening {
                    addr,
                    locked: password_protected,
                };
                if password_protected {
                    self.push_system(format!("Listening on {addr} (locked)"));
                } else {
                    self.push_system(format!("Listening on {addr}"));
                }
            }
            ServiceEvent::ListenerStopped => {
                self.connection = ConnectionStatus::Disconnected;
                self.push_system("Listener stopped");
            }
            ServiceEvent::Disconnected => {
                self.connection = ConnectionStatus::Disconnected;
                self.push_system("Disconnected");
            }
            ServiceEvent::MessageReceived { peer, author, text } => {
                self.peer_names.insert(peer, author.clone());
                self.push_message(MessageDirection::Incoming(author), text);
            }
            ServiceEvent::MessageSent { author, text } => {
                self.username = author.clone();
                self.push_message(MessageDirection::Outgoing(author), text);
            }
            ServiceEvent::PeerProfile { peer, username } => {
                self.peer_names.insert(peer, username.clone());
                if let ConnectionStatus::Connected { peer: current, .. } = &mut self.connection {
                    if *current == peer {
                        self.connection = ConnectionStatus::Connected {
                            peer,
                            name: username.clone(),
                        };
                    }
                }
                self.push_system(format!("{username} is now online ({peer})"));
            }
            ServiceEvent::FileTransfer(progress) => self.update_transfer(progress),
            ServiceEvent::Discovery(event) => match event {
                DiscoveryEvent::PeerFound(peer) => {
                    if !self.discovered.contains(&peer) {
                        self.discovered.push(peer);
                        self.discovered.sort();
                        if self.selected_peer >= self.discovered.len() {
                            self.selected_peer = 0;
                        }
                    }
                    self.status_line = format!("Found peer {peer}");
                }
                DiscoveryEvent::Completed => {
                    if self.discovered.is_empty() {
                        self.status_line = "No peers were found".into();
                        self.selected_peer = 0;
                    } else {
                        self.status_line = format!("{} peer(s) ready", self.discovered.len());
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
        let author = direction.source().to_string();
        self.messages.push(ChatEntry {
            direction,
            author,
            text,
            timestamp: OffsetDateTime::now_utc(),
        });
    }

    pub fn peer_alias(&self, addr: &SocketAddr) -> Option<&String> {
        self.peer_names.get(addr)
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
    ListenAddress,
    ListenPassword,
    ConnectAddress,
    ConnectPassword,
    Username,
    DiscoveryNetwork,
}

/// Connection state summary.
#[derive(Debug, Clone)]
pub enum ConnectionStatus {
    Disconnected,
    Listening { addr: SocketAddr, locked: bool },
    Connecting(SocketAddr),
    Connected { peer: SocketAddr, name: String },
}

/// Direction of a chat entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageDirection {
    Incoming(String),
    Outgoing(String),
    System,
}

impl MessageDirection {
    pub fn style(&self) -> Color {
        match self {
            MessageDirection::Incoming(_) => Color::LightCyan,
            MessageDirection::Outgoing(_) => Color::LightGreen,
            MessageDirection::System => Color::Gray,
        }
    }

    fn source(&self) -> &str {
        match self {
            MessageDirection::Incoming(name) => name,
            MessageDirection::Outgoing(name) => name,
            MessageDirection::System => "system",
        }
    }
}

/// Entry to render in the message list.
#[derive(Debug, Clone)]
pub struct ChatEntry {
    pub direction: MessageDirection,
    pub author: String,
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
        "crimson" => Color::Rgb(220, 20, 60),
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
    fn listen_flow_requests_password() {
        let config = AppConfig::default();
        let args = TuiCommand::default();
        let mut app = App::new(&config, &args);
        app.mode = Mode::ListenAddress;
        app.input = "127.0.0.1:6000".into();
        assert!(app.commit_input().is_none());
        app.input = "secret".into();
        app.mode = Mode::ListenPassword;
        assert!(matches!(
            app.commit_input(),
            Some(ServiceCommand::Listen { addr, password }) if addr.port() == 6000 && password == Some("secret".into())
        ));
    }
}
