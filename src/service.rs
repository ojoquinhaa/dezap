use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use chacha20poly1305::aead::{generic_array::GenericArray, Aead, KeyInit};
use chacha20poly1305::ChaCha20Poly1305;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::{spawn_blocking, JoinHandle};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::cli::{ListenCommand, SendCommand, SendFileCommand};
use crate::config::{AppConfig, LimitsConfig};
use crate::net;
use crate::protocol::{
    self, CipherFrame, ControlMessage, FileAccept, FileChunk, FileMetadata, FileOffer, FileReject,
    HelloMessage, TextMessage, WireMessage,
};
use parking_lot::Mutex;
use tempfile::NamedTempFile;

const COMMAND_BUFFER: usize = 64;
const EVENT_BUFFER: usize = 256;

/// High-level command channel to the async runtime.
pub struct DezapService {
    cmd_tx: mpsc::Sender<ServiceCommand>,
    event_rx: mpsc::Receiver<ServiceEvent>,
}

impl DezapService {
    /// Spawns the background runtime.
    pub fn new(config: AppConfig) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_BUFFER);
        let (event_tx, event_rx) = mpsc::channel(EVENT_BUFFER);
        tokio::spawn(runtime_loop(config, cmd_rx, event_tx.clone()));
        Self { cmd_tx, event_rx }
    }

    /// Sends a command to the runtime.
    pub async fn send(&self, command: ServiceCommand) -> Result<()> {
        self.cmd_tx
            .send(command)
            .await
            .map_err(|_| anyhow!("dezap service runtime stopped"))
    }

    /// Blocking receive for the next service event.
    pub async fn next_event(&mut self) -> Option<ServiceEvent> {
        self.event_rx.recv().await
    }

    /// Cloned sender for concurrent tasks (e.g. TUI).
    pub fn command_sender(&self) -> mpsc::Sender<ServiceCommand> {
        self.cmd_tx.clone()
    }
}

/// Commands that drive the networking service.
#[derive(Debug)]
pub enum ServiceCommand {
    Listen {
        addr: std::net::SocketAddr,
        password: Option<String>,
    },
    StopListening,
    Connect {
        addr: std::net::SocketAddr,
        password: Option<String>,
    },
    Disconnect,
    SendText {
        text: String,
    },
    SendFile {
        path: PathBuf,
    },
    Discover,
    SetUsername {
        username: String,
    },
    SetDiscoveryTarget {
        target: Option<Ipv4Addr>,
    },
    AcceptFile {
        id: u64,
        path: PathBuf,
    },
    DeclineFile {
        id: u64,
    },
}

/// Events emitted by the service to inform the UI/CLI.
#[derive(Debug, Clone)]
pub enum ServiceEvent {
    Connected {
        peer: std::net::SocketAddr,
        name: String,
    },
    Connecting {
        peer: std::net::SocketAddr,
    },
    Listening {
        addr: std::net::SocketAddr,
        password_protected: bool,
    },
    ListenerStopped,
    Disconnected,
    MessageReceived {
        peer: std::net::SocketAddr,
        author: String,
        text: String,
    },
    MessageSent {
        author: String,
        text: String,
    },
    PeerProfile {
        peer: std::net::SocketAddr,
        username: String,
    },
    FileTransfer(FileTransferProgress),
    Discovery(DiscoveryEvent),
    SavedPeers(Vec<SavedPeer>),
    FileOffer(FileOfferNotice),
    Error {
        message: String,
    },
}

/// Transfer progress payload.
#[derive(Debug, Clone)]
pub struct FileTransferProgress {
    pub id: u64,
    pub name: String,
    pub transferred: u64,
    pub total: u64,
    pub direction: TransferDirection,
    pub path: Option<PathBuf>,
    pub completed: bool,
}

/// Transfer direction.
#[derive(Debug, Clone, Copy)]
pub enum TransferDirection {
    Incoming,
    Outgoing,
}

