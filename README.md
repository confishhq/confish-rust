# confish

Official Rust SDK for [confish](https://confi.sh) — typed configuration, actions, and webhook verification.

- Async-first, built on `tokio` + `reqwest`
- Typed configuration via the standard `serde::Deserialize` generic — `client.fetch::<MyConfig>().await?`
- Long-running action consumer with `CancellationToken` and bounded concurrency
- HMAC-SHA256 webhook verification

## Install

```toml
[dependencies]
confish = "0.1"
```

By default the SDK uses `rustls`. For native TLS, opt in:

```toml
confish = { version = "0.1", default-features = false, features = ["native-tls"] }
```

Requires Rust 1.75+.

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

    let config: MyConfig = client.fetch().await?;
    println!("{config:?}");
    Ok(())
}
```

## Reading and writing config

```rust
// GET /c/{env_id}
let config: MyConfig = client.fetch().await?;

// PATCH — only listed fields change
#[derive(serde::Serialize)]
struct Patch { maintenance_mode: bool }
let updated: MyConfig = client.update(&Patch { maintenance_mode: true }).await?;

// PUT — replaces everything; omitted fields reset to defaults
let new_config: MyConfig = client.replace(&MyConfig {
    site_name: "My App".into(),
    max_upload_mb: 50,
    maintenance_mode: false,
    allowed_origins: vec!["https://example.com".into()],
}).await?;
```

> Write access must be enabled in environment settings before `update` and `replace` will work.

## Logging

```rust
use confish::LogLevel;
use serde_json::json;

client.logger().info("Worker started", Some(json!({"region": "eu-west-1"}))).await?;
client.logger().error("Job failed", Some(json!({"job_id": "abc"}))).await?;

// Or directly:
let log_id = client.log(LogLevel::Critical, "system down", None).await?;
```

Levels: `Debug`, `Info`, `Notice`, `Warning`, `Error`, `Critical`, `Alert`.

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
                "place_order" => {
                    ctx.update("Submitting order", Some(json!({"params": action.params}))).await?;
                    // ... do work ...
                    Ok(Some(json!({"order_id": "abc123", "filled_price": 66980.0})))
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
client.actions().update("action_id", "progress", Some(json!({"step": 2}))).await?;
client.actions().complete("action_id", Some(json!({"order_id": "abc"}))).await?;
client.actions().fail("action_id", Some(json!({"error": "timeout"}))).await?;
```

To extract typed params:

```rust
#[derive(serde::Deserialize)]
struct OrderParams { symbol: String, size: f64 }

if let Some(params) = action.params.clone() {
    let order: OrderParams = serde_json::from_value(params)?;
}
```

## Webhook verification

```rust
use confish::webhook::{verify, VerifyOptions};

async fn handler(req: actix_web::HttpRequest, body: actix_web::web::Bytes) -> actix_web::HttpResponse {
    let signature = req.headers().get("x-confish-signature")
        .and_then(|v| v.to_str().ok());
    let secret = std::env::var("CONFISH_WEBHOOK_SECRET").unwrap();

    if !verify(&body, signature, &secret, &VerifyOptions::default()) {
        return actix_web::HttpResponse::Unauthorized().body("invalid signature");
    }

    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // handle payload ...
    actix_web::HttpResponse::Ok().finish()
}
```

`verify` uses constant-time comparison and rejects timestamps older than 5 minutes by default. Pass `VerifyOptions { tolerance: Duration::ZERO, .. }` to disable. Always pass the **raw, unparsed body** — re-serializing parsed JSON breaks verification.

## Errors

```rust
use confish::Error;

match client.fetch::<serde_json::Value>().await {
    Ok(_) => {}
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
