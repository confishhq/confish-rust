#![allow(missing_docs)] // error variants document themselves through their #[error(...)] strings

use std::collections::HashMap;

use serde::Deserialize;
use thiserror::Error;

/// All errors the SDK can return.
#[derive(Debug, Error)]
pub enum Error {
    /// HTTP 401 — missing or invalid API key.
    #[error("HTTP 401: {message}")]
    Auth {
        message: String,
        body: Option<serde_json::Value>,
    },

    /// HTTP 403 — API key doesn't match the environment, or the application is disabled.
    #[error("HTTP 403: {message}")]
    Forbidden {
        message: String,
        body: Option<serde_json::Value>,
    },

    /// HTTP 409 — typically the action is no longer actionable.
    ///
    /// The action consumer silently skips actions that fail to acknowledge with this error.
    #[error("HTTP 409: {message}")]
    Conflict {
        message: String,
        body: Option<serde_json::Value>,
    },

    /// HTTP 422 — request body failed validation.
    #[error("HTTP 422: {message}")]
    Validation {
        message: String,
        errors: HashMap<String, Vec<String>>,
        body: Option<serde_json::Value>,
    },

    /// HTTP 429 — rate limit exceeded. Headers are populated from the response when present.
    #[error("HTTP 429: {message}")]
    RateLimit {
        message: String,
        retry_after: Option<u64>,
        limit: Option<u64>,
        remaining: Option<u64>,
        body: Option<serde_json::Value>,
    },

    /// HTTP 5xx — server-side error.
    #[error("HTTP {status}: {message}")]
    Server {
        status: u16,
        message: String,
        body: Option<serde_json::Value>,
    },

    /// Any other non-2xx response that doesn't fit the categories above.
    #[error("HTTP {status}: {message}")]
    Api {
        status: u16,
        message: String,
        body: Option<serde_json::Value>,
    },

    /// Transport-level failure (DNS, TCP, TLS, refused connection, timeout).
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    /// Failed to serialize a request body.
    #[error("serialization error: {0}")]
    Serialize(serde_json::Error),

    /// Failed to deserialize a response body.
    #[error("deserialization error: {0}")]
    Deserialize(serde_json::Error),
}

impl Error {
    /// True if this error represents an HTTP 409 conflict — useful for action consumers.
    #[must_use]
    pub fn is_conflict(&self) -> bool {
        matches!(self, Error::Conflict { .. })
    }
}

#[derive(Deserialize)]
struct ErrorBody {
    error: Option<String>,
    message: Option<String>,
    errors: Option<HashMap<String, Vec<String>>>,
}

pub(crate) fn from_response(
    status: u16,
    body_text: &str,
    headers: &reqwest::header::HeaderMap,
) -> Error {
    let body_value: Option<serde_json::Value> = serde_json::from_str(body_text).ok();
    let parsed: Option<ErrorBody> = serde_json::from_str(body_text).ok();
    let message = parsed
        .as_ref()
        .and_then(|e| e.error.clone().or_else(|| e.message.clone()))
        .unwrap_or_else(|| format!("Request failed ({status})"));

    match status {
        401 => Error::Auth {
            message,
            body: body_value,
        },
        403 => Error::Forbidden {
            message,
            body: body_value,
        },
        409 => Error::Conflict {
            message,
            body: body_value,
        },
        422 => Error::Validation {
            message,
            errors: parsed.and_then(|e| e.errors).unwrap_or_default(),
            body: body_value,
        },
        429 => Error::RateLimit {
            message,
            retry_after: parse_int_header(headers, "retry-after"),
            limit: parse_int_header(headers, "x-ratelimit-limit"),
            remaining: parse_int_header(headers, "x-ratelimit-remaining"),
            body: body_value,
        },
        s if s >= 500 => Error::Server {
            status: s,
            message,
            body: body_value,
        },
        s => Error::Api {
            status: s,
            message,
            body: body_value,
        },
    }
}

fn parse_int_header(headers: &reqwest::header::HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}

pub type Result<T> = std::result::Result<T, Error>;