/// Peer discovery events.
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    PeerFound(std::net::SocketAddr),
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPeer {
    pub addr: std::net::SocketAddr,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct FileOfferNotice {
    pub id: u64,
    pub name: String,
    pub original_size: u64,
    pub compressed_size: u64,
    pub peer: std::net::SocketAddr,
}

/// Runs the service runtime loop.
async fn runtime_loop(
    config: AppConfig,
    mut cmd_rx: mpsc::Receiver<ServiceCommand>,
    event_tx: mpsc::Sender<ServiceEvent>,
) {
    let (internal_tx, mut internal_rx) = mpsc::channel(32);
    let history = match HistoryWriter::new(config.paths.history_dir.clone()) {
        Ok(writer) => Arc::new(writer),
        Err(err) => {
            let _ = event_tx
                .send(ServiceEvent::Error {
                    message: format!("failed to initialize history store: {err:#}"),
                })
                .await;
            return;
        }
    };
    let peers_store = match SavedPeersStore::new(config.paths.peers_file.clone()) {
        Ok(store) => Arc::new(store),
        Err(err) => {
            let _ = event_tx
                .send(ServiceEvent::Error {
                    message: format!("failed to load peers: {err:#}"),
                })
                .await;
            return;
        }
    };
    let mut state = ServiceState::new(
        config,
        event_tx.clone(),
        internal_tx.clone(),
        history,
        peers_store.clone(),
    );
    let _ = event_tx
        .send(ServiceEvent::SavedPeers(peers_store.list()))
        .await;

    loop {
        tokio::select! {
            Some(cmd) = cmd_rx.recv() => {
                if let Err(err) = state.handle_command(cmd).await {
                    let _ = event_tx.send(ServiceEvent::Error { message: format!("{err:#}") }).await;
                }
            }
            Some(signal) = internal_rx.recv() => {
                if let Err(err) = state.handle_internal(signal).await {
                    let _ = event_tx.send(ServiceEvent::Error { message: format!("{err:#}") }).await;
                }
            }
            else => break,
        }
    }

    state.shutdown().await;
}

struct ServiceState {
    config: AppConfig,
    event_tx: mpsc::Sender<ServiceEvent>,
    internal_tx: mpsc::Sender<InternalSignal>,
    listener: Option<ListenerState>,
    client: Option<ClientState>,
    connection: Option<ConnectionState>,
    username: String,
    listener_password: Option<String>,
    discovery_override: Option<Ipv4Addr>,
    history: Arc<HistoryWriter>,
    peers: Arc<SavedPeersStore>,
    pending_transfers: Arc<Mutex<HashMap<u64, PreparedTransfer>>>,
    incoming_offers: Arc<Mutex<HashMap<u64, FileOfferNotice>>>,
    incoming_transfers: Arc<Mutex<HashMap<u64, IncomingTransfer>>>,
}

impl ServiceState {
    fn new(
        config: AppConfig,
        event_tx: mpsc::Sender<ServiceEvent>,
        internal_tx: mpsc::Sender<InternalSignal>,
        history: Arc<HistoryWriter>,
        peers: Arc<SavedPeersStore>,
    ) -> Self {
        let username = config.identity.username.clone();
        let listener_password = config.listen.password.clone();
        let pending_transfers = Arc::new(Mutex::new(HashMap::new()));
        let incoming_offers = Arc::new(Mutex::new(HashMap::new()));
        let incoming_transfers = Arc::new(Mutex::new(HashMap::new()));
        Self {
            config,
            event_tx,
            internal_tx,
            listener: None,
            client: None,
            connection: None,
            username,
            listener_password,
            discovery_override: None,
            history,
            peers,
            pending_transfers,
            incoming_offers,
            incoming_transfers,
        }
    }

    async fn handle_command(&mut self, command: ServiceCommand) -> Result<()> {
        match command {
            ServiceCommand::Listen { addr, password } => self.start_listener(addr, password).await,
            ServiceCommand::StopListening => self.stop_listener().await,
            ServiceCommand::Connect { addr, password } => self.connect(addr, password).await,
            ServiceCommand::Disconnect => self.disconnect().await,
            ServiceCommand::SendText { text } => self.send_text(text).await,
            ServiceCommand::SendFile { path } => self.send_file(path).await,
            ServiceCommand::Discover => self.run_discovery().await,
            ServiceCommand::SetUsername { username } => {
                self.username = username;
                Ok(())
            }
            ServiceCommand::SetDiscoveryTarget { target } => {
                self.discovery_override = target;
                Ok(())
            }
            ServiceCommand::AcceptFile { id, path } => self.accept_file(id, path).await,
            ServiceCommand::DeclineFile { id } => self.decline_file(id).await,
        }
    }

    async fn handle_internal(&mut self, signal: InternalSignal) -> Result<()> {
        match signal {
            InternalSignal::Inbound(connection, peer) => {
                let required = self.listener_password.clone();
                self.attach_connection(connection, peer, None, required)
                    .await
            }
            InternalSignal::ConnectionClosed(peer) => {
                if let Some(state) = &self.connection {
                    if state.peer == peer {
                        self.connection = None;
                        self.event_tx.send(ServiceEvent::Disconnected).await.ok();
                    }
                }
                Ok(())
            }
        }
    }

    async fn start_listener(
        &mut self,
        addr: std::net::SocketAddr,
        password: Option<String>,
    ) -> Result<()> {
        if self.listener.is_some() {
            bail!("listener already active");
        }

        let server = net::bind_server(addr, &self.config.tls)?;
        let discovery = net::spawn_discovery_responder(addr, &self.config.discovery).await?;
        self.listener_password = password
            .clone()
            .or_else(|| self.config.listen.password.clone());
        let endpoint = server.endpoint.clone();
        let internal = self.internal_tx.clone();
        let incoming_task = tokio::spawn(async move {
            loop {
                match endpoint.accept().await {
                    Some(incoming) => match incoming.await {
                        Ok(connection) => {
                            let peer = connection.remote_address();
                            if internal
                                .send(InternalSignal::Inbound(connection, peer))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(err) => {
                            tracing::warn!(%err, "incoming connection failed");
                        }
                    },
                    None => break,
                }
            }
        });

        self.listener = Some(ListenerState {
            endpoint: server.endpoint,
            client_config: server.client_config,
            incoming_task,
            discovery_task: discovery,
        });

        self.event_tx
            .send(ServiceEvent::Listening {
                addr,
                password_protected: self.listener_password.is_some(),
            })
            .await
            .ok();
        Ok(())
    }

    async fn stop_listener(&mut self) -> Result<()> {
        if let Some(listener) = self.listener.take() {
            listener.endpoint.close(0u32.into(), b"listener stopped");
            listener.incoming_task.abort();
            if let Some(task) = listener.discovery_task {
                task.abort();
            }
            self.event_tx.send(ServiceEvent::ListenerStopped).await.ok();
        }
        Ok(())
    }

    async fn connect(
        &mut self,
        addr: std::net::SocketAddr,
        password: Option<String>,
    ) -> Result<()> {
        self.event_tx
            .send(ServiceEvent::Connecting { peer: addr })
            .await
            .ok();
        self.disconnect().await?;
        let server_name = self.config.tls.server_name().to_string();
        let client = self.client_endpoint()?;
        let connection =
            net::connect(client.endpoint, &client.client_config, addr, &server_name).await?;
        self.attach_connection(connection, addr, password, None)
            .await
    }

    async fn disconnect(&mut self) -> Result<()> {
        if let Some(connection) = self.connection.take() {
            connection
                .connection
                .close(0u32.into(), b"manual disconnect");
            connection.reader.abort();
            self.event_tx.send(ServiceEvent::Disconnected).await.ok();
        }
        Ok(())
    }

    async fn send_text(&mut self, text: String) -> Result<()> {
        let state = self
            .connection
            .as_ref()
            .ok_or_else(|| anyhow!("no active connection"))?;
        let connection = state.connection.clone();
        let meta = state.meta.clone();
        send_text_message(
            &connection,
            state.peer,
            &self.username,
            text.clone(),
            &self.config.limits,
            &self.event_tx,
            meta,
            self.history.clone(),
        )
        .await?;
        persist_chat(
            self.config.paths.chat_log.clone(),
            format!("{} (you): {}", self.username, text),
        )
        .await?;
        Ok(())
    }

    async fn send_file(&mut self, path: PathBuf) -> Result<()> {
        let state = self
            .connection
            .as_ref()
            .ok_or_else(|| anyhow!("no active connection"))?;
        let prepared = prepare_transfer(path.clone(), &self.config.limits).await?;
        let offer = prepared.offer.clone();
        {
            let mut pending = self.pending_transfers.lock();
            pending.insert(offer.id, prepared);
        }
        send_control_message(
            &state.connection,
            ControlMessage::FileOffer(FileOffer {
                id: offer.id,
                name: offer.name.clone(),
                original_size: offer.original_size,
                compressed_size: offer.compressed_size,
            }),
        )
        .await?;
        self.event_tx
            .send(ServiceEvent::FileTransfer(FileTransferProgress {
                id: offer.id,
                name: offer.name,
                transferred: 0,
                total: offer.compressed_size,
                direction: TransferDirection::Outgoing,
                path: Some(path),
                completed: false,
            }))
            .await
            .ok();
        Ok(())
    }

    async fn run_discovery(&mut self) -> Result<()> {
        let peers = net::discover_peers(&self.config.discovery, self.discovery_override).await?;
        if peers.is_empty() {
            self.event_tx
                .send(ServiceEvent::Discovery(DiscoveryEvent::Completed))
                .await
                .ok();
        } else {
            for peer in peers {
                self.event_tx
                    .send(ServiceEvent::Discovery(DiscoveryEvent::PeerFound(peer)))
                    .await
                    .ok();
            }
            self.event_tx
                .send(ServiceEvent::Discovery(DiscoveryEvent::Completed))
                .await
                .ok();
        }
        Ok(())
    }

    async fn accept_file(&mut self, id: u64, requested: PathBuf) -> Result<()> {
        let state = self
            .connection
            .as_ref()
            .ok_or_else(|| anyhow!("no active connection"))?;
        let offer = {
            let mut pending = self.incoming_offers.lock();
            pending.remove(&id)
        }
        .ok_or_else(|| anyhow!("no pending offer for id {id}"))?;
        let mut target = requested;
        let hint_dir = target
            .to_string_lossy()
            .ends_with(std::path::MAIN_SEPARATOR);
        if hint_dir || target.is_dir() {
            target = target.join(&offer.name);
        }
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        let temp_path = spawn_blocking(|| -> Result<PathBuf> {
            let path = NamedTempFile::new()
                .context("failed to create temp file")?
                .into_temp_path()
                .keep()
                .context("failed to persist temp receive file")?;
            Ok(path)
        })
        .await??;

        {
            let mut incoming = self.incoming_transfers.lock();
            incoming.insert(
                id,
                IncomingTransfer {
                    target_path: target.clone(),
                    temp_path,
                    original_name: offer.name.clone(),
                },
            );
        }
        send_control_message(
            &state.connection,
            ControlMessage::FileAccept(FileAccept { id }),
        )
        .await?;
        self.event_tx
            .send(ServiceEvent::FileTransfer(FileTransferProgress {
                id,
                name: offer.name.clone(),
                transferred: 0,
                total: offer.compressed_size,
                direction: TransferDirection::Incoming,
                path: Some(target.clone()),
                completed: false,
            }))
            .await
            .ok();
        Ok(())
    }

    async fn decline_file(&mut self, id: u64) -> Result<()> {
        let state = self
            .connection
            .as_ref()
            .ok_or_else(|| anyhow!("no active connection"))?;
        let existed = {
            let mut pending = self.incoming_offers.lock();
            pending.remove(&id)
        };
        if existed.is_some() {
            send_control_message(
                &state.connection,
                ControlMessage::FileReject(FileReject {
                    id,
                    reason: Some("Recipient declined".into()),
                }),
            )
            .await?;
        }
        Ok(())
    }

    async fn attach_connection(
        &mut self,
        connection: quinn::Connection,
        peer: std::net::SocketAddr,
        outgoing_password: Option<String>,
        required_password: Option<String>,
    ) -> Result<()> {
        self.disconnect().await?;
        let event_tx = self.event_tx.clone();
        let chat_log = self.config.paths.chat_log.clone();
        let internal = self.internal_tx.clone();
        let reader_connection = connection.clone();
        let meta = ConnectionMeta::new("???");
        let peer_ctx = PeerContext {
            required_password: required_password.clone(),
            meta: meta.clone(),
            history: self.history.clone(),
            peers: self.peers.clone(),
            pending_transfers: self.pending_transfers.clone(),
            incoming_offers: self.incoming_offers.clone(),
            incoming_transfers: self.incoming_transfers.clone(),
            limits: self.config.limits.clone(),
        };
        let reader = tokio::spawn(async move {
            if let Err(err) =
                read_connection(reader_connection, chat_log, event_tx.clone(), peer_ctx).await
            {
                let _ = event_tx
                    .send(ServiceEvent::Error {
                        message: format!("connection reader error: {err:#}"),
                    })
                    .await;
            }
            let _ = internal.send(InternalSignal::ConnectionClosed(peer)).await;
        });

        self.connection = Some(ConnectionState {
            peer,
            connection: connection.clone(),
            reader,
            meta: meta.clone(),
        });
        self.event_tx
            .send(ServiceEvent::Connected {
                peer,
                name: meta.name(),
            })
            .await
            .ok();
        send_hello(
            &connection,
            &self.username,
            outgoing_password,
            meta.public_key(),
        )
        .await
        .ok();
        Ok(())
    }

    fn client_endpoint(&mut self) -> Result<ClientEndpoint<'_>> {
        if let Some(listener) = &self.listener {
            return Ok(ClientEndpoint {
                endpoint: &listener.endpoint,
                client_config: listener.client_config.clone(),
            });
        }

        if self.client.is_none() {
            let ctx = net::build_client_endpoint(self.config.listen.bind_addr, &self.config.tls)?;
            self.client = Some(ClientState {
                endpoint: ctx.endpoint,
                client_config: ctx.client_config,
            });
        }

        let client = self.client.as_ref().expect("client endpoint initialized");
        Ok(ClientEndpoint {
            endpoint: &client.endpoint,
            client_config: client.client_config.clone(),
        })
    }

    async fn shutdown(&mut self) {
        self.disconnect().await.ok();
        self.stop_listener().await.ok();
    }
}

struct ClientEndpoint<'a> {
    endpoint: &'a quinn::Endpoint,
    client_config: quinn::ClientConfig,
}

