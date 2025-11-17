use anyhow::Result;
use clap::Parser;
use dezap::{
    cli::{self, Commands},
    config::AppConfig,
    logging, service,
};

#[cfg(feature = "tui")]
use dezap::tui;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let cli = cli::Cli::parse();
    let config = AppConfig::load(cli.config_path())?;
    let command = cli.command_or_default();
    let is_tui = matches!(command, Commands::Tui(_));
    if is_tui {
        logging::init_quiet(&config.logging, cli.verbosity())?;
    } else {
        logging::init(&config.logging, cli.verbosity())?;
    }

    match command {
        Commands::Listen(cmd) => service::run_listener(&config, cmd).await,
        Commands::Send(cmd) => service::run_cli_message(&config, cmd).await,
        Commands::SendFile(cmd) => service::run_cli_file_send(&config, cmd).await,
        Commands::Tui(args) => {
            #[cfg(feature = "tui")]
            {
                let service = service::DezapService::new(config.clone());
                tui::run(service, config.clone(), args).await
            }

            #[cfg(not(feature = "tui"))]
            {
                anyhow::bail!(
                    "dezap was built without the `tui` feature; enable it to launch the interface"
                );
            }
        }
    }
}
