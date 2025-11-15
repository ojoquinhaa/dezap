use anyhow::Result;
use once_cell::sync::OnceCell;
use tracing_subscriber::{fmt, EnvFilter};

use crate::config::LoggingConfig;

static INITIALIZED: OnceCell<()> = OnceCell::new();

/// Initializes global tracing/logging subscribers.
pub fn init(config: &LoggingConfig, verbosity: u8) -> Result<()> {
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

    fmt().with_env_filter(env_filter).with_target(false).init();
    INITIALIZED
        .set(())
        .map_err(|_| anyhow::anyhow!("logger already initialized"))?;

    Ok(())
}
