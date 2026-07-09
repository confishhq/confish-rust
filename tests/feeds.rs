use std::time::Duration;

use confish::{Client, Error, FeedItem, FeedItemInput};
use serde::{Deserialize, Serialize};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn feed_item_json(external_id: &str) -> serde_json::Value {
    json!({
        "id": "fi_1",
        "external_id": external_id,
        "data": {"url": "https://example.com/sitemap.xml", "status": "done"},
        "expires_at": null,
        "created_at": "2026-07-09T12:00:00+00:00",
        "updated_at": "2026-07-09T12:00:00+00:00",
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
async fn set_puts_data_with_ttl_in_whole_seconds() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/c/env_test/feeds/jobs/items/sitemap-crawl"))
        .and(header("authorization", "Bearer confish_sk_test"))
        .respond_with(ResponseTemplate::new(201).set_body_json(feed_item_json("sitemap-crawl")))
        .mount(&server)
        .await;

    #[derive(Serialize)]
    struct Job {
        url: String,
        status: String,
    }

    let client = build_client(&server.uri());
    let item = client
        .feed("jobs")
        .set(
            "sitemap-crawl",
            &Job {
                url: "https://example.com/sitemap.xml".into(),
                status: "done".into(),
            },
            Some(Duration::from_secs(86400)),
        )
        .await
        .expect("set");
    assert_eq!(item.external_id, "sitemap-crawl");
    assert_eq!(
        item.data,
        json!({"url": "https://example.com/sitemap.xml", "status": "done"})
    );

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(
        body,
        json!({
            "data": {"url": "https://example.com/sitemap.xml", "status": "done"},
            "ttl": 86400,
        })
    );
}

#[tokio::test]
async fn set_without_ttl_omits_the_key() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/c/env_test/feeds/jobs/items/sitemap-crawl"))
        .respond_with(ResponseTemplate::new(200).set_body_json(feed_item_json("sitemap-crawl")))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    client
        .feed("jobs")
        .set("sitemap-crawl", &json!({"status": "queued"}), None)
        .await
        .expect("set");

    let received = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(body, json!({"data": {"status": "queued"}}));
    assert!(
        body.get("ttl").is_none(),
        "ttl key must be omitted when None so the server clears any existing TTL"
    );
}

#[tokio::test]
async fn set_percent_encodes_external_id_in_path() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(201).set_body_json(feed_item_json("crawl #1/2")))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    client
        .feed("jobs")
        .set("crawl #1/2", &json!({"status": "queued"}), None)
        .await
        .expect("set");

    let received = server.received_requests().await.unwrap();
    assert_eq!(
        received[0].url.path(),
        "/c/env_test/feeds/jobs/items/crawl%20%231%2F2"
    );
}

#[tokio::test]
async fn replace_puts_full_item_list_and_parses_counts() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/c/env_test/feeds/jobs/items"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"created": 1, "updated": 1, "deleted": 2})),
        )
        .mount(&server)
        .await;

    #[derive(Serialize)]
    struct Job {
        status: String,
    }

    let client = build_client(&server.uri());
    let result = client
        .feed("jobs")
        .replace(&[
            FeedItemInput::new(
                "db-backup",
                Job {
                    status: "running".into(),
                },
                Some(Duration::from_secs(3600)),
            ),
            FeedItemInput::new(
                "sitemap-crawl",
                Job {
                    status: "queued".into(),
                },
                None,
            ),
        ])
        .await
        .expect("replace");
    assert_eq!(result.created, 1);
    assert_eq!(result.updated, 1);
    assert_eq!(result.deleted, 2);

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(
        body,
        json!({"items": [
            {"external_id": "db-backup", "data": {"status": "running"}, "ttl": 3600},
            {"external_id": "sitemap-crawl", "data": {"status": "queued"}},
        ]})
    );
    assert!(
        body["items"][1].get("ttl").is_none(),
        "per-item ttl key must be omitted when None"
    );
}

#[tokio::test]
async fn replace_with_empty_slice_clears_the_feed() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/c/env_test/feeds/jobs/items"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"created": 0, "updated": 0, "deleted": 7})),
        )
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    let result = client
        .feed("jobs")
        .replace::<serde_json::Value>(&[])
        .await
        .expect("replace");
    assert_eq!(result.deleted, 7);

    let received = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(body, json!({"items": []}));
}

#[tokio::test]
async fn replace_rejects_invalid_batch_as_validation_error() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/c/env_test/feeds/jobs/items"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "error": "Duplicate external_id in payload: sitemap-crawl",
        })))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    let result = client
        .feed("jobs")
        .replace(&[
            FeedItemInput::new("sitemap-crawl", json!({"status": "queued"}), None),
            FeedItemInput::new("sitemap-crawl", json!({"status": "running"}), None),
        ])
        .await;
    match result {
        Err(Error::Validation { message, .. }) => {
            assert!(message.starts_with("Duplicate external_id"));
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[tokio::test]
async fn list_unwraps_typed_items() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/c/env_test/feeds/jobs/items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [feed_item_json("sitemap-crawl"), feed_item_json("db-backup")]
        })))
        .mount(&server)
        .await;

    #[derive(Deserialize)]
    struct Job {
        url: String,
        status: String,
    }

    let client = build_client(&server.uri());
    let items: Vec<FeedItem<Job>> = client.feed("jobs").list().await.expect("list");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].external_id, "sitemap-crawl");
    assert_eq!(items[0].data.url, "https://example.com/sitemap.xml");
    assert_eq!(items[0].data.status, "done");
    assert_eq!(items[1].external_id, "db-backup");
}

#[tokio::test]
async fn delete_returns_unit_on_204() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/c/env_test/feeds/jobs/items/sitemap-crawl"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    client
        .feed("jobs")
        .delete("sitemap-crawl")
        .await
        .expect("delete");
    // Idempotent: the server 204s even when the item is already gone.
    client
        .feed("jobs")
        .delete("sitemap-crawl")
        .await
        .expect("repeat delete");

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 2);
}

#[tokio::test]
async fn unknown_feed_slug_returns_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/c/env_test/feeds/nope/items"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "Feed not found"})))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    let result: Result<Vec<FeedItem<serde_json::Value>>, _> = client.feed("nope").list().await;
    match result {
        Err(Error::NotFound { message, .. }) => assert_eq!(message, "Feed not found"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[tokio::test]
async fn full_feed_returns_validation_error() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/c/env_test/feeds/jobs/items/db-backup"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "error": "Feed is full (100 items). Delete items, set TTLs, or upgrade your plan.",
        })))
        .mount(&server)
        .await;

    let client = build_client(&server.uri());
    let result = client
        .feed("jobs")
        .set("db-backup", &json!({"status": "queued"}), None)
        .await;
    match result {
        Err(Error::Validation { message, .. }) => assert!(message.starts_with("Feed is full")),
        other => panic!("expected Validation, got {other:?}"),
    }
}
