use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::cli::{ListenCommand, SendCommand, SendFileCommand};
use crate::config::{AppConfig, LimitsConfig};
use crate::net;
use crate::protocol::{self, FileChunk, FileMetadata, TextMessage, WireMessage};

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
    Listen { addr: std::net::SocketAddr },
    StopListening,
    Connect { addr: std::net::SocketAddr },
    Disconnect,
    SendText { text: String },
    SendFile { path: PathBuf },
    Discover,
}

/// Events emitted by the service to inform the UI/CLI.
#[derive(Debug, Clone)]
pub enum ServiceEvent {
    Connected {
        peer: std::net::SocketAddr,
    },
    Connecting {
        peer: std::net::SocketAddr,
    },
    Listening {
        addr: std::net::SocketAddr,
    },
    ListenerStopped,
    Disconnected,
    MessageReceived {
        peer: std::net::SocketAddr,
        text: String,
    },
    MessageSent {
        text: String,
    },
    FileTransfer(FileTransferProgress),
    Discovery(DiscoveryEvent),
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

/// Runs the service runtime loop.
async fn runtime_loop(
    config: AppConfig,
    mut cmd_rx: mpsc::Receiver<ServiceCommand>,
    event_tx: mpsc::Sender<ServiceEvent>,
) {
    let (internal_tx, mut internal_rx) = mpsc::channel(32);
    let mut state = ServiceState::new(config, event_tx.clone(), internal_tx.clone());

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
}

impl ServiceState {
    fn new(
        config: AppConfig,
        event_tx: mpsc::Sender<ServiceEvent>,
        internal_tx: mpsc::Sender<InternalSignal>,
    ) -> Self {
        Self {
            config,
            event_tx,
            internal_tx,
            listener: None,
            client: None,
            connection: None,
        }
    }

    async fn handle_command(&mut self, command: ServiceCommand) -> Result<()> {
        match command {
            ServiceCommand::Listen { addr } => self.start_listener(addr).await,
            ServiceCommand::StopListening => self.stop_listener().await,
            ServiceCommand::Connect { addr } => self.connect(addr).await,
            ServiceCommand::Disconnect => self.disconnect().await,
            ServiceCommand::SendText { text } => self.send_text(text).await,
            ServiceCommand::SendFile { path } => self.send_file(path).await,
            ServiceCommand::Discover => self.run_discovery().await,
        }
    }

