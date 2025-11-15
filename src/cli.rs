use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};

/// dezap command line interface definition.
#[derive(Debug, Parser)]
#[command(author, version, about = "Retro QUIC-based LAN messenger", long_about = None)]
pub struct Cli {
    /// Path to an alternate configuration file.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Increase log verbosity (-vv for debug, -vvv for trace).
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Optional subcommands. Defaults to launching the TUI.
    #[command(subcommand)]
    pub command: Option<Commands>,
}

impl Cli {
    /// Returns the configured verbosity level.
    pub fn verbosity(&self) -> u8 {
        self.verbose
    }

    /// Returns the configured config path, if any.
    pub fn config_path(&self) -> Option<&Path> {
        self.config.as_deref()
    }

    /// Resolves the command, defaulting to [`Commands::Tui`].
    pub fn command_or_default(&self) -> Commands {
        self.command
            .clone()
            .unwrap_or_else(|| Commands::Tui(TuiCommand::default()))
    }
}

/// dezap operational modes.
#[derive(Debug, Clone, Subcommand)]
pub enum Commands {
    /// Launch the retro TUI.
    Tui(TuiCommand),
    /// Run headless listener mode that prints logs.
    Listen(ListenCommand),
    /// Send a single text message to a peer.
    Send(SendCommand),
    /// Send a file to a peer without launching the TUI.
    SendFile(SendFileCommand),
}

/// Parameters for the TUI mode.
#[derive(Debug, Clone, Args)]
pub struct TuiCommand {
    /// Automatically start listening using this bind address when the TUI boots.
    #[arg(long)]
    pub bind: Option<SocketAddr>,

    /// Automatically connect to this peer after the TUI boots.
    #[arg(long)]
    pub connect: Option<SocketAddr>,

    /// Skip peer discovery on launch (useful for restrictive environments).
    #[arg(long)]
    pub disable_discovery: bool,
}

impl Default for TuiCommand {
    fn default() -> Self {
        Self {
            bind: None,
            connect: None,
            disable_discovery: false,
        }
    }
}

/// Parameters for the headless listener.
#[derive(Debug, Clone, Args)]
pub struct ListenCommand {
    /// Address to bind to (overrides configuration).
    #[arg(long)]
    pub bind: Option<SocketAddr>,

    /// Optional password required for peers to connect.
    #[arg(long)]
    pub password: Option<String>,
}

/// Single-message send options.
#[derive(Debug, Clone, Args)]
pub struct SendCommand {
    /// Peer socket address in host:port form.
    #[arg(long, value_name = "HOST:PORT")]
    pub to: SocketAddr,

    /// Message contents.
    #[arg(long)]
    pub text: String,
}

/// Single file trasfer command.
#[derive(Debug, Clone, Args)]
pub struct SendFileCommand {
    /// Peer socket address in host:port form.
    #[arg(long, value_name = "HOST:PORT")]
    pub to: SocketAddr,

    /// Path to the local file that should be transmitted.
    #[arg(long, value_name = "PATH")]
    pub path: PathBuf,
}
