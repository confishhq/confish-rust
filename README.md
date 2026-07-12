# confish

Official Rust SDK for [confish](https://confi.sh) — typed configuration, actions, logs, feeds, and webhook verification.

- Async-first, built on `tokio` + `reqwest`
- Typed configuration via the standard `serde::Deserialize` generic — `client.config().fetch::<MyConfig>().await?`
- Live feeds with declarative upserts and optional TTLs
- Long-running action consumer with `CancellationToken` and bounded concurrency
- HMAC-SHA256 webhook verification that returns the parsed payload

## Install

```toml
[dependencies]
confish = "0.2"
```

By default the SDK uses `rustls`. For native TLS, opt in:

```toml
confish = { version = "0.2", default-features = false, features = ["native-tls"] }
```

Requires Rust 1.86+.

## Quick start

```rust
use confish::Client;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
struct MyConfig {
    site_name: String,
    max_upload_mb: u32,
    maintenance_mode: bool,
    allowed_origins: Vec<String>,
}

#[tokio::main]
async fn main() -> confish::Result<()> {
    let client = Client::builder(
        std::env::var("CONFISH_ENV_ID").unwrap(),
        std::env::var("CONFISH_API_KEY").unwrap(),
    )
    .build()?;

    let config: MyConfig = client.config().fetch().await?;
    println!("{config:?}");
    Ok(())
}
```

## Reading and writing config

```rust
// GET /c/{env_id}
let config: MyConfig = client.config().fetch().await?;

// PATCH — only listed fields change
#[derive(serde::Serialize)]
struct Patch { maintenance_mode: bool }
let updated: MyConfig = client.config().update(&Patch { maintenance_mode: true }).await?;

// PUT — replaces everything; omitted fields reset to defaults
let new_config: MyConfig = client.config().replace(&MyConfig {
    site_name: "My App".into(),
    max_upload_mb: 50,
    maintenance_mode: false,
    allowed_origins: vec!["https://example.com".into()],
}).await?;
```

> Write access must be enabled in environment settings before `update` and `replace` will work.

## Feeds

A feed holds your environment's live state — active jobs, crawl results, open incidents — keyed by an external ID you choose. `client.feed(slug)` returns a bound handle; construction performs no HTTP.

```rust
use confish::FeedItem;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Deserialize)]
struct CrawlResult { url: String, pages: u32 }

let crawls = client.feed("crawl-results");

// PUT — create-or-replace by external ID, with an optional TTL
crawls.set(
    "sitemap-crawl",
    &CrawlResult { url: "https://example.com/sitemap.xml".into(), pages: 214 },
    Some(Duration::from_secs(86400)),
).await?;

// GET — live (non-expired) items, newest first
let items: Vec<FeedItem<CrawlResult>> = crawls.list().await?;

// DELETE — idempotent; deleting an item that's already gone succeeds
crawls.delete("sitemap-crawl").await?;
```

`set` is declarative: the item's data becomes exactly what you send, and the TTL becomes exactly what you pass. **Passing `None` for `ttl` makes the item permanent — it clears any TTL set by a previous call.** TTLs are sent as whole seconds and must be between 1 second and 30 days; `external_id` must be 255 characters or fewer.

### Replacing the whole feed

`replace` is a collection PUT built for sync-style cron jobs that push their full dataset in one request — the feed becomes exactly the items you send. Existing external IDs update in place, new ones are created, and **anything absent is deleted**; an empty slice clears the feed. It's all-or-nothing: duplicate external IDs, payloads over the plan's item cap, or any schema-invalid item are rejected with `Error::Validation` and nothing is written.

```rust
use confish::FeedItemInput;
use serde_json::json;
use std::time::Duration;

let result = client.feed("jobs").replace(&[
    FeedItemInput::new("db-backup", json!({"status": "running"}), Some(Duration::from_secs(3600))),
    FeedItemInput::new("sitemap-crawl", json!({"status": "queued"}), None),
]).await?;
println!("created {}, updated {}, deleted {}", result.created, result.updated, result.deleted);
```

An unknown feed slug returns `Error::NotFound`. A full feed or an item that fails the feed's schema returns `Error::Validation`.

## Logging

```rust
use confish::LogLevel;
use serde_json::json;

client.logs().info("Worker started", Some(json!({"region": "eu-west-1"}))).await?;
client.logs().error("Job failed", Some(json!({"job_id": "abc"}))).await?;

// Or with an explicit level:
let log_id = client.logs().write(LogLevel::Critical, "system down", None).await?;
```

Levels (the full RFC 5424 set): `Debug`, `Info`, `Notice`, `Warning`, `Error`, `Critical`, `Alert`, `Emergency`. Because the levels follow RFC 5424 (syslog), they map cleanly onto the level vocabularies of the `log` and `tracing` crates.

### Batch writes

`write_batch` sends up to 100 entries in a single request and returns their IDs in input order. Each entry carries a level, a message, optional context, and an optional ISO 8601 timestamp — set one to backdate entries you buffered yourself; leave it off and the server stamps the entry on receipt.

```rust
use confish::{LogEntryInput, LogLevel};
use serde_json::json;

let ids = client.logs().write_batch(&[
    LogEntryInput::new(LogLevel::Info, "Crawl started", Some(json!({"pages": 214}))),
    LogEntryInput::new(LogLevel::Warning, "Two pages timed out", None)
        .timestamp("2026-07-10T08:30:00Z"),
]).await?;
```

More than 100 entries fails fast client-side — no request is made — so chunk larger backfills. An empty slice sends nothing and returns no IDs.

### Use as a `tracing` layer

If you already instrument your code with the [`tracing`](https://docs.rs/tracing) crate, you can ship those events to confish without touching a single call site. Enable the `tracing` feature:

```toml
[dependencies]
confish = { version = "0.3", features = ["tracing"] }
```

Then add the layer to your subscriber stack. Building the layer requires a running tokio runtime (it spawns a background sender), so build it inside `#[tokio::main]` — and hold on to the guard for as long as you log:

```rust
use confish::TracingLayer;
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> confish::Result<()> {
    let client = confish::Client::builder("env_id", "confish_sk_...").build()?;

    let (layer, guard) = TracingLayer::builder(&client).build()?;
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(layer)
        .init();

    tracing::info!(job = "db-backup", "Job started");
    tracing::warn!(url = "https://example.com/sitemap.xml", attempt = 2, "Crawl retrying");
    tracing::error!(incident = "inc-42", "Health check failed");

    // ... run your workload ...

    guard.shutdown().await; // flush everything before exiting
    Ok(())
}
```

How it behaves:

- **Non-blocking.** Emitting an event only pushes onto a bounded in-memory queue (default capacity 1000) — it never blocks and never panics. A background task batches entries and sends them: at 50 entries or every 5 seconds, whichever comes first, chunked to at most 100 entries per request.
- **Level mapping.** `tracing` has five levels, confish follows RFC 5424: `TRACE` → `debug`, `DEBUG` → `debug`, `INFO` → `info`, `WARN` → `warning`, `ERROR` → `error`.
- **Fields become context.** `info!(job = "db-backup", attempt = 2, "Job started")` produces the message `Job started` with context `{"job": "db-backup", "attempt": 2}`. Timestamps are captured when you emit the event, not when the batch is sent. Span fields are not captured in this version — event fields only.
- **Overflow drops the newest.** When the queue is full the incoming event is dropped rather than blocking you; `guard.dropped_count()` tells you how many went missing.
- **Errors never come back.** Delivery failures are retried per the client's retry policy, then counted in `dropped_count()` and discarded — the layer never emits `tracing` events of its own, so it can't feed back into itself.
- **Shutdown.** Dropping the guard flushes whatever is buffered, blocking for at most `shutdown_timeout` (default 5 seconds). On a current-thread runtime prefer `guard.shutdown().await`, which flushes without blocking the runtime.

Tune the knobs on the builder: `capacity`, `flush_after`, `flush_interval`, and `shutdown_timeout`.

## Actions

The action consumer polls for pending actions, acknowledges them, runs your handler, and reports completion or failure — including idempotent skip if another consumer claimed the action first.

```rust
use confish::{Action, ActionContext, ConsumeOptions};
use serde_json::json;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

let cancel = CancellationToken::new();
let cancel_clone = cancel.clone();
tokio::spawn(async move {
    tokio::signal::ctrl_c().await.ok();
    cancel_clone.cancel();
});

client
    .actions()
    .consume(
        |action: Action, ctx: ActionContext| async move {
            match action.kind.as_str() {
                "run_crawl" => {
                    ctx.progress("Crawling sitemap", Some(json!({"params": action.params}))).await?;
                    // ... do work ...
                    Ok(Some(json!({"pages_indexed": 214, "duration_ms": 5320})))
                }
                other => Err(format!("unknown action type: {other}").into()),
            }
        },
        ConsumeOptions {
            poll_interval: Duration::from_secs(15),    // base — defaults to 15s
            max_poll_interval: Duration::from_secs(60), // adaptive backoff cap
            concurrency: 2,
            cancel_token: Some(cancel),
            ..Default::default()
        },
    )
    .await?;
```

What happens automatically:
- Returning `Ok(Some(value))` completes the action with `value` as `result`.
- Returning `Ok(None)` completes with no result.
- Returning `Err(_)` fails the action with `{"error": <message>}`.
- A `409 Conflict` on ack is silently skipped — safe to run multiple consumers.
- Cancelling the token halts new work and waits for in-flight handlers to finalize.
- After 3 consecutive empty polls the loop doubles its sleep up to `max_poll_interval`, resetting to `poll_interval` the moment any action is processed. Idle consumers make ~240 requests/hour by default.

You can also drive the lifecycle manually:

```rust
let actions = client.actions().list().await?;
client.actions().ack("action_id").await?;
client.actions().progress("action_id", "Closing 3 stale incidents", Some(json!({"step": 2}))).await?;
client.actions().complete("action_id", Some(json!({"closed": 3}))).await?;
client.actions().fail("action_id", Some(json!({"error": "timeout"}))).await?;
```

To extract typed params:

```rust
#[derive(serde::Deserialize)]
struct CrawlParams { url: String, max_depth: u32 }

if let Some(params) = action.params.clone() {
    let crawl: CrawlParams = serde_json::from_value(params)?;
}
```

## Webhook verification

`verify` checks the signature and parses the payload in one operation — the Stripe pattern. On success you get the parsed `Payload`; on failure a `WebhookError` that says why (`InvalidSignature` vs `TimestampOutsideTolerance`), so the failure mode can't be ignored.

```rust
use confish::webhook::{verify, VerifyOptions, WebhookError};

async fn handler(req: actix_web::HttpRequest, body: actix_web::web::Bytes) -> actix_web::HttpResponse {
    let signature = req.headers().get("x-confish-signature")
        .and_then(|v| v.to_str().ok());
    let secret = std::env::var("CONFISH_WEBHOOK_SECRET").unwrap();

    let payload = match verify(&body, signature, &secret, &VerifyOptions::default()) {
        Ok(payload) => payload,
        Err(WebhookError::TimestampOutsideTolerance { .. }) => {
            return actix_web::HttpResponse::Unauthorized().body("stale webhook");
        }
        Err(_) => {
            return actix_web::HttpResponse::Unauthorized().body("invalid signature");
        }
    };

    match payload.event.as_str() {
        "environment.updated" => { /* reload config ... */ }
        "environment.deleted" => { /* clean up ... */ }
        _ => {}
    }
    actix_web::HttpResponse::Ok().finish()
}
```

`verify` uses constant-time comparison and rejects timestamps older than 5 minutes by default. Pass `VerifyOptions { tolerance: Duration::ZERO, .. }` to disable. Always pass the **raw, unparsed body** — re-serializing parsed JSON breaks verification.

## Errors

```rust
use confish::Error;

match client.config().fetch::<serde_json::Value>().await {
    Ok(_) => {}
    Err(Error::NotFound { message, .. }) => eprintln!("not found: {message}"),
    Err(Error::RateLimit { retry_after, .. }) => {
        eprintln!("retry after {retry_after:?}s");
    }
    Err(Error::Validation { errors, .. }) => {
        for (field, msgs) in errors { eprintln!("{field}: {msgs:?}"); }
    }
    Err(Error::Auth { message, .. }) => eprintln!("auth failed: {message}"),
    Err(e) => eprintln!("error: {e}"),
}
```

`Error::is_conflict()` is a convenience for action consumers.

By default the client retries `429` (honoring `Retry-After`) and `5xx` responses up to twice. Tune via the builder.

## Builder options

```rust
let client = Client::builder("env_id", "confish_sk_...")
    .base_url("https://confi.sh")           // override for self-hosted
    .user_agent("my-app/1.0")
    .max_retries(2)
    .max_retry_delay(Duration::from_secs(30))
    .http_client(reqwest::Client::new())   // inject your own
    .build()?;
```

## License

MIT
