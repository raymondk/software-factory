//! Spec 4.7: every line the worker prints goes to stderr and, batched, to the orchestrator.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use api_client::{Client, Error, ShipLogs};
use tokio::sync::{mpsc, oneshot};

enum Item {
    Line(Option<i64>, String),
    Flush(oneshot::Sender<()>),
}

pub struct Shipping {
    pub interval: Duration,
    pub batch: usize,
    pub max_line: usize,
}

#[derive(Clone)]
pub struct Log {
    tx: mpsc::UnboundedSender<Item>,
    /// The current run; 0 outside a run.
    run: Arc<AtomicI64>,
}

impl Log {
    /// Starts the shipper. Call `flush` before exiting.
    pub fn start(client: Client, worker: String, cfg: Shipping) -> Log {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(ship(client, worker, rx, cfg));
        Log { tx, run: Arc::default() }
    }

    /// Prints only; nothing is shipped.
    #[cfg(test)]
    pub fn stderr() -> Log {
        let (tx, _) = mpsc::unbounded_channel();
        Log { tx, run: Arc::default() }
    }

    pub fn set_run(&self, run: Option<i64>) {
        self.run.store(run.unwrap_or(0), Ordering::Relaxed);
    }

    pub fn line(&self, line: impl Into<String>) {
        let line = line.into();
        eprintln!("{line}");
        let run = self.run.load(Ordering::Relaxed);
        let _ = self.tx.send(Item::Line((run != 0).then_some(run), line));
    }

    /// Ships everything queued so far.
    pub async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Item::Flush(tx)).is_ok() {
            let _ = rx.await;
        }
    }
}

/// Flushes every `interval` or `batch` lines, whichever comes first, so a crash loses at most one batch.
async fn ship(client: Client, worker: String, mut rx: mpsc::UnboundedReceiver<Item>, cfg: Shipping) {
    let mut buf: Vec<(Option<i64>, String)> = Vec::new();
    loop {
        let timer = tokio::time::sleep(cfg.interval);
        tokio::pin!(timer);
        let mut ack = None;
        loop {
            tokio::select! {
                item = rx.recv() => match item {
                    Some(Item::Line(run, mut line)) => {
                        if line.len() > cfg.max_line {
                            let mut end = cfg.max_line;
                            while !line.is_char_boundary(end) {
                                end -= 1;
                            }
                            line.truncate(end);
                        }
                        buf.push((run, line));
                        if buf.len() >= cfg.batch {
                            break;
                        }
                    }
                    Some(Item::Flush(tx)) => {
                        ack = Some(tx);
                        break;
                    }
                    None => {
                        flush(&client, &worker, &mut buf).await;
                        return;
                    }
                },
                _ = &mut timer => break,
            }
        }
        flush(&client, &worker, &mut buf).await;
        if let Some(tx) = ack {
            let _ = tx.send(());
        }
    }
}

/// One request per stretch of consecutive lines from the same run. Lines the orchestrator could not be reached for
/// stay queued for the next flush; rejected ones are dropped.
async fn flush(client: &Client, worker: &str, buf: &mut Vec<(Option<i64>, String)>) {
    while !buf.is_empty() {
        let run = buf[0].0;
        let n = buf.iter().take_while(|(r, _)| *r == run).count();
        let lines = buf[..n].iter().map(|(_, l)| l.clone()).collect();
        match client.ship_logs(worker, &ShipLogs { run, lines }).await {
            Ok(()) => drop(buf.drain(..n)),
            Err(Error::Api { status, body }) if status < 500 => {
                eprintln!("worker {worker}: shipping logs: {status}: {body}");
                buf.drain(..n);
            }
            Err(e) => {
                eprintln!("worker {worker}: shipping logs: {e}");
                return;
            }
        }
    }
}
