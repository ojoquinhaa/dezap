use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEvent};
use tokio::sync::mpsc::{self, UnboundedReceiver};

/// Events emitted from the terminal input thread.
#[derive(Debug, Clone)]
pub enum TuiEvent {
    Input(KeyEvent),
    Tick,
    Resize,
}

/// Async wrapper over the input polling thread.
pub struct EventStream {
    rx: UnboundedReceiver<TuiEvent>,
    running: Arc<AtomicBool>,
}

impl EventStream {
    pub fn new() -> Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let running = Arc::new(AtomicBool::new(true));
        let flag = running.clone();
        std::thread::spawn(move || {
            while flag.load(Ordering::Relaxed) {
                match event::poll(Duration::from_millis(150)) {
                    Ok(true) => match event::read() {
                        Ok(Event::Key(key)) => {
                            let _ = tx.send(TuiEvent::Input(key));
                        }
                        Ok(Event::Resize(_, _)) => {
                            let _ = tx.send(TuiEvent::Resize);
                        }
                        _ => {}
                    },
                    Ok(false) => {
                        let _ = tx.send(TuiEvent::Tick);
                    }
                    Err(err) => {
                        tracing::warn!(%err, "terminal event error");
                        break;
                    }
                }
            }
        });

        Ok(Self { rx, running })
    }

    pub async fn next(&mut self) -> Option<TuiEvent> {
        self.rx.recv().await
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}
