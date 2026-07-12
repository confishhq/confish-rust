//! Ships [`tracing`](https://docs.rs/tracing) events to confish as log entries.
//!
//! See [`TracingLayer`] for the entry point.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing_core::field::{Field, Visit};
use tracing_core::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::client::Client;
use crate::error::{Error, Result};
use crate::logs::{LogEntryInput, Logs, MAX_BATCH_ENTRIES};
use crate::types::LogLevel;

tokio::task_local! {
    /// Set inside the worker's own HTTP flushes so `tracing` events emitted by
    /// the SDK's HTTP stack while sending a batch don't feed back into the layer.
    static IN_FLUSH: bool;
}

/// A [`tracing_subscriber::Layer`](Layer) that ships `tracing` events to
/// confish as log entries.
///
/// Built via [`TracingLayer::builder`], which returns the layer plus a
/// [`TracingGuard`]. **Building requires a running tokio runtime** — the
/// builder spawns a background task that batches entries and sends them.
///
/// `on_event` never blocks and never panics: it only pushes onto a bounded
/// in-memory queue (default capacity 1000). When the queue is full the
/// incoming event is dropped (drop-newest — a bounded `tokio::sync::mpsc`
/// channel cannot evict from the front); [`TracingGuard::dropped_count`]
/// reports how many. The background task flushes through
/// [`Logs::write_batch`](crate::Logs::write_batch) at 50 entries or every
/// 5 seconds, whichever comes first, chunked to at most 100 entries per
/// request.
///
/// Level mapping (confish levels follow RFC 5424): `TRACE` → `debug`,
/// `DEBUG` → `debug`, `INFO` → `info`, `WARN` → `warning`, `ERROR` → `error`.
///
/// Event fields become the entry's `context` object and the `message` field
/// becomes the entry's message. Span fields are **not** captured in this
/// version — event fields only. Timestamps are captured at event time and
/// sent per entry.
///
/// Delivery failures are retried per the client's retry policy and then
/// discarded silently — the layer never emits `tracing` events of its own,
/// so it cannot feed back into itself.
///
/// ```no_run
/// use tracing_subscriber::prelude::*;
///
/// # async fn run() -> confish::Result<()> {
/// let client = confish::Client::builder("env_id", "confish_sk_...").build()?;
/// let (layer, guard) = confish::TracingLayer::builder(&client).build()?;
/// tracing_subscriber::registry().with(layer).init();
///
/// tracing::info!(job = "db-backup", "Job started");
///
/// // ... run your workload ...
///
/// guard.shutdown().await; // flush everything before exiting
/// # Ok(())
/// # }
/// ```
#[cfg_attr(docsrs, doc(cfg(feature = "tracing")))]
pub struct TracingLayer {
    tx: mpsc::Sender<LogEntryInput>,
    dropped: Arc<AtomicU64>,
}

impl TracingLayer {
    /// Start configuring a layer that sends through the given client.
    #[must_use]
    pub fn builder(client: &Client) -> TracingLayerBuilder {
        TracingLayerBuilder {
            client: client.clone(),
            capacity: 1000,
            flush_after: 50,
            flush_interval: Duration::from_secs(5),
            shutdown_timeout: Duration::from_secs(5),
        }
    }
}

impl<S: Subscriber> Layer<S> for TracingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        // Events emitted by the SDK's own HTTP stack while a batch is being
        // sent would recurse forever — skip them.
        if IN_FLUSH.try_with(|in_flush| *in_flush).unwrap_or(false) {
            return;
        }

        let timestamp = format_rfc3339(SystemTime::now());
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        let entry = LogEntryInput {
            level: map_level(event.metadata().level()),
            message: visitor.message,
            context: if visitor.fields.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(visitor.fields))
            },
            timestamp: Some(timestamp),
        };

        // Full queue or a worker that has already shut down both mean the
        // event goes nowhere — count it and move on without blocking.
        if self.tx.try_send(entry).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Builder for [`TracingLayer`].
#[cfg_attr(docsrs, doc(cfg(feature = "tracing")))]
pub struct TracingLayerBuilder {
    client: Client,
    capacity: usize,
    flush_after: usize,
    flush_interval: Duration,
    shutdown_timeout: Duration,
}

impl TracingLayerBuilder {
    /// Capacity of the bounded in-memory queue between `on_event` and the
    /// background sender. When full, incoming events are dropped (newest).
    /// Defaults to 1000; clamped to at least 1.
    #[must_use]
    pub fn capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// Number of buffered entries that triggers a flush. Defaults to 50;
    /// clamped to at least 1. Flushes are chunked to at most 100 entries per
    /// request regardless of this setting.
    #[must_use]
    pub fn flush_after(mut self, flush_after: usize) -> Self {
        self.flush_after = flush_after;
        self
    }

