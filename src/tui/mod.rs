use std::io::{self, stdout};

use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::cli::TuiCommand;
use crate::config::AppConfig;
use crate::service::{DezapService, ServiceCommand, ServiceEvent};

mod app;
mod events;
mod ui;

pub use app::App;

use events::{EventStream, TuiEvent};

/// Main entry point for the TUI runtime.
pub async fn run(mut service: DezapService, config: AppConfig, args: TuiCommand) -> Result<()> {
    let _guard = TerminalGuard::new()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut app = App::new(&config, &args);
    let mut events = EventStream::new()?;

    if let Some(addr) = args.bind {
        service.send(ServiceCommand::Listen { addr }).await.ok();
    }
    if let Some(peer) = args.connect.or(config.peer.default_peer) {
        service
            .send(ServiceCommand::Connect { addr: peer })
            .await
            .ok();
    }
    if app.discovery_enabled {
        service.send(ServiceCommand::Discover).await.ok();
    }

    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;
        tokio::select! {
            Some(evt) = events.next() => match evt {
                TuiEvent::Input(key) => {
                    if let Some(cmd) = app.handle_key(key) {
                        if let Err(err) = service.send(cmd).await {
                            app.handle_service_event(ServiceEvent::Error {
                                message: err.to_string(),
                            });
                        }
                    }
                }
                TuiEvent::Resize | TuiEvent::Tick => {}
            },
            event = service.next_event() => {
                match event {
                    Some(svc_event) => app.handle_service_event(svc_event),
                    None => {
                        app.should_quit = true;
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    terminal.show_cursor()?;
    Ok(())
}

struct TerminalGuard;

impl TerminalGuard {
    fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Err(err) = disable_raw_mode() {
            tracing::warn!(%err, "failed to disable raw mode");
        }
        let mut stdout = io::stdout();
        let _ = crossterm::execute!(stdout, LeaveAlternateScreen, DisableMouseCapture);
    }
}
