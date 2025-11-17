use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use arboard::Clipboard;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::Color;
use time::OffsetDateTime;

use crate::cli::TuiCommand;
use crate::config::AppConfig;
use crate::service::{
    DiscoveryEvent, FileOfferNotice, FileTransferProgress, SavedPeer, ServiceCommand, ServiceEvent,
    TransferDirection,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelFocus {
    None,
    Discovered,
    Saved,
}

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
    pub saved_peers: Vec<SavedPeer>,
    pending_listen_addr: Option<SocketAddr>,
    pending_connect_addr: Option<SocketAddr>,
    selected_peer: usize,
    pub chat_focus: bool,
    pub selected_message: Option<usize>,
    pub marked_messages: HashSet<usize>,
    download_dir: PathBuf,
    offer_queue: VecDeque<FileOfferNotice>,
    active_offer: Option<FileOfferNotice>,
    panel_focus: PanelFocus,
    saved_peer_index: usize,
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
            saved_peers: Vec::new(),
            pending_listen_addr: None,
            pending_connect_addr: None,
            selected_peer: 0,
            chat_focus: false,
            selected_message: None,
            marked_messages: HashSet::new(),
            download_dir: config.paths.download_dir.clone(),
            offer_queue: VecDeque::new(),
            active_offer: None,
            panel_focus: PanelFocus::None,
            saved_peer_index: 0,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ServiceCommand> {
        if key.kind != KeyEventKind::Press {
            return None;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if self.panel_focus != PanelFocus::None {
            match key.code {
                KeyCode::Esc => {
                    self.exit_panel_focus();
                    return None;
                }
                KeyCode::Up => {
                    self.panel_move_selection(-1);
                    return None;
                }
                KeyCode::Down => {
                    self.panel_move_selection(1);
                    return None;
                }
                KeyCode::Enter => {
                    return self.panel_connect_selection();
                }
                _ => {}
            }
            return None;
        }

        if self.chat_focus {
            match key.code {
                KeyCode::Esc => {
                    self.leave_chat_focus();
                    return None;
                }
                KeyCode::Enter => {
                    self.leave_chat_focus();
                    return None;
                }
                KeyCode::Up => {
                    self.move_selection(-1);
                    return None;
                }
                KeyCode::Down => {
                    self.move_selection(1);
                    return None;
                }
                KeyCode::Char(ch) if !ctrl && ch.eq_ignore_ascii_case(&'v') => {
                    self.toggle_marked_message();
                    return None;
                }
                KeyCode::Char(ch) if !ctrl && ch.eq_ignore_ascii_case(&'c') => {
                    self.copy_selected_message();
                    return None;
                }
                _ => {}
            }
            if key.modifiers.is_empty() {
                return None;
            }
        }

        match key.code {
            KeyCode::Char('c') if ctrl => {
                self.should_quit = true;
                return Some(ServiceCommand::Disconnect);
            }
            KeyCode::Esc => {
                if let Mode::IncomingFile(id) = self.mode {
                    let label = self
                        .active_offer
                        .as_ref()
                        .map(|offer| offer.name.clone())
                        .unwrap_or_else(|| "file".into());
                    self.status_line = format!("Declined '{}'", label);
                    self.mark_offer_handled();
                    return Some(ServiceCommand::DeclineFile { id });
                }
                self.mode = Mode::Chat;
                if !self.chat_focus {
                    self.input.clear();
                }
                self.status_line.clear();
                self.leave_chat_focus();
            }
            KeyCode::Tab => {
                if self.mode == Mode::File {
                    self.autocomplete_path();
                } else {
                    self.show_help = !self.show_help;
                }
            }
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
            KeyCode::Char('g') if ctrl => {
                self.toggle_chat_focus();
                return None;
            }
            KeyCode::Char('x') if ctrl => {
                self.status_line = "Disconnecting…".into();
                return Some(ServiceCommand::Disconnect);
            }
            KeyCode::Char('p') if ctrl => {
                self.focus_discovered_panel();
                return None;
            }
            KeyCode::Char('u') if ctrl => {
                self.mode = Mode::Username;
                self.input = self.username.clone();
                self.status_line = "Choose a nickname".into();
            }
            KeyCode::Char('s') if ctrl => {
                self.focus_saved_panel();
                return None;
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

    fn toggle_chat_focus(&mut self) {
        if self.chat_focus {
            self.leave_chat_focus();
        } else {
            self.enter_chat_focus();
        }
    }

    fn enter_chat_focus(&mut self) {
        if self.messages.is_empty() {
            self.chat_focus = false;
            self.selected_message = None;
            self.status_line = "No conversations yet.".into();
            return;
        }
        self.chat_focus = true;
        if self
            .selected_message
            .map(|idx| idx >= self.messages.len())
            .unwrap_or(true)
        {
            self.selected_message = Some(self.messages.len().saturating_sub(1));
        }
        self.status_line =
            "Browsing chat • ↑/↓ move, 'v' mark/unmark, 'c' copy selection, Esc to exit.".into();
    }

    fn leave_chat_focus(&mut self) {
        self.chat_focus = false;
        self.selected_message = None;
    }

    fn focus_discovered_panel(&mut self) {
        if self.discovered.is_empty() {
            self.status_line = "No discovered peers yet.".into();
            return;
        }
        self.selected_peer = self
            .selected_peer
            .min(self.discovered.len().saturating_sub(1));
        self.panel_focus = PanelFocus::Discovered;
        self.status_line =
            "Discovered peers focused · ↑/↓ to navigate, Enter to connect, Esc to cancel".into();
    }

    fn focus_saved_panel(&mut self) {
        if self.saved_peers.is_empty() {
            self.status_line = "No saved peers available yet.".into();
            return;
        }
        self.saved_peer_index = self
            .saved_peer_index
            .min(self.saved_peers.len().saturating_sub(1));
        self.panel_focus = PanelFocus::Saved;
        self.status_line =
            "Saved peers focused · ↑/↓ to navigate, Enter to connect, Esc to cancel".into();
    }

    fn exit_panel_focus(&mut self) {
        self.panel_focus = PanelFocus::None;
        self.status_line.clear();
    }

    fn panel_move_selection(&mut self, delta: isize) {
        match self.panel_focus {
            PanelFocus::Discovered => {
                if self.discovered.is_empty() {
                    return;
                }
                let len = self.discovered.len() as isize;
                let mut next = self.selected_peer as isize + delta;
                if next < 0 {
                    next = len - 1;
                }
                if next >= len {
                    next = 0;
                }
                self.selected_peer = next as usize;
                let addr = self.discovered[self.selected_peer];
                self.status_line = format!("Selected {addr} (Enter to connect)");
            }
            PanelFocus::Saved => {
                if self.saved_peers.is_empty() {
                    return;
                }
                let len = self.saved_peers.len() as isize;
                let mut next = self.saved_peer_index as isize + delta;
                if next < 0 {
                    next = len - 1;
                }
                if next >= len {
                    next = 0;
                }
                self.saved_peer_index = next as usize;
                let peer = &self.saved_peers[self.saved_peer_index];
                self.status_line = format!("Selected {} ({})", peer.name, peer.addr);
            }
            PanelFocus::None => {}
        }
    }

    fn panel_connect_selection(&mut self) -> Option<ServiceCommand> {
        let command = match self.panel_focus {
            PanelFocus::Discovered if !self.discovered.is_empty() => {
                let addr = self.discovered[self.selected_peer];
                Some(ServiceCommand::Connect {
                    addr,
                    password: None,
                })
            }
            PanelFocus::Saved if !self.saved_peers.is_empty() => {
                let addr = self.saved_peers[self.saved_peer_index].addr;
                Some(ServiceCommand::Connect {
                    addr,
                    password: None,
                })
            }
            _ => None,
        };
        if command.is_some() {
            self.exit_panel_focus();
            self.status_line = "Connecting…".into();
        }
        command
    }

    fn move_selection(&mut self, delta: isize) {
        if self.messages.is_empty() {
            self.selected_message = None;
            self.chat_focus = false;
            return;
        }
        let len = self.messages.len() as isize;
        let current = self
            .selected_message
            .map(|idx| idx as isize)
            .unwrap_or(len.saturating_sub(1));
        let mut next = current.saturating_add(delta);
        if next < 0 {
            next = 0;
        }
        if next >= len {
            next = len - 1;
        }
        self.selected_message = Some(next as usize);
    }

    fn copy_selected_message(&mut self) {
        if !self.chat_focus {
            self.show_warning("Press Ctrl+G to browse chat first.");
            return;
        }
        if self.messages.is_empty() {
            self.show_warning("No messages to copy.");
            return;
        }
        let mut targets: Vec<usize> = if self.marked_messages.is_empty() {
            match self.selected_message {
                Some(idx) => vec![idx],
                None => {
                    self.show_warning("No message selected.");
                    return;
                }
            }
        } else {
            self.marked_messages.iter().copied().collect()
        };
        targets.sort_unstable();
        targets.dedup();
        let mut payloads: Vec<String> = Vec::new();
        for idx in targets.into_iter() {
            if let Some(entry) = self.messages.get(idx) {
                payloads.push(entry.text.clone());
            }
        }
        if payloads.is_empty() {
            self.show_warning("Selected messages are unavailable.");
            self.clamp_selection();
            return;
        }
        let payload = payloads.join("\n");
        match Clipboard::new().and_then(|mut clip| clip.set_text(payload)) {
            Ok(_) => {
                if self.marked_messages.is_empty() {
                    if let Some(idx) = self.selected_message {
                        if let Some(entry) = self.messages.get(idx) {
                            self.status_line = format!("Copied message from {}", entry.author);
                        } else {
                            self.status_line = "Copied message".into();
                        }
                    } else {
                        self.status_line = "Copied message".into();
                    }
                } else {
                    self.status_line = format!(
                        "Copied {} marked message{}",
                        self.marked_messages.len(),
                        if self.marked_messages.len() == 1 { "" } else { "s" }
                    );
                }
            }
            Err(err) => {
                self.show_error(format!("Clipboard error: {err}"));
            }
        }
    }

    fn enqueue_offer(&mut self, offer: FileOfferNotice) {
        if self.active_offer.is_none() {
            self.activate_offer(offer);
        } else {
            self.offer_queue.push_back(offer);
            self.status_line = format!("Queued {} pending transfer(s)", self.offer_queue.len());
        }
    }

    fn activate_offer(&mut self, offer: FileOfferNotice) {
        let suggested = self.download_dir.join(&offer.name);
        self.input = suggested.to_string_lossy().to_string();
        self.mode = Mode::IncomingFile(offer.id);
        self.active_offer = Some(offer.clone());
        self.status_line = format!(
            "Incoming '{}' from {} ({}). Edit path & Enter to accept, Esc to decline.",
            offer.name,
            offer.peer,
            human_size(offer.original_size)
        );
    }

    fn mark_offer_handled(&mut self) {
        self.active_offer = None;
        self.input.clear();
        self.mode = Mode::Chat;
        if let Some(next) = self.offer_queue.pop_front() {
            self.activate_offer(next);
        }
    }

    pub fn panel_focus(&self) -> PanelFocus {
        self.panel_focus
    }

    pub fn selected_discovered(&self) -> Option<usize> {
        if self.discovered.is_empty() {
            None
        } else {
            Some(
                self.selected_peer
                    .min(self.discovered.len().saturating_sub(1)),
            )
        }
    }

    pub fn selected_saved(&self) -> Option<usize> {
        if self.saved_peers.is_empty() {
            None
        } else {
            Some(
                self.saved_peer_index
                    .min(self.saved_peers.len().saturating_sub(1)),
            )
        }
    }

    fn autocomplete_path(&mut self) {
        let raw = self.input.trim();
        let sep = std::path::MAIN_SEPARATOR;
        let (dir, prefix) = self.split_path_for_completion(raw);
        let read_dir = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(err) => {
                self.show_error(format!("Cannot read {}: {err}", dir.display()));
                return;
            }
        };
        let mut matches: Vec<(String, bool)> = Vec::new();
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !prefix.is_empty() && !name.starts_with(&prefix) {
                continue;
            }
            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            matches.push((name, is_dir));
        }
        if matches.is_empty() {
            self.status_line = format!("No entries matching '{}' in {}", prefix, dir.display());
            return;
        }
        matches.sort_by(|a, b| a.0.cmp(&b.0));
        let base = if dir == Path::new(".") {
            PathBuf::new()
        } else {
            dir.clone()
        };
        if matches.len() == 1 {
            let (name, is_dir) = &matches[0];
            let candidate = if base.as_os_str().is_empty() {
                PathBuf::from(name)
            } else {
                base.join(name)
            };
            let mut rendered = candidate.to_string_lossy().to_string();
            if *is_dir && !rendered.ends_with(sep) {
                rendered.push(sep);
            }
            self.input = rendered;
            self.status_line = if *is_dir {
                format!("Completed directory {}", name)
            } else {
                format!("Selected file {}", name)
            };
            return;
        }

        let lcp = longest_common_prefix(
            &matches
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>(),
        );
        if lcp.len() > prefix.len() {
            let candidate = if base.as_os_str().is_empty() {
                PathBuf::from(&lcp)
            } else {
                base.join(&lcp)
            };
            let rendered = candidate.to_string_lossy().to_string();
            self.input = rendered;
        }
        let preview = matches
            .iter()
            .take(6)
            .map(|(name, is_dir)| {
                if *is_dir {
                    format!("{name}/")
                } else {
                    name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        self.status_line = format!(
            "Options: {preview}{}",
            if matches.len() > 6 { " …" } else { "" }
        );
    }

    fn split_path_for_completion(&self, raw: &str) -> (PathBuf, String) {
        if raw.is_empty() {
            return (PathBuf::from("."), String::new());
        }
        let sep = std::path::MAIN_SEPARATOR;
        let hint_dir = raw.ends_with(sep);
        let path = PathBuf::from(raw);
        if hint_dir {
            return (path, String::new());
        }
        if path.is_dir() {
            return (path, String::new());
        }
        let prefix = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_string();
        let parent = path
            .parent()
            .map(|p| {
                if p.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    p.to_path_buf()
                }
            })
            .unwrap_or_else(|| PathBuf::from("."));
        (parent, prefix)
    }

    fn commit_input(&mut self) -> Option<ServiceCommand> {
        match self.mode {
            Mode::Chat => {
                if self.input.trim().is_empty() {
                    self.show_warning("Cannot send empty message");
                    return None;
                }
                let text = self.input.clone();
                self.input.clear();
                return Some(ServiceCommand::SendText { text });
            }
            Mode::File => {
                if self.input.trim().is_empty() {
                    self.show_warning("Provide a file path");
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
                Err(_) => self.show_warning("Invalid listen address"),
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
                Err(_) => self.show_warning("Invalid peer address"),
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
                    self.show_warning("Nickname cannot be empty");
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
                            self.show_warning("Enter a valid IPv4, e.g. 192.168.0.255");
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
            Mode::IncomingFile(id) => {
                let trimmed = self.input.trim();
                if trimmed.is_empty() {
                    self.show_warning("Choose a path to save the file");
                    return None;
                }
                let path = PathBuf::from(trimmed);
                self.input.clear();
                self.mode = Mode::Chat;
                self.status_line = "Preparing to receive file…".into();
                self.mark_offer_handled();
                return Some(ServiceCommand::AcceptFile { id, path });
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
            ServiceEvent::SavedPeers(list) => {
                self.saved_peers = list;
                if self.saved_peers.is_empty() {
                    self.saved_peer_index = 0;
                } else {
                    self.saved_peer_index = self
                        .saved_peer_index
                        .min(self.saved_peers.len().saturating_sub(1));
                }
            }
            ServiceEvent::FileOffer(offer) => {
                self.push_system(format!(
                    "Incoming file '{}' ({}) from {}",
                    offer.name,
                    human_size(offer.original_size),
                    offer.peer
                ));
                self.enqueue_offer(offer);
            }
            ServiceEvent::Error { message } => {
                self.show_error(message);
            }
        }
    }

    fn update_transfer(&mut self, progress: FileTransferProgress) {
        let mut was_completed = false;
        if let Some(existing) = self.transfers.iter_mut().find(|t| t.id == progress.id) {
            was_completed = existing.completed;
            existing.transferred = progress.transferred;
            existing.total = progress.total;
            existing.completed = progress.completed;
            existing.path = progress.path.clone();
        } else {
            self.transfers.push(TransferState {
                id: progress.id,
                name: progress.name.clone(),
                direction: progress.direction,
                transferred: progress.transferred,
                total: progress.total,
                path: progress.path.clone(),
                completed: progress.completed,
            });
        }
        if progress.completed && !was_completed {
            let size = human_size(progress.total.max(progress.transferred));
            let where_to = progress
                .path
                .as_ref()
                .map(|p| format!(" at {}", p.display()))
                .unwrap_or_default();
            let verb = match progress.direction {
                TransferDirection::Incoming => "Received",
                TransferDirection::Outgoing => "Sent",
            };
            self.push_system(format!("{verb} '{}' ({size}){where_to}", progress.name));
        }
    }

    fn push_message(&mut self, direction: MessageDirection, text: String) {
        if self.messages.len() >= MAX_MESSAGES {
            self.messages.remove(0);
            if let Some(idx) = self.selected_message {
                if idx == 0 {
                    self.selected_message = None;
                } else {
                    self.selected_message = Some(idx - 1);
                }
            }
            self.marked_messages = self
                .marked_messages
                .iter()
                .filter_map(|idx| idx.checked_sub(1))
                .collect();
        }
        let author = direction.source().to_string();
        self.messages.push(ChatEntry {
            direction,
            author,
            text,
            timestamp: OffsetDateTime::now_utc(),
        });
        self.clamp_selection();
    }

    pub fn peer_alias(&self, addr: &SocketAddr) -> Option<&String> {
        self.peer_names.get(addr)
    }

    pub fn bind_address(&self) -> SocketAddr {
        self.default_bind
    }

    fn push_system(&mut self, text: impl Into<String>) {
        self.push_message(MessageDirection::System, text.into());
    }

    fn push_warning(&mut self, text: impl Into<String>) {
        self.push_message(MessageDirection::Warning, text.into());
    }

    fn push_error(&mut self, text: impl Into<String>) {
        self.push_message(MessageDirection::Error, text.into());
    }

    fn show_warning(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.status_line = text.clone();
        self.push_warning(text);
    }

    fn show_error(&mut self, text: impl Into<String>) {
        let text = text.into();
        let preview = text
            .lines()
            .next()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .map(|line| line.to_string());
        self.status_line = preview
            .map(|line| format!("[x] {line}"))
            .unwrap_or_else(|| "[x] Error".into());
        self.push_error(text);
    }

    fn clamp_selection(&mut self) {
        if self.messages.is_empty() {
            self.selected_message = None;
            self.marked_messages.clear();
            return;
        }
        if let Some(idx) = self.selected_message {
            if idx >= self.messages.len() {
                self.selected_message = Some(self.messages.len().saturating_sub(1));
            }
        }
        let len = self.messages.len();
        self.marked_messages.retain(|idx| *idx < len);
    }

    fn toggle_marked_message(&mut self) {
        if !self.chat_focus {
            self.show_warning("Press Ctrl+G to browse chat first.");
            return;
        }
        let Some(idx) = self.selected_message else {
            self.show_warning("No message selected.");
            return;
        };
        let Some(entry) = self.messages.get(idx) else {
            self.show_warning("Selected message is unavailable.");
            return;
        };
        if self.marked_messages.remove(&idx) {
            self.status_line = format!("Unmarked message from {}", entry.author);
        } else {
            self.marked_messages.insert(idx);
            self.status_line = format!("Marked message from {}", entry.author);
        }
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
    IncomingFile(u64),
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
    Warning,
    Error,
}

impl MessageDirection {
    pub fn style(&self) -> Color {
        match self {
            MessageDirection::Incoming(_) => Color::LightCyan,
            MessageDirection::Outgoing(_) => Color::LightGreen,
            MessageDirection::System => Color::Gray,
            MessageDirection::Warning => Color::Yellow,
            MessageDirection::Error => Color::LightRed,
        }
    }

    fn source(&self) -> &str {
        match self {
            MessageDirection::Incoming(name) => name,
            MessageDirection::Outgoing(name) => name,
            MessageDirection::System => "system",
            MessageDirection::Warning => "warning",
            MessageDirection::Error => "error",
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

fn longest_common_prefix(strings: &[String]) -> String {
    if strings.is_empty() {
        return String::new();
    }
    let mut prefix = strings[0].clone();
    for item in strings.iter().skip(1) {
        while !item.starts_with(&prefix) {
            if prefix.is_empty() {
                return prefix;
            }
            prefix.pop();
        }
    }
    prefix
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let exp = (bytes as f64).log(1024.0).floor() as usize;
    let idx = exp.min(UNITS.len() - 1);
    let value = bytes as f64 / 1024f64.powi(idx as i32);
    if idx == 0 {
        format!("{bytes} {}", UNITS[idx])
    } else {
        format!("{value:.1} {}", UNITS[idx])
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
