use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use confish::{Client, ConsumeOptions, Error};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn pending_action_json(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "type": "noop",
        "status": "pending",
        "params": null,
        "updates": [],
        "result": null,
        "expires_at": null,
        "acknowledged_at": null,
        "completed_at": null,
        "created_at": null,
    })
}

fn build_client(uri: &str) -> Client {
    Client::builder("env_test", "confish_sk_test")
        .base_url(uri)
        .max_retries(1)
        .max_retry_delay(Duration::from_millis(1))
        .build()
        .expect("client")
}

#[tokio::test]
async fn list_unwraps_actions_array() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/c/env_test/actions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "actions": [pending_action_json("a1"), pending_action_json("a2")]
        })))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    let actions = client.actions().list().await.expect("list");
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].id, "a1");
}

#[tokio::test]
async fn complete_with_result_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/c/env_test/actions/a1/complete"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pending_action_json("a1")))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    client
        .actions()
        .complete("a1", Some(json!({"order_id": "abc"})))
        .await
        .expect("complete");

    let received = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(body, json!({"result": {"order_id": "abc"}}));
}

#[tokio::test]
async fn complete_without_result() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/c/env_test/actions/a1/complete"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pending_action_json("a1")))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    client
        .actions()
        .complete("a1", None)
        .await
        .expect("complete");

    let received = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(body, json!({}));
}

#[tokio::test]
async fn progress_posts_to_update_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/c/env_test/actions/a1/update"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pending_action_json("a1")))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    client
        .actions()
        .progress("a1", "Submitting order", Some(json!({"step": 2})))
        .await
        .expect("progress");

    let received = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(
        body,
        json!({"message": "Submitting order", "data": {"step": 2}})
    );
}

#[tokio::test]
async fn ack_returns_conflict_on_409() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/c/env_test/actions/a1/ack"))
        .respond_with(
            ResponseTemplate::new(409).set_body_json(json!({"error": "already acknowledged"})),
        )
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    let err = client.actions().ack("a1").await.unwrap_err();
    assert!(err.is_conflict(), "expected Conflict, got {err:?}");
    assert!(matches!(err, Error::Conflict { .. }));
}

#[tokio::test]
async fn consume_processes_action_and_completes() {
    let server = MockServer::start().await;
    // First poll returns one action.
    Mock::given(method("GET"))
        .and(path("/c/env_test/actions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"actions": [pending_action_json("a1")]})),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // Subsequent polls return empty.
    Mock::given(method("GET"))
        .and(path("/c/env_test/actions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"actions": []})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/c/env_test/actions/a1/ack"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pending_action_json("a1")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/c/env_test/actions/a1/complete"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pending_action_json("a1")))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    let cancel = CancellationToken::new();
    let cancel_for_handler = cancel.clone();
    let handler_called = Arc::new(AtomicBool::new(false));
    let handler_called_clone = handler_called.clone();

    let consume = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .actions()
                .consume(
                    move |action, _ctx| {
                        let cancel = cancel_for_handler.clone();
                        let flag = handler_called_clone.clone();
                        async move {
                            assert_eq!(action.id, "a1");
                            flag.store(true, Ordering::SeqCst);
                            // Stop the loop after this action.
                            cancel.cancel();
                            Ok(Some(json!({"filled": true})))
                        }
                    },
                    ConsumeOptions {
                        poll_interval: Duration::from_millis(5),
                        max_poll_interval: Duration::from_millis(10),
                        cancel_token: Some(cancel.clone()),
                        ..Default::default()
                    },
                )
                .await
        }
    });

    consume.await.expect("join").expect("consume");

    assert!(handler_called.load(Ordering::SeqCst));

    let received = server.received_requests().await.unwrap();
    let complete = received
        .iter()
        .find(|r| r.url.path().ends_with("/complete"))
        .expect("complete request");
    let body: serde_json::Value = serde_json::from_slice(&complete.body).unwrap();
    assert_eq!(body, json!({"result": {"filled": true}}));
}

#[tokio::test]
async fn consume_fails_on_handler_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/c/env_test/actions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"actions": [pending_action_json("a1")]})),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/c/env_test/actions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"actions": []})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/c/env_test/actions/a1/ack"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pending_action_json("a1")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/c/env_test/actions/a1/fail"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pending_action_json("a1")))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    client
        .actions()
        .consume(
            move |_action, _ctx| {
                let cancel = cancel_clone.clone();
                async move {
                    cancel.cancel();
                    Err::<Option<serde_json::Value>, _>("boom".into())
                }
            },
            ConsumeOptions {
                poll_interval: Duration::from_millis(5),
                cancel_token: Some(cancel.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("consume");

    let received = server.received_requests().await.unwrap();
    let fail = received
        .iter()
        .find(|r| r.url.path().ends_with("/fail"))
        .expect("fail request");
    let body: serde_json::Value = serde_json::from_slice(&fail.body).unwrap();
    assert_eq!(body, json!({"result": {"error": "boom"}}));
}

#[tokio::test]
async fn consume_skips_on_409_ack() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/c/env_test/actions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"actions": [pending_action_json("a1")]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/c/env_test/actions/a1/ack"))
        .respond_with(
            ResponseTemplate::new(409).set_body_json(json!({"error": "already acknowledged"})),
        )
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    let handler_call_count = Arc::new(AtomicUsize::new(0));
    let counter = handler_call_count.clone();

    // Cancel after a short delay — long enough for several poll/ack cycles.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel_clone.cancel();
    });

    client
        .actions()
        .consume(
            move |_action, _ctx| {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok(None)
                }
            },
            ConsumeOptions {
                poll_interval: Duration::from_millis(5),
                cancel_token: Some(cancel.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("consume");

    assert_eq!(handler_call_count.load(Ordering::SeqCst), 0);
}