    /// How often buffered entries are flushed even when [`flush_after`](Self::flush_after)
    /// hasn't been reached. Defaults to 5 seconds.
    #[must_use]
    pub fn flush_interval(mut self, flush_interval: Duration) -> Self {
        self.flush_interval = flush_interval;
        self
    }

    /// How long dropping the [`TracingGuard`] blocks waiting for the final
    /// flush. Defaults to 5 seconds. [`TracingGuard::shutdown`] ignores this
    /// and waits for the flush to complete without blocking the runtime.
    #[must_use]
    pub fn shutdown_timeout(mut self, shutdown_timeout: Duration) -> Self {
        self.shutdown_timeout = shutdown_timeout;
        self
    }

    /// Build the layer and spawn its background sender.
    ///
    /// Must be called from within a tokio runtime (the sender is spawned with
    /// `tokio::spawn`); otherwise this fails with a descriptive error rather
    /// than panicking.
    pub fn build(self) -> Result<(TracingLayer, TracingGuard)> {
        let handle = tokio::runtime::Handle::try_current().map_err(|_| Error::Api {
            status: 0,
            message: "the tracing layer must be built inside a tokio runtime \
                      (its background sender is spawned with tokio::spawn)"
                .to_string(),
            body: None,
        })?;

        let (tx, rx) = mpsc::channel(self.capacity.max(1));
        let dropped = Arc::new(AtomicU64::new(0));
        let cancel = CancellationToken::new();
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        let worker = Worker {
            logs: self.client.logs(),
            rx,
            flush_after: self.flush_after.max(1),
            flush_interval: self.flush_interval.max(Duration::from_millis(1)),
            cancel: cancel.clone(),
            dropped: dropped.clone(),
            done_tx,
        };
        let join = handle.spawn(worker.run());

        Ok((
            TracingLayer {
                tx,
                dropped: dropped.clone(),
            },
            TracingGuard {
                cancel,
                done_rx: Some(done_rx),
                worker: Some(join),
                dropped,
                shutdown_timeout: self.shutdown_timeout,
            },
        ))
    }
}

/// Keeps the background sender alive and flushes it on shutdown — hold on to
/// it for as long as the layer is in use (the tracing-appender `WorkerGuard`
/// pattern).
///
/// Dropping the guard stops the sender and flushes everything still buffered,
/// blocking the current thread for at most
/// [`shutdown_timeout`](TracingLayerBuilder::shutdown_timeout). On a
/// current-thread tokio runtime that blocking wait would starve the sender —
/// prefer [`TracingGuard::shutdown`] there, which flushes without blocking.
#[cfg_attr(docsrs, doc(cfg(feature = "tracing")))]
pub struct TracingGuard {
    cancel: CancellationToken,
    done_rx: Option<std::sync::mpsc::Receiver<()>>,
    worker: Option<tokio::task::JoinHandle<()>>,
    dropped: Arc<AtomicU64>,
    shutdown_timeout: Duration,
}

impl TracingGuard {
    /// Number of events that were not delivered: dropped because the queue
    /// was full (drop-newest) or the sender had already shut down, plus
    /// entries discarded after a delivery failure exhausted the client's
    /// retries.
    #[must_use]
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Stop the background sender and wait for it to flush everything still
    /// buffered, without blocking a runtime thread.
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        self.done_rx = None; // Drop must not wait again.
        if let Some(worker) = self.worker.take() {
            let _ = worker.await;
        }
    }
}

impl Drop for TracingGuard {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(done_rx) = self.done_rx.take() {
            // Bounded wait for the final flush. If the runtime is gone the
            // sender half is dropped and this returns immediately.
            let _ = done_rx.recv_timeout(self.shutdown_timeout);
        }
    }
}

struct Worker {
    logs: Logs,
    rx: mpsc::Receiver<LogEntryInput>,
    flush_after: usize,
    flush_interval: Duration,
    cancel: CancellationToken,
    dropped: Arc<AtomicU64>,
    done_tx: std::sync::mpsc::Sender<()>,
}

impl Worker {
    async fn run(self) {
        let Worker {
            logs,
            mut rx,
            flush_after,
            flush_interval,
            cancel,
            dropped,
            done_tx,
        } = self;

        let mut buffer: Vec<LogEntryInput> = Vec::new();
        let mut ticker = tokio::time::interval(flush_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        ticker.tick().await; // the first tick completes immediately

        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                received = rx.recv() => match received {
                    Some(entry) => {
                        buffer.push(entry);
                        if buffer.len() >= flush_after {
                            flush(&logs, &mut buffer, &dropped).await;
                            ticker.reset();
                        }
                    }
                    // All senders gone — the layer itself was dropped.
                    None => break,
                },
                _ = ticker.tick() => {
                    if !buffer.is_empty() {
                        flush(&logs, &mut buffer, &dropped).await;
                    }
                }
            }
        }

