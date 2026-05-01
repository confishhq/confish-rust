use confish::{Client, Error};
use serde::{Deserialize, Serialize};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn build_client(uri: &str) -> Client {
    Client::builder("env_test", "confish_sk_test")
        .base_url(uri)
        .max_retries(1)
        .max_retry_delay(std::time::Duration::from_millis(1))
        .build()
        .expect("client")
}

#[tokio::test]
async fn fetch_returns_typed_config() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/c/env_test"))
        .and(header("authorization", "Bearer confish_sk_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "site_name": "My App",
            "max_upload_mb": 25,
            "maintenance_mode": false,
        })))
        .mount(&server)
        .await;

    #[derive(Deserialize)]
    struct MyConfig {
        site_name: String,
        max_upload_mb: u32,
        maintenance_mode: bool,
    }

    let client = build_client(&server.uri());
    let config: MyConfig = client.fetch().await.expect("fetch");
    assert_eq!(config.site_name, "My App");
    assert_eq!(config.max_upload_mb, 25);
    assert!(!config.maintenance_mode);
}

#[tokio::test]
async fn update_wraps_values() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/c/env_test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    #[derive(Serialize)]
    struct Patch {
        maintenance_mode: bool,
    }

    let client = build_client(&server.uri());
    let _: serde_json::Value = client
        .update(&Patch {
            maintenance_mode: true,
        })
        .await
        .expect("update");

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(body, json!({"values": {"maintenance_mode": true}}));
}

#[tokio::test]
async fn auth_error_on_401() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": "Missing API key"})))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    let result: Result<serde_json::Value, _> = client.fetch().await;
    assert!(matches!(result, Err(Error::Auth { .. })));
}

#[tokio::test]
async fn validation_error_exposes_field_errors() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "invalid",
            "errors": {"values.max_upload_mb": ["Must be at most 100."]},
        })))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    let result: Result<serde_json::Value, _> = client.update(&json!({"x": 1})).await;
    match result {
        Err(Error::Validation { errors, .. }) => {
            assert_eq!(
                errors.get("values.max_upload_mb"),
                Some(&vec!["Must be at most 100.".to_string()])
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[tokio::test]
async fn rate_limit_retries_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "0")
                .set_body_json(json!({"error": "limited"})),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    let result: serde_json::Value = client.fetch().await.expect("fetch");
    assert_eq!(result, json!({"ok": true}));
}

#[tokio::test]
async fn rate_limit_exhausts_retries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "0")
                .insert_header("x-ratelimit-limit", "60")
                .set_body_json(json!({"error": "limited"})),
        )
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    let result: Result<serde_json::Value, _> = client.fetch().await;
    match result {
        Err(Error::RateLimit {
            limit: Some(60), ..
        }) => {}
        other => panic!("expected RateLimit, got {other:?}"),
    }
}

#[tokio::test]
async fn logger_sends_level_and_context() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/c/env_test/log"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "log_1"})))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    let id = client
        .logger()
        .info("hello", Some(json!({"user_id": 1})))
        .await
        .expect("log");
    assert_eq!(id, "log_1");

    let received = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(
        body,
        json!({"level": "info", "message": "hello", "context": {"user_id": 1}})
    );
}

#[tokio::test]
async fn builder_validates_required_fields() {
    let result = Client::builder("", "k").build();
    assert!(matches!(result, Err(Error::Api { ref message, .. }) if message.contains("env_id")));
    let result = Client::builder("e", "").build();
    assert!(matches!(result, Err(Error::Api { ref message, .. }) if message.contains("api_key")));
}