struct ListenerState {
    endpoint: quinn::Endpoint,
    client_config: quinn::ClientConfig,
    incoming_task: JoinHandle<()>,
    discovery_task: Option<JoinHandle<()>>,
}

struct ClientState {
    endpoint: quinn::Endpoint,
    client_config: quinn::ClientConfig,
}

struct ConnectionState {
    peer: std::net::SocketAddr,
    connection: quinn::Connection,
    reader: JoinHandle<()>,
    meta: ConnectionMeta,
}

enum InternalSignal {
    Inbound(quinn::Connection, std::net::SocketAddr),
    ConnectionClosed(std::net::SocketAddr),
}

#[derive(Clone)]
struct ConnectionMeta {
    name: Arc<Mutex<String>>,
    crypto: Arc<CryptoCtx>,
}

impl ConnectionMeta {
    fn new(initial: &str) -> Self {
        Self {
            name: Arc::new(Mutex::new(initial.to_string())),
            crypto: Arc::new(CryptoCtx::new()),
        }
    }

    fn set_name(&self, value: &str) {
        *self.name.lock() = value.to_string();
    }

    fn name(&self) -> String {
        self.name.lock().clone()
    }

    fn public_key(&self) -> [u8; 32] {
        self.crypto.public_key()
    }

    fn derive(&self, remote: &[u8]) -> Result<bool> {
        self.crypto.accept_remote(remote)
    }