        // Drain whatever is still queued (bounded by the channel capacity),
        // then flush one last time.
        while let Ok(entry) = rx.try_recv() {
            buffer.push(entry);
        }
        flush(&logs, &mut buffer, &dropped).await;
        let _ = done_tx.send(());
    }
}

async fn flush(logs: &Logs, buffer: &mut Vec<LogEntryInput>, dropped: &AtomicU64) {
    for chunk in buffer.chunks(MAX_BATCH_ENTRIES) {
        let result = IN_FLUSH.scope(true, logs.write_batch(chunk)).await;
        // Failures past the client's own retries are counted and discarded —
        // surfacing them through `tracing` would feed back into the layer.
        if result.is_err() {
            dropped.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
    }
    buffer.clear();
}

/// `TRACE` → `debug`, `DEBUG` → `debug`, `INFO` → `info`, `WARN` → `warning`,
/// `ERROR` → `error` (confish levels follow RFC 5424).
fn map_level(level: &Level) -> LogLevel {
    if *level == Level::ERROR {
        LogLevel::Error
    } else if *level == Level::WARN {
        LogLevel::Warning
    } else if *level == Level::INFO {
        LogLevel::Info
    } else {
        LogLevel::Debug // DEBUG and TRACE
    }
}

/// Collects an event's fields: `message` becomes the entry message, everything
/// else lands in the context object.
#[derive(Default)]
struct FieldVisitor {
    message: String,
    fields: serde_json::Map<String, serde_json::Value>,
}

impl FieldVisitor {
    fn record_value(&mut self, field: &Field, value: serde_json::Value) {
        if field.name() == "message" {
            self.message = match value {
                serde_json::Value::String(text) => text,
                other => other.to_string(),
            };
        } else {
            self.fields.insert(field.name().to_string(), value);
        }
    }
}

impl Visit for FieldVisitor {
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record_value(field, value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record_value(field, value.into());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        // NaN and infinities have no JSON representation — record null.
        let value = serde_json::Number::from_f64(value)
            .map_or(serde_json::Value::Null, serde_json::Value::Number);
        self.record_value(field, value);
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record_value(field, value.into());
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_value(field, value.into());
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.record_value(field, value.to_string().into());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record_value(field, format!("{value:?}").into());
    }
}

/// Format a timestamp as RFC 3339 / ISO 8601 UTC with millisecond precision,
/// e.g. `2026-07-12T10:15:30.123Z`. Times before the epoch clamp to the epoch.
fn format_rfc3339(time: SystemTime) -> String {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since_epoch.as_secs();
    let millis = since_epoch.subsec_millis();
    let (year, month, day) = civil_from_days(secs / 86_400);
    let (hour, minute, second) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Days since 1970-01-01 to a `(year, month, day)` civil date — Howard
/// Hinnant's `civil_from_days`, unsigned variant (valid from 1970 onward).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64, millis: u32) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs) + Duration::from_millis(u64::from(millis))
    }

    #[test]
    fn formats_the_epoch() {
        assert_eq!(format_rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn formats_known_timestamps() {
        assert_eq!(
            format_rfc3339(at(946_684_799, 0)),
            "1999-12-31T23:59:59.000Z"
        );
        assert_eq!(
            format_rfc3339(at(1_700_000_000, 123)),
            "2023-11-14T22:13:20.123Z"
        );
        assert_eq!(
            format_rfc3339(at(1_782_864_000, 7)),
            "2026-07-01T00:00:00.007Z"
        );
    }

    #[test]
    fn formats_leap_days() {
        assert_eq!(
            format_rfc3339(at(1_709_164_800, 0)),
            "2024-02-29T00:00:00.000Z"
        );
        // 2000 was a leap year despite being a century year.
        assert_eq!(
            format_rfc3339(at(951_782_400, 0)),
            "2000-02-29T00:00:00.000Z"
        );
    }

    #[test]
    fn maps_all_five_tracing_levels() {
        assert_eq!(map_level(&Level::TRACE), LogLevel::Debug);
        assert_eq!(map_level(&Level::DEBUG), LogLevel::Debug);
        assert_eq!(map_level(&Level::INFO), LogLevel::Info);
        assert_eq!(map_level(&Level::WARN), LogLevel::Warning);
        assert_eq!(map_level(&Level::ERROR), LogLevel::Error);
    }
}
