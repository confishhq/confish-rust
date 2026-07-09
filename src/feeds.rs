use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use std::time::Duration;

use reqwest::Method;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::client::Client;
use crate::error::Result;
use crate::types::{FeedItem, FeedReplaceResult};

/// RFC 3986 unreserved characters pass through; everything else is
/// percent-encoded. External IDs are arbitrary user-supplied strings
/// (spaces, unicode, slashes) - encode so the path segment is always
/// well-formed.
const EXTERNAL_ID_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

fn encode_external_id(external_id: &str) -> String {
    utf8_percent_encode(external_id, EXTERNAL_ID_SET).to_string()
}

/// A single item in a [`Feed::replace`] payload.
#[derive(Debug, Clone, Serialize)]
pub struct FeedItemInput<V> {
    /// Client-chosen identifier, 255 characters or fewer. It travels in the
    /// JSON body here (not the URL path), so it needs no percent-encoding.
    pub external_id: String,
    /// The item's data.
    pub data: V,
    /// Optional time-to-live, sent as whole seconds and omitted entirely when
    /// `None` — the item is then permanent. Must be between 1 second and 30
    /// days when set.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_ttl_as_secs"
    )]
    pub ttl: Option<Duration>,
}

impl<V> FeedItemInput<V> {
    /// Build an input item; parameter order matches [`Feed::set`].
    pub fn new(external_id: impl Into<String>, data: V, ttl: Option<Duration>) -> Self {
        Self {
            external_id: external_id.into(),
            data,
            ttl,
        }
    }
}

fn serialize_ttl_as_secs<S: serde::Serializer>(
    ttl: &Option<Duration>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    match ttl {
        Some(ttl) => serializer.serialize_u64(ttl.as_secs()),
        // Unreachable thanks to skip_serializing_if, but keep it total.
        None => serializer.serialize_none(),
    }
}

/// Wraps the `/c/{env}/feeds/{slug}/items` endpoints for a single feed.
///
/// Obtained via [`Client::feed`] — construction performs no HTTP; an unknown
/// slug only surfaces as [`Error::NotFound`](crate::Error::NotFound) when a
/// method is called.
#[derive(Clone)]
pub struct Feed {
    client: Client,
    slug: String,
}

impl Feed {
    pub(crate) fn new(client: Client, slug: String) -> Self {
        Self { client, slug }
    }

    /// Upsert an item by external ID (PUT — create-or-replace, declarative).
    ///
    /// The item's data becomes exactly `data`, and the TTL becomes exactly
    /// `ttl`: passing `None` makes the item permanent, **clearing any TTL set
    /// by a previous call**. TTLs are truncated to whole seconds and must be
    /// between 1 second and 30 days. `external_id` must be 255 characters or
    /// fewer.
    pub async fn set<V: Serialize>(
        &self,
        external_id: &str,
        data: &V,
        ttl: Option<Duration>,
    ) -> Result<FeedItem<serde_json::Value>> {
        let env_id = &self.client.inner.env_id;
        let slug = &self.slug;
        let mut body = serde_json::json!({ "data": data });
        if let Some(ttl) = ttl {
            body["ttl"] = serde_json::json!(ttl.as_secs());
        }
        self.client
            .inner
            .http
            .request(
                Method::PUT,
                &format!(
                    "/c/{env_id}/feeds/{slug}/items/{}",
                    encode_external_id(external_id)
                ),
                Some(&body),
            )
            .await
    }

    /// Replace the environment's entire partition of this feed (collection PUT).
    ///
    /// Built for sync-style cron jobs that push their full dataset in one
    /// request. The partition becomes exactly `items`: existing external IDs
    /// update in place, new ones are created, and **anything absent from
    /// `items` is DELETED** — an empty slice clears the feed.
    ///
    /// All-or-nothing: duplicate external IDs, more items than the plan's
    /// cap, or any schema-invalid item is rejected with
    /// [`Error::Validation`](crate::Error::Validation) and nothing is written.
    pub async fn replace<V: Serialize>(
        &self,
        items: &[FeedItemInput<V>],
    ) -> Result<FeedReplaceResult> {
        let env_id = &self.client.inner.env_id;
        let slug = &self.slug;
        let body = serde_json::json!({ "items": items });
        self.client
            .inner
            .http
            .request(
                Method::PUT,
                &format!("/c/{env_id}/feeds/{slug}/items"),
                Some(&body),
            )
            .await
    }

    /// Return the environment's live (non-expired) items, newest first.
    pub async fn list<T: DeserializeOwned>(&self) -> Result<Vec<FeedItem<T>>> {
        #[derive(Deserialize)]
        struct Wrapper<T> {
            items: Vec<FeedItem<T>>,
        }
        let env_id = &self.client.inner.env_id;
        let slug = &self.slug;
        let resp: Wrapper<T> = self
            .client
            .inner
            .http
            .request(
                Method::GET,
                &format!("/c/{env_id}/feeds/{slug}/items"),
                None::<&()>,
            )
            .await?;
        Ok(resp.items)
    }

    /// Delete an item by external ID. Idempotent — deleting an item that is
    /// already gone succeeds, so retries never error.
    pub async fn delete(&self, external_id: &str) -> Result<()> {
        let env_id = &self.client.inner.env_id;
        let slug = &self.slug;
        self.client
            .inner
            .http
            .request(
                Method::DELETE,
                &format!(
                    "/c/{env_id}/feeds/{slug}/items/{}",
                    encode_external_id(external_id)
                ),
                None::<&()>,
            )
            .await
    }
}
