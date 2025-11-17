use std::io;

use anyhow::Result;
use once_cell::sync::OnceCell;
use tracing_subscriber::{fmt, EnvFilter};
use tracing_subscriber::fmt::MakeWriter;

use crate::config::LoggingConfig;

static INITIALIZED: OnceCell<()> = OnceCell::new();

/// Initializes global tracing/logging subscribers.
pub fn init(config: &LoggingConfig, verbosity: u8) -> Result<()> {
    init_with_writer(config, verbosity, io::stderr)
}

/// Initializes logging without writing to the terminal (useful for the TUI).
pub fn init_quiet(config: &LoggingConfig, verbosity: u8) -> Result<()> {
    init_with_writer(config, verbosity, || io::sink())
}

fn init_with_writer<W>(config: &LoggingConfig, verbosity: u8, writer: W) -> Result<()>
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    if INITIALIZED.get().is_some() {
        return Ok(());
    }

    let level = match verbosity {
        0 => config.level(),
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    let env_filter = EnvFilter::try_new(level).or_else(|_| EnvFilter::try_new("info"))?;

    fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    INITIALIZED
        .set(())
        .map_err(|_| anyhow::anyhow!("logger already initialized"))?;

    Ok(())
}