    fn shared_key(&self) -> Option<[u8; 32]> {
        self.crypto.shared_key()
    }
}

#[derive(Clone)]
struct PeerContext {
    required_password: Option<String>,
    meta: ConnectionMeta,
    history: Arc<HistoryWriter>,
    peers: Arc<SavedPeersStore>,
    pending_transfers: Arc<Mutex<HashMap<u64, PreparedTransfer>>>,
    incoming_offers: Arc<Mutex<HashMap<u64, FileOfferNotice>>>,
    incoming_transfers: Arc<Mutex<HashMap<u64, IncomingTransfer>>>,
    limits: LimitsConfig,
}

struct CryptoCtx {
    inner: Mutex<CryptoState>,
}

struct CryptoState {
    secret: StaticSecret,
    public: PublicKey,
    shared: Option<[u8; 32]>,
}

impl CryptoCtx {
    fn new() -> Self {
        let secret = StaticSecret::new(OsRng);
        let public = PublicKey::from(&secret);
        Self {
            inner: Mutex::new(CryptoState {
                secret,
                public,
                shared: None,
            }),
        }
    }

    fn public_key(&self) -> [u8; 32] {
        self.inner.lock().public.to_bytes()
    }

    fn accept_remote(&self, remote: &[u8]) -> Result<bool> {
        let mut inner = self.inner.lock();
        if inner.shared.is_some() {
            return Ok(false);
        }
        if remote.len() != 32 {
            bail!("invalid remote public key");
        }
        let mut buf = [0u8; 32];
        buf.copy_from_slice(remote);
        let remote = PublicKey::from(buf);
        let shared = inner.secret.diffie_hellman(&remote);
        inner.shared = Some(shared.to_bytes());
        Ok(true)
    }

    fn shared_key(&self) -> Option<[u8; 32]> {
        self.inner.lock().shared
    }
}