    async fn handle_internal(&mut self, signal: InternalSignal) -> Result<()> {
        match signal {
            InternalSignal::Inbound(connection, peer) => {
                self.attach_connection(connection, peer).await
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

    async fn start_listener(&mut self, addr: std::net::SocketAddr) -> Result<()> {
        if self.listener.is_some() {
            bail!("listener already active");
        }

        let server = net::bind_server(addr, &self.config.tls)?;
        let discovery = net::spawn_discovery_responder(addr, &self.config.discovery).await?;
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
            .send(ServiceEvent::Listening { addr })
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

    async fn connect(&mut self, addr: std::net::SocketAddr) -> Result<()> {
        self.event_tx
            .send(ServiceEvent::Connecting { peer: addr })
            .await
            .ok();
        self.disconnect().await?;
        let server_name = self.config.tls.server_name().to_string();
        let client = self.client_endpoint()?;
        let connection =
            net::connect(client.endpoint, &client.client_config, addr, &server_name).await?;
        self.attach_connection(connection, addr).await
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
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| anyhow!("no active connection"))?
            .connection
            .clone();
        send_text_message(
            &connection,
            text.clone(),
            &self.config.limits,
            &self.event_tx,
        )
        .await?;
        persist_chat(
            self.config.paths.chat_log.clone(),
            format!("outgoing: {}", text),
        )
        .await?;
        Ok(())
    }

    async fn send_file(&mut self, path: PathBuf) -> Result<()> {
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| anyhow!("no active connection"))?
            .connection
            .clone();
        send_file(&connection, path, &self.config.limits, &self.event_tx).await
    }

    async fn run_discovery(&mut self) -> Result<()> {
        let peers = net::discover_peers(&self.config.discovery).await?;
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

    async fn attach_connection(
        &mut self,
        connection: quinn::Connection,
        peer: std::net::SocketAddr,
    ) -> Result<()> {
        self.disconnect().await?;
        let download_dir = self.config.paths.download_dir.clone();
        let limits = self.config.limits.clone();
        let event_tx = self.event_tx.clone();
        let chat_log = self.config.paths.chat_log.clone();
        let internal = self.internal_tx.clone();
        let reader_connection = connection.clone();
        let reader = tokio::spawn(async move {
            if let Err(err) = read_connection(
                reader_connection,
                download_dir,
                limits,
                chat_log,
                event_tx.clone(),
            )
            .await
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
        });
        self.event_tx
            .send(ServiceEvent::Connected { peer })
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
}

enum InternalSignal {
    Inbound(quinn::Connection, std::net::SocketAddr),
    ConnectionClosed(std::net::SocketAddr),
}

/// Runs a headless listener in CLI mode.
pub async fn run_listener(config: &AppConfig, cmd: ListenCommand) -> Result<()> {
    let addr = cmd.bind.unwrap_or(config.listen.bind_addr);
    let mut service = DezapService::new(config.clone());
    service.send(ServiceCommand::Listen { addr }).await?;

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
    let ctx = net::build_client_endpoint(config.listen.bind_addr, &config.tls)?;
    let connection = net::connect(
        &ctx.endpoint,
        &ctx.client_config,
        cmd.to,
        config.tls.server_name(),
    )
    .await?;
    send_text_message(
        &connection,
        cmd.text.clone(),
        &config.limits,
        &noop_event_tx(),
    )
    .await?;
    connection.close(0u32.into(), b"text sent");
    ctx.endpoint.wait_idle().await;
    persist_chat(
        config.paths.chat_log.clone(),
        format!("cli -> {}: {}", cmd.to, cmd.text),
    )
    .await
    .ok();
    Ok(())
}

/// CLI helper for file transfers.
pub async fn run_cli_file_send(config: &AppConfig, cmd: SendFileCommand) -> Result<()> {
    let ctx = net::build_client_endpoint(config.listen.bind_addr, &config.tls)?;
    let connection = net::connect(
        &ctx.endpoint,
        &ctx.client_config,
        cmd.to,
        config.tls.server_name(),
    )
    .await?;
    send_file(
        &connection,
        cmd.path.clone(),
        &config.limits,
        &noop_event_tx(),
    )
    .await?;
    connection.close(0u32.into(), b"file sent");
    ctx.endpoint.wait_idle().await;
    persist_chat(
        config.paths.chat_log.clone(),
        format!("file sent to {}: {}", cmd.to, cmd.path.display()),
    )
    .await
    .ok();
    Ok(())
}

fn noop_event_tx() -> mpsc::Sender<ServiceEvent> {
    let (tx, _rx) = mpsc::channel(1);
    tx
}

async fn send_text_message(
    connection: &quinn::Connection,
    text: String,
    limits: &LimitsConfig,
    event_tx: &mpsc::Sender<ServiceEvent>,
) -> Result<()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("empty messages are ignored");
    }
    if trimmed.len() > limits.max_message_bytes {
        bail!("message length exceeds configured limit");
    }
    let mut stream = connection
        .open_uni()
        .await
        .context("failed opening unidirectional stream")?;
    let payload = WireMessage::Text(TextMessage {
        id: rand::random(),
        body: trimmed.to_string(),
        timestamp: protocol::utc_timestamp(),
    });
    protocol::write_message(&mut stream, &payload).await?;
    let _ = stream.finish();
    event_tx
        .send(ServiceEvent::MessageSent {
            text: trimmed.to_string(),
        })
        .await
        .ok();
    Ok(())
}

async fn send_file(
    connection: &quinn::Connection,
    path: PathBuf,
    limits: &LimitsConfig,
    event_tx: &mpsc::Sender<ServiceEvent>,
) -> Result<()> {
    let metadata = tokio::fs::metadata(&path)
        .await
        .with_context(|| format!("unable to read {}", path.display()))?;
    if !metadata.is_file() {
        bail!("{} is not a file", path.display());
    }
    if metadata.len() > limits.max_file_bytes {
        bail!("file exceeds maximum permitted size");
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file.bin")
        .to_string();
    let file_id = rand::random::<u64>();
    let mut file = tokio::fs::File::open(&path).await?;
    let mut stream = connection
        .open_uni()
        .await
        .context("cannot open file-transfer stream")?;

    protocol::write_message(
        &mut stream,
        &WireMessage::FileMeta(FileMetadata {
            id: file_id,
            name: file_name.clone(),
            size: metadata.len(),
        }),
    )
    .await?;

    let mut offset = 0u64;
    let mut buffer = vec![0u8; limits.chunk_size_bytes];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let chunk = WireMessage::FileChunk(FileChunk {
            id: file_id,
            offset,
            bytes: buffer[..read].to_vec(),
            last: offset + read as u64 >= metadata.len(),
        });
        protocol::write_message(&mut stream, &chunk).await?;
        offset += read as u64;
        event_tx
            .send(ServiceEvent::FileTransfer(FileTransferProgress {
                id: file_id,
                name: file_name.clone(),
                transferred: offset,
                total: metadata.len(),
                direction: TransferDirection::Outgoing,
                path: Some(path.clone()),
                completed: offset >= metadata.len(),
            }))
            .await
            .ok();
    }
    let _ = stream.finish();
    Ok(())
}

async fn read_connection(
    connection: quinn::Connection,
    download_dir: PathBuf,
    limits: LimitsConfig,
    chat_log: Option<PathBuf>,
    event_tx: mpsc::Sender<ServiceEvent>,
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
                    let dir = download_dir.clone();
                    let limits = limits.clone();
                    let tx = event_tx.clone();
                    let log = chat_log.clone();
                    tokio::spawn(async move {
                        if let Err(err) = handle_stream(recv, dir, limits, log, tx, peer).await {
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
                    let dir = download_dir.clone();
                    let limits = limits.clone();
                    let tx = event_tx.clone();
                    let log = chat_log.clone();
                    tokio::spawn(async move {
                        if let Err(err) = handle_stream(recv, dir, limits, log, tx, peer).await {
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
    download_dir: PathBuf,
    limits: LimitsConfig,
    chat_log: Option<PathBuf>,
    event_tx: mpsc::Sender<ServiceEvent>,
    peer: std::net::SocketAddr,
) -> Result<()> {
    match protocol::read_message(&mut recv).await? {
        Some(WireMessage::Text(text)) => {
            event_tx
                .send(ServiceEvent::MessageReceived {
                    peer,
                    text: text.body.clone(),
                })
                .await
                .ok();
            persist_chat(
                chat_log.clone(),
                format!("incoming from {peer}: {}", text.body),
            )
            .await
            .ok();
        }
        Some(WireMessage::FileMeta(meta)) => {
            receive_file_stream(recv, meta, download_dir, limits, event_tx, peer).await?;
        }
        Some(other) => {
            tracing::debug!(?other, "unexpected first frame");
        }
        None => {}
    }
    Ok(())
}

async fn receive_file_stream(
    mut recv: quinn::RecvStream,
    meta: FileMetadata,
    download_dir: PathBuf,
    limits: LimitsConfig,
    event_tx: mpsc::Sender<ServiceEvent>,
    peer: std::net::SocketAddr,
) -> Result<()> {
    if meta.size > limits.max_file_bytes {
        bail!("incoming file exceeds configured limit");
    }

    let file_name = sanitize_filename(&meta.name);
    let target_path = download_dir.join(file_name.clone());
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&target_path)
        .await
        .context("unable to create destination file")?;

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
                        name: file_name.clone(),
                        transferred: total,
                        total: meta.size,
                        direction: TransferDirection::Incoming,
                        path: Some(target_path.clone()),
                        completed: chunk.last || total >= meta.size,
                    }))
                    .await
                    .ok();
                if chunk.last || total >= meta.size {
                    break;
                }
            }
            Some(_) => {}
            None => break,
        }
    }
    file.flush().await?;
    tracing::info!(?peer, path=%target_path.display(), "file received");
    Ok(())
}

fn sanitize_filename(name: &str) -> String {
    let candidate = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect::<String>();
    if candidate.is_empty() {
        "download.bin".to_string()
    } else {
        candidate
    }
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
