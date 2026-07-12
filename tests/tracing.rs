#![cfg(feature = "tracing")]

use std::time::Duration;

use confish::{Client, Error, TracingLayer};
use serde_json::json;
use tracing::Dispatch;
use tracing_subscriber::layer::SubscriberExt;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn build_client(uri: &str) -> Client {
    Client::builder("env_test", "confish_sk_test")
        .base_url(uri)
        .max_retries(1)
        .max_retry_delay(Duration::from_millis(1))
        .build()
        .expect("client")
}

async fn mount_batch_endpoint(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/c/env_test/logs"))
        .and(header("authorization", "Bearer confish_sk_test"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"ids": []})))
        .mount(server)
        .await;
}

/// All batch requests received so far, as one entry list per request.
async fn received_batches(server: &MockServer) -> Vec<Vec<serde_json::Value>> {
    server
        .received_requests()
        .await
        .expect("requests")
        .iter()
        .filter(|request| request.url.path() == "/c/env_test/logs")
        .map(|request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            body["entries"].as_array().expect("entries array").clone()
        })
        .collect()
}

async fn received_entries(server: &MockServer) -> Vec<serde_json::Value> {
    received_batches(server)
        .await
        .into_iter()
        .flatten()
        .collect()
}

/// Poll until at least `at_least` entries arrived or two seconds elapsed.
async fn wait_for_entries(server: &MockServer, at_least: usize) -> Vec<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let entries = received_entries(server).await;
        if entries.len() >= at_least || tokio::time::Instant::now() >= deadline {
            return entries;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn maps_tracing_levels_per_rfc_5424() {
    let server = MockServer::start().await;
    mount_batch_endpoint(&server).await;

    let client = build_client(&server.uri());
    let (layer, guard) = TracingLayer::builder(&client).build().expect("layer");
    let dispatch = Dispatch::new(tracing_subscriber::registry().with(layer));

    tracing::dispatcher::with_default(&dispatch, || {
        tracing::trace!("crawl queued");
        tracing::debug!("crawl scheduled");
        tracing::info!("crawl started");
        tracing::warn!("crawl retrying");
        tracing::error!("crawl failed");
    });

    guard.shutdown().await;

    let entries = received_entries(&server).await;
    let levels: Vec<&str> = entries
        .iter()
        .map(|e| e["level"].as_str().unwrap())
        .collect();
    assert_eq!(levels, ["debug", "debug", "info", "warning", "error"]);
    assert_eq!(entries[0]["message"], "crawl queued");
    assert_eq!(entries[4]["message"], "crawl failed");
}

#[tokio::test(flavor = "multi_thread")]
async fn captures_event_fields_as_context() {
    let server = MockServer::start().await;
    mount_batch_endpoint(&server).await;

    let client = build_client(&server.uri());
    let (layer, guard) = TracingLayer::builder(&client).build().expect("layer");
    let dispatch = Dispatch::new(tracing_subscriber::registry().with(layer));

    tracing::dispatcher::with_default(&dispatch, || {
        tracing::info!(
            job = "db-backup",
            attempt = 2,
            dry_run = false,
            "Job started"
        );
        tracing::info!("no fields here");
    });

    guard.shutdown().await;

    let entries = received_entries(&server).await;
    assert_eq!(entries.len(), 2);

    assert_eq!(entries[0]["message"], "Job started");
    assert_eq!(
        entries[0]["context"],
        json!({"job": "db-backup", "attempt": 2, "dry_run": false})
    );
    let timestamp = entries[0]["timestamp"].as_str().expect("timestamp");
    assert_eq!(timestamp.len(), "2026-07-12T10:15:30.123Z".len());
    assert!(timestamp.ends_with('Z'));
    assert_eq!(&timestamp[10..11], "T");

    // An event without fields sends no context key at all.
    assert_eq!(entries[1]["message"], "no fields here");
    assert!(entries[1].get("context").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn flushes_when_the_batch_threshold_is_reached() {
    let server = MockServer::start().await;
    mount_batch_endpoint(&server).await;

    let client = build_client(&server.uri());
    let (layer, _guard) = TracingLayer::builder(&client)
        .flush_after(3)
        .flush_interval(Duration::from_secs(60))
        .build()
        .expect("layer");
    let dispatch = Dispatch::new(tracing_subscriber::registry().with(layer));

    tracing::dispatcher::with_default(&dispatch, || {
        tracing::info!("incident opened");
        tracing::info!("incident acknowledged");
        tracing::info!("incident resolved");
    });

    // No guard drop, no interval — only the size trigger can flush here.
    let entries = wait_for_entries(&server, 3).await;
    assert_eq!(entries.len(), 3);
    assert_eq!(received_batches(&server).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn flushes_on_the_interval_before_the_threshold() {
    let server = MockServer::start().await;
    mount_batch_endpoint(&server).await;

    let client = build_client(&server.uri());
    let (layer, _guard) = TracingLayer::builder(&client)
        .flush_after(1000)
        .flush_interval(Duration::from_millis(50))
        .build()
        .expect("layer");
    let dispatch = Dispatch::new(tracing_subscriber::registry().with(layer));

    tracing::dispatcher::with_default(&dispatch, || {
        tracing::info!("job heartbeat");
        tracing::info!("job heartbeat");
    });

    let entries = wait_for_entries(&server, 2).await;
    assert_eq!(entries.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn chunks_flushes_to_at_most_100_entries_per_request() {
    let server = MockServer::start().await;
    mount_batch_endpoint(&server).await;

    let client = build_client(&server.uri());
    let (layer, guard) = TracingLayer::builder(&client)
        .flush_after(500)
        .flush_interval(Duration::from_secs(60))
        .build()
        .expect("layer");
    let dispatch = Dispatch::new(tracing_subscriber::registry().with(layer));

    tracing::dispatcher::with_default(&dispatch, || {
        for page in 0..250 {
            tracing::info!(page, "page crawled");
        }
    });

    guard.shutdown().await;

    let batches = received_batches(&server).await;
    let sizes: Vec<usize> = batches.iter().map(Vec::len).collect();
    assert_eq!(sizes, [100, 100, 50]);
}

#[tokio::test]
async fn counts_dropped_events_when_the_queue_overflows() {
    let server = MockServer::start().await;
    mount_batch_endpoint(&server).await;

    let client = build_client(&server.uri());
    let (layer, guard) = TracingLayer::builder(&client)
        .capacity(5)
        .flush_after(1000)
        .flush_interval(Duration::from_secs(60))
        .build()
        .expect("layer");
    let dispatch = Dispatch::new(tracing_subscriber::registry().with(layer));

    // Current-thread runtime: the sender task cannot run while this closure
    // emits synchronously, so the queue deterministically overflows.
    tracing::dispatcher::with_default(&dispatch, || {
        for job in 0..12 {
            tracing::info!(job, "job finished");
        }
    });

    assert_eq!(guard.dropped_count(), 7);

    guard.shutdown().await;

    // Drop-newest: the first five events survive.
    let entries = received_entries(&server).await;
    let jobs: Vec<i64> = entries
        .iter()
        .map(|e| e["context"]["job"].as_i64().unwrap())
        .collect();
    assert_eq!(jobs, [0, 1, 2, 3, 4]);
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_guard_flushes_pending_entries() {
    let server = MockServer::start().await;
    mount_batch_endpoint(&server).await;

    let client = build_client(&server.uri());
    let (layer, guard) = TracingLayer::builder(&client)
        .flush_after(1000)
        .flush_interval(Duration::from_secs(60))
        .build()
        .expect("layer");
    let dispatch = Dispatch::new(tracing_subscriber::registry().with(layer));

    tracing::dispatcher::with_default(&dispatch, || {
        tracing::warn!("backup overdue");
        tracing::error!("backup failed");
    });

    // The layer is still alive inside `dispatch` — only the guard drop can
    // trigger this flush, and it blocks until the flush lands.
    drop(guard);

    let entries = received_entries(&server).await;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["message"], "backup overdue");
    assert_eq!(entries[1]["message"], "backup failed");
}

#[test]
fn build_outside_a_runtime_fails_gracefully() {
    let client = Client::builder("env_test", "confish_sk_test")
        .build()
        .expect("client");
    let result = TracingLayer::builder(&client).build();
    match result {
        Err(Error::Api { message, .. }) => assert!(message.contains("tokio runtime")),
        _ => panic!("expected a graceful error when built outside a runtime"),
    }
}