/// Runs a headless listener in CLI mode.
pub async fn run_listener(config: &AppConfig, cmd: ListenCommand) -> Result<()> {
    let addr = cmd.bind.unwrap_or(config.listen.bind_addr);
    let mut service = DezapService::new(config.clone());
    service
        .send(ServiceCommand::Listen {
            addr,
            password: cmd
                .password
                .clone()
                .or_else(|| config.listen.password.clone()),
        })
        .await?;

    tracing::info!(%addr, "listener ready");
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("ctrl-c received, shutting down listener");
                service.send(ServiceCommand::StopListening).await.ok();
                break;
            }
            event = service.next_event() => {
                if let Some(event) = event {
                    tracing::info!(?event, "service event");
                } else {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// CLI helper to send a one-off text message.
pub async fn run_cli_message(config: &AppConfig, cmd: SendCommand) -> Result<()> {
    let mut service = DezapService::new(config.clone());
    service
        .send(ServiceCommand::Connect {
            addr: cmd.to,
            password: None,
        })
        .await?;
    let mut sent = false;
    while let Some(event) = service.next_event().await {
        match event {
            ServiceEvent::Connected { .. } => {
                service
                    .send(ServiceCommand::SendText {
                        text: cmd.text.clone(),
                    })
                    .await?;
            }
            ServiceEvent::MessageSent { .. } => {
                sent = true;
                service.send(ServiceCommand::Disconnect).await.ok();
                break;
            }
            ServiceEvent::Error { message } => bail!(message),
            ServiceEvent::Disconnected => break,
            _ => {}
        }
    }
    if !sent {
        bail!("message delivery incomplete");
    }
    Ok(())
}

/// CLI helper for file transfers.
pub async fn run_cli_file_send(config: &AppConfig, cmd: SendFileCommand) -> Result<()> {
    let mut service = DezapService::new(config.clone());
    service
        .send(ServiceCommand::Connect {
            addr: cmd.to,
            password: None,
        })
        .await?;
    let mut completed = false;
    while let Some(event) = service.next_event().await {
        match event {
            ServiceEvent::Connected { .. } => {
                service
                    .send(ServiceCommand::SendFile {
                        path: cmd.path.clone(),
                    })
                    .await?;
            }
            ServiceEvent::FileTransfer(progress) if progress.completed => {
                completed = true;
                service.send(ServiceCommand::Disconnect).await.ok();
                break;
            }
            ServiceEvent::Error { message } => bail!(message),
            ServiceEvent::Disconnected => break,
            _ => {}
        }
    }
    if !completed {
        bail!("file transfer incomplete");
    }
    Ok(())
}

async fn send_text_message(
    connection: &quinn::Connection,
    peer: std::net::SocketAddr,
    author: &str,
    text: String,
    limits: &LimitsConfig,
    event_tx: &mpsc::Sender<ServiceEvent>,
    meta: ConnectionMeta,
    history: Arc<HistoryWriter>,
) -> Result<()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("empty messages are ignored");
    }
    if trimmed.len() > limits.max_message_bytes {
        bail!("message length exceeds configured limit");
    }
    let text_payload = TextMessage {
        id: rand::random(),
        author: author.to_string(),
        body: trimmed.to_string(),
        timestamp: protocol::utc_timestamp(),
    };
    let encrypted = encrypt_text(&meta, &text_payload)?;
    let mut stream = connection
        .open_uni()
        .await
        .context("failed opening unidirectional stream")?;
    protocol::write_message(&mut stream, &encrypted).await?;
    let _ = stream.finish();
    event_tx
        .send(ServiceEvent::MessageSent {
            author: author.to_string(),
            text: trimmed.to_string(),
        })
        .await
        .ok();
    history
        .record(
            peer,
            HistoryEntry {
                timestamp: protocol::utc_timestamp(),
                outgoing: true,
                author: author.to_string(),
                text: trimmed.to_string(),
            },
        )
        .ok();
    Ok(())
}

async fn send_hello(
    connection: &quinn::Connection,
    username: &str,
    password: Option<String>,
    public_key: [u8; 32],
) -> Result<()> {
    let mut stream = connection
        .open_uni()
        .await
        .context("failed to open control stream")?;
    let message = WireMessage::Control(ControlMessage::Hello(HelloMessage {
        username: username.to_string(),
        password,
        public_key,
    }));
    protocol::write_message(&mut stream, &message).await?;
    let _ = stream.finish();
    Ok(())
}

async fn send_control_message(
    connection: &quinn::Connection,
    message: ControlMessage,
) -> Result<()> {
    let mut stream = connection
        .open_uni()
        .await
        .context("failed to open control stream")?;
    protocol::write_message(&mut stream, &WireMessage::Control(message)).await?;
    let _ = stream.finish();
    Ok(())
}

fn encrypt_text(meta: &ConnectionMeta, message: &TextMessage) -> Result<WireMessage> {
    let key = meta
        .shared_key()
        .ok_or_else(|| anyhow!("secure channel not established yet"))?;
    let cipher = ChaCha20Poly1305::new(GenericArray::from_slice(&key));
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let plaintext = bincode::serde::encode_to_vec(message, bincode::config::standard())
        .context("failed to encode plaintext")?;
    let ciphertext = cipher
        .encrypt(GenericArray::from_slice(&nonce), plaintext.as_ref())
        .context("failed to encrypt payload")?;
    Ok(WireMessage::Ciphertext(CipherFrame {
        nonce,
        body: ciphertext,
    }))
}

fn decrypt_text(meta: &ConnectionMeta, frame: &CipherFrame) -> Result<TextMessage> {
    let key = meta
        .shared_key()
        .ok_or_else(|| anyhow!("secure channel not established yet"))?;
    let cipher = ChaCha20Poly1305::new(GenericArray::from_slice(&key));
    let plaintext = cipher
        .decrypt(GenericArray::from_slice(&frame.nonce), frame.body.as_ref())
        .context("failed to decrypt payload")?;
    let (message, _) = bincode::serde::decode_from_slice(&plaintext, bincode::config::standard())
        .context("failed to decode message")?;
    Ok(message)
}

async fn read_connection(
    connection: quinn::Connection,
    chat_log: Option<PathBuf>,
    event_tx: mpsc::Sender<ServiceEvent>,
    ctx: PeerContext,
) -> Result<()> {
    let peer = connection.remote_address();
    loop {
        tokio::select! {
            res = connection.closed() => {
                tracing::debug!(?peer, "connection closed: {:?}", res);
                break;
            }
            stream = connection.accept_uni() => match stream {
                Ok(recv) => {
                    let tx = event_tx.clone();
                    let log = chat_log.clone();
                    let conn = connection.clone();
                    let peer_ctx = ctx.clone();
                    tokio::spawn(async move {
                        if let Err(err) = handle_stream(recv, log, tx, peer, conn, peer_ctx).await {
                            tracing::warn!(?peer, "stream error: {err:#}");
                        }
                    });
                }
                Err(err) => {
                    tracing::warn!(?peer, "failed to accept uni stream: {err:?}");
                    break;
                }
            },
            stream = connection.accept_bi() => match stream {
                Ok((_send, recv)) => {
                    let tx = event_tx.clone();
                    let log = chat_log.clone();
                    let conn = connection.clone();
                    let peer_ctx = ctx.clone();
                    tokio::spawn(async move {
                        if let Err(err) = handle_stream(recv, log, tx, peer, conn, peer_ctx).await {
                            tracing::warn!(?peer, "bi stream error: {err:#}");
                        }
                    });
                }
                Err(err) => {
                    tracing::warn!(?peer, "failed to accept bi stream: {err:?}");
                    break;
                }
            },
        }
    }
    Ok(())
}

async fn handle_stream(
    mut recv: quinn::RecvStream,
    chat_log: Option<PathBuf>,
    event_tx: mpsc::Sender<ServiceEvent>,
    peer: std::net::SocketAddr,
    connection: quinn::Connection,
    ctx: PeerContext,
) -> Result<()> {
    match protocol::read_message(&mut recv).await? {
        Some(WireMessage::Text(text)) => {
            event_tx
                .send(ServiceEvent::MessageReceived {
                    peer,
                    author: text.author.clone(),
                    text: text.body.clone(),
                })
                .await
                .ok();
            persist_chat(
                chat_log.clone(),
                format!("{} -> you: {}", text.author, text.body),
            )
            .await
            .ok();
            ctx.history
                .record(
                    peer,
                    HistoryEntry {
                        timestamp: text.timestamp,
                        outgoing: false,
                        author: text.author.clone(),
                        text: text.body.clone(),
                    },
                )
                .ok();
        }
        Some(WireMessage::FileMeta(meta)) => {
            receive_file_stream(recv, meta, event_tx.clone(), peer, ctx.clone()).await?;
        }
        Some(WireMessage::Control(control)) => {
            handle_control(control, connection, ctx.clone(), event_tx.clone(), peer).await?;
        }
        Some(WireMessage::Ciphertext(frame)) => match decrypt_text(&ctx.meta, &frame) {
            Ok(text) => {
                event_tx
                    .send(ServiceEvent::MessageReceived {
                        peer,
                        author: text.author.clone(),
                        text: text.body.clone(),
                    })
                    .await
                    .ok();
                persist_chat(
                    chat_log.clone(),
                    format!("{} -> you: {}", text.author, text.body),
                )
                .await
                .ok();
                ctx.history
                    .record(
                        peer,
                        HistoryEntry {
                            timestamp: text.timestamp,
                            outgoing: false,
                            author: text.author.clone(),
                            text: text.body.clone(),
                        },
                    )
                    .ok();
            }
            Err(err) => {
                event_tx
                    .send(ServiceEvent::Error {
                        message: format!("decryption error from {peer}: {err:#}"),
                    })
                    .await
                    .ok();
            }
        },
        Some(other) => {
            tracing::debug!(?other, "unexpected first frame");
        }
        None => {}
    }
    Ok(())
}

async fn handle_control(
    control: ControlMessage,
    connection: quinn::Connection,
    ctx: PeerContext,
    event_tx: mpsc::Sender<ServiceEvent>,
    peer: std::net::SocketAddr,
) -> Result<()> {
    match control {
        ControlMessage::Hello(hello) => {
            if let Some(required) = &ctx.required_password {
                if hello.password.as_deref() != Some(required) {
                    let _ = send_control_message(
                        &connection,
                        ControlMessage::Denied("Senha incorreta".into()),
                    )
                    .await;
                    connection.close(0u32.into(), b"invalid password");
                    bail!("peer {peer} failed password validation");
                }
            }
            ctx.meta.set_name(&hello.username);
            ctx.meta
                .derive(&hello.public_key)
                .context("failed to derive shared key")?;
            if let Ok(list) = ctx.peers.record(peer, &hello.username) {
                event_tx.send(ServiceEvent::SavedPeers(list)).await.ok();
            }
            event_tx
                .send(ServiceEvent::PeerProfile {
                    peer,
                    username: hello.username,
                })
                .await
                .ok();
        }
        ControlMessage::FileOffer(offer) => {
            let notice = FileOfferNotice {
                id: offer.id,
                name: offer.name.clone(),
                original_size: offer.original_size,
                compressed_size: offer.compressed_size,
                peer,
            };
            {
                let mut pending = ctx.incoming_offers.lock();
                pending.insert(offer.id, notice.clone());
            }
            event_tx.send(ServiceEvent::FileOffer(notice)).await.ok();
        }
        ControlMessage::FileAccept(ack) => {
            let transfer = {
                let mut pending = ctx.pending_transfers.lock();
                pending.remove(&ack.id)
            };
            if let Some(transfer) = transfer {
                let limits = ctx.limits.clone();
                let tx = event_tx.clone();
                let conn = connection.clone();
                tokio::spawn(async move {
                    if let Err(err) =
                        transmit_prepared_file(conn, transfer, limits, tx.clone()).await
                    {
                        let _ = tx
                            .send(ServiceEvent::Error {
                                message: format!("file transfer failed: {err:#}"),
                            })
                            .await;
                    }
                });
            }
        }
        ControlMessage::FileReject(reject) => {
            let transfer = {
                let mut pending = ctx.pending_transfers.lock();
                pending.remove(&reject.id)
            };
            if let Some(transfer) = transfer {
                let reason = reject
                    .reason
                    .unwrap_or_else(|| "peer declined the transfer".into());
                let name = transfer.offer.name;
                event_tx
                    .send(ServiceEvent::Error {
                        message: format!("File '{name}' was rejected: {reason}"),
                    })
                    .await
                    .ok();
            }
        }
        ControlMessage::Denied(reason) => {
            event_tx
                .send(ServiceEvent::Error {
                    message: format!("conexão recusada por {peer}: {reason}"),
                })
                .await
                .ok();
            connection.close(0u32.into(), b"remote denied");
        }
        ControlMessage::Info(info) => {
            event_tx
                .send(ServiceEvent::Error {
                    message: format!("mensagem de {peer}: {info}"),
                })
                .await
                .ok();
        }
    }
    Ok(())
}

async fn receive_file_stream(
    mut recv: quinn::RecvStream,
    meta: FileMetadata,
    event_tx: mpsc::Sender<ServiceEvent>,
    peer: std::net::SocketAddr,
    ctx: PeerContext,
) -> Result<()> {
    if meta.original_size > ctx.limits.max_file_bytes {
        bail!("incoming file exceeds configured limit");
    }
    let transfer = {
        let mut guard = ctx.incoming_transfers.lock();
        guard.remove(&meta.id)
    };
    let transfer = match transfer {
        Some(t) => t,
        None => {
            event_tx
                .send(ServiceEvent::Error {
                    message: format!("received unexpected file '{}' without approval", meta.name),
                })
                .await
                .ok();
            return Ok(());
        }
    };
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&transfer.temp_path)
        .await
        .context("unable to create temporary receive file")?;

    let mut total = 0u64;
    loop {
        match protocol::read_message(&mut recv).await? {
            Some(WireMessage::FileChunk(chunk)) => {
                if chunk.id != meta.id {
                    continue;
                }
                file.write_all(&chunk.bytes).await?;
                total += chunk.bytes.len() as u64;
                event_tx
                    .send(ServiceEvent::FileTransfer(FileTransferProgress {
                        id: meta.id,
                        name: transfer.original_name.clone(),
                        transferred: total,
                        total: meta.compressed_size,
                        direction: TransferDirection::Incoming,
                        path: Some(transfer.target_path.clone()),
                        completed: false,
                    }))
                    .await
                    .ok();
                if chunk.last || total >= meta.compressed_size {
                    break;
                }
            }
            Some(_) => {}
            None => break,
        }
    }
    file.flush().await?;
    drop(file);
    decompress_to_destination(&transfer.temp_path, &transfer.target_path).await?;
    tokio::fs::remove_file(&transfer.temp_path).await.ok();
    event_tx
        .send(ServiceEvent::FileTransfer(FileTransferProgress {
            id: meta.id,
            name: transfer.original_name.clone(),
            transferred: meta.compressed_size,
            total: meta.compressed_size,
            direction: TransferDirection::Incoming,
            path: Some(transfer.target_path.clone()),
            completed: true,
        }))
        .await
        .ok();
    tracing::info!(
        ?peer,
        path = %transfer.target_path.display(),
        "file received and decompressed"
    );
    Ok(())
}

async fn persist_chat(chat_log: Option<PathBuf>, line: String) -> Result<()> {
    if let Some(path) = chat_log {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await.ok();
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryEntry {
    timestamp: i64,
    outgoing: bool,
    author: String,
    text: String,
}

struct HistoryWriter {
    dir: PathBuf,
    key: [u8; 32],
    guard: Mutex<()>,
}

impl HistoryWriter {
    fn new(dir: PathBuf) -> Result<Self> {
        if !dir.exists() {
            fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create history directory {}", dir.display()))?;
        }
        let key = Self::load_or_create_key(&dir)?;
        Ok(Self {
            dir,
            key,
            guard: Mutex::new(()),
        })
    }

    fn load_or_create_key(dir: &Path) -> Result<[u8; 32]> {
        let key_path = dir.join("history.key");
        if key_path.exists() {
            let data = fs::read(&key_path).context("failed to read history key")?;
            if data.len() >= 32 {
                let mut key = [0u8; 32];
                key.copy_from_slice(&data[..32]);
                return Ok(key);
            }
        }
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        fs::write(&key_path, &key).context("failed to write history key")?;
        Ok(key)
    }

    fn record(&self, peer: std::net::SocketAddr, entry: HistoryEntry) -> Result<()> {
        let _lock = self.guard.lock();
        let encoded = bincode::serde::encode_to_vec(&entry, bincode::config::standard())
            .context("failed to encode history entry")?;
        let mut compressor = GzEncoder::new(Vec::new(), Compression::default());
        compressor
            .write_all(&encoded)
            .context("failed to compress history entry")?;
        let compressed = compressor
            .finish()
            .context("failed to finish compression")?;
        let cipher = ChaCha20Poly1305::new(GenericArray::from_slice(&self.key));
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = cipher
            .encrypt(GenericArray::from_slice(&nonce), compressed.as_ref())
            .context("failed to encrypt history payload")?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.file_for(peer))
            .with_context(|| format!("failed to open history file for {peer}"))?;
        file.write_all(&nonce)
            .context("failed writing history nonce")?;
        let len = ciphertext.len() as u32;
        file.write_all(&len.to_be_bytes())
            .context("failed writing history length")?;
        file.write_all(&ciphertext)
            .context("failed writing history payload")?;
        file.flush().ok();
        Ok(())
    }

    fn file_for(&self, peer: std::net::SocketAddr) -> PathBuf {
        let name = format!("{}", peer).replace(':', "_");
        self.dir.join(format!("{name}.hist"))
    }
}

struct SavedPeersStore {
    path: PathBuf,
    peers: Mutex<Vec<SavedPeer>>,
}

impl SavedPeersStore {
    fn new(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create peers directory {}", parent.display())
            })?;
        }
        let mut peers: Vec<SavedPeer> = if path.exists() {
            let data = fs::read(&path).context("failed to read peers file")?;
            if data.is_empty() {
                Vec::new()
            } else {
                serde_json::from_slice(&data).unwrap_or_default()
            }
        } else {
            Vec::new()
        };
        peers.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Self {
            path,
            peers: Mutex::new(peers),
        })
    }

    fn list(&self) -> Vec<SavedPeer> {
        self.peers.lock().clone()
    }

    fn record(&self, addr: std::net::SocketAddr, name: &str) -> Result<Vec<SavedPeer>> {
        let mut peers = self.peers.lock();
        if let Some(existing) = peers.iter_mut().find(|peer| peer.addr == addr) {
            existing.name = name.to_string();
        } else {
            peers.push(SavedPeer {
                addr,
                name: name.to_string(),
            });
        }
        peers.sort_by(|a, b| a.name.cmp(&b.name));
        let serialized = serde_json::to_vec_pretty(&*peers).context("failed to encode peers")?;
        fs::write(&self.path, serialized).context("failed to store peers file")?;
        Ok(peers.clone())
    }
}

struct PreparedTransfer {
    offer: FileOffer,
    original_path: PathBuf,
    compressed_path: PathBuf,
}

struct IncomingTransfer {
    target_path: PathBuf,
    temp_path: PathBuf,
    original_name: String,
}

async fn prepare_transfer(path: PathBuf, limits: &LimitsConfig) -> Result<PreparedTransfer> {
    let metadata = tokio::fs::metadata(&path)
        .await
        .with_context(|| format!("unable to read {}", path.display()))?;
    if !metadata.is_file() {
        bail!("{} is not a file", path.display());
    }
    if metadata.len() > limits.max_file_bytes {
        bail!("file exceeds maximum permitted size");
    }
    let original_size = metadata.len();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file.bin")
        .to_string();
    let path_clone = path.clone();
    let (compressed_path, compressed_size) = spawn_blocking(move || -> Result<(PathBuf, u64)> {
        let mut source = std::fs::File::open(&path_clone)
            .with_context(|| format!("unable to open {}", path_clone.display()))?;
        let temp = NamedTempFile::new().context("failed to create temp file")?;
        let mut encoder = GzEncoder::new(temp, Compression::default());
        std::io::copy(&mut source, &mut encoder).context("failed to compress file")?;
        let mut finished = encoder.finish().context("failed to finalize compression")?;
        finished.as_file_mut().flush().ok();
        let compressed_size = finished
            .as_file()
            .metadata()
            .context("failed to inspect compressed file")?
            .len();
        let temp_path = finished.into_temp_path();
        let stored = temp_path
            .keep()
            .context("failed to persist compressed file")?;
        Ok((stored, compressed_size))
    })
    .await??;
    let offer = FileOffer {
        id: rand::random(),
        name,
        original_size,
        compressed_size,
    };
    Ok(PreparedTransfer {
        offer,
        original_path: path,
        compressed_path,
    })
}

async fn transmit_prepared_file(
    connection: quinn::Connection,
    transfer: PreparedTransfer,
    limits: LimitsConfig,
    event_tx: mpsc::Sender<ServiceEvent>,
) -> Result<()> {
    let mut file = tokio::fs::File::open(&transfer.compressed_path)
        .await
        .context("failed to open compressed file")?;
    let mut stream = connection
        .open_uni()
        .await
        .context("cannot open file-transfer stream")?;
    protocol::write_message(
        &mut stream,
        &WireMessage::FileMeta(FileMetadata {
            id: transfer.offer.id,
            name: transfer.offer.name.clone(),
            compressed_size: transfer.offer.compressed_size,
            original_size: transfer.offer.original_size,
        }),
    )
    .await?;

    let mut transferred = 0u64;
    let mut buffer = vec![0u8; limits.chunk_size_bytes];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let chunk = WireMessage::FileChunk(FileChunk {
            id: transfer.offer.id,
            offset: transferred,
            bytes: buffer[..read].to_vec(),
            last: transferred + read as u64 >= transfer.offer.compressed_size,
        });
        protocol::write_message(&mut stream, &chunk).await?;
        transferred += read as u64;
        event_tx
            .send(ServiceEvent::FileTransfer(FileTransferProgress {
                id: transfer.offer.id,
                name: transfer.offer.name.clone(),
                transferred,
                total: transfer.offer.compressed_size,
                direction: TransferDirection::Outgoing,
                path: Some(transfer.original_path.clone()),
                completed: transferred >= transfer.offer.compressed_size,
            }))
            .await
            .ok();
    }
    let _ = stream.finish();
    tokio::fs::remove_file(&transfer.compressed_path).await.ok();
    Ok(())
}

async fn decompress_to_destination(source: &Path, destination: &Path) -> Result<()> {
    let src = source.to_path_buf();
    let dst = destination.to_path_buf();
    spawn_blocking(move || -> Result<()> {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let input = std::fs::File::open(&src)
            .with_context(|| format!("failed to open {}", src.display()))?;
        let mut decoder = GzDecoder::new(input);
        let mut output = std::fs::File::create(&dst)
            .with_context(|| format!("failed to create {}", dst.display()))?;
        std::io::copy(&mut decoder, &mut output).context("failed to decompress payload")?;
        output.flush().ok();
        Ok(())
    })
    .await??;
    Ok(())
}
