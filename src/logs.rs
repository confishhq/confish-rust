use reqwest::Method;
use serde::Serialize;

use crate::client::Client;
use crate::error::{Error, Result};
use crate::types::LogLevel;

/// Server-side cap on entries per batch request.
pub(crate) const MAX_BATCH_ENTRIES: usize = 100;

/// A single entry in a [`Logs::write_batch`] payload.
#[derive(Debug, Clone, Serialize)]
pub struct LogEntryInput {
    /// Severity level.
    pub level: LogLevel,
    /// The log message.
    pub message: String,
    /// Optional structured context, shown alongside the entry in the dashboard.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
    /// Optional ISO 8601 timestamp. When omitted the server stamps the entry
    /// at receipt time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

impl LogEntryInput {
    /// Build an entry; parameter order matches [`Logs::write`].
    pub fn new(
        level: LogLevel,
        message: impl Into<String>,
        context: Option<serde_json::Value>,
    ) -> Self {
        Self {
            level,
            message: message.into(),
            context,
            timestamp: None,
        }
    }

    /// Attach an explicit ISO 8601 timestamp instead of the server's receipt
    /// time — useful when you buffered the entry yourself.
    #[must_use]
    pub fn timestamp(mut self, timestamp: impl Into<String>) -> Self {
        self.timestamp = Some(timestamp.into());
        self
    }
}

/// Wraps the `/c/{env}/log` and `/c/{env}/logs` endpoints, with one
/// convenience method per level plus batch writing.
#[derive(Clone)]
pub struct Logs {
    client: Client,
}

macro_rules! level_method {
    ($name:ident, $level:expr) => {
        /// Send a log entry at this level. Returns the new log entry's ID.
        pub async fn $name(
            &self,
            message: impl Into<String>,
            context: Option<serde_json::Value>,
        ) -> Result<String> {
            self.write($level, message, context).await
        }
    };
}

impl Logs {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    /// Send a log entry. Returns the new log entry's ID.
    pub async fn write(
        &self,
        level: LogLevel,
        message: impl Into<String>,
        context: Option<serde_json::Value>,
    ) -> Result<String> {
        #[derive(Serialize)]
        struct Body {
            level: LogLevel,
            message: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            context: Option<serde_json::Value>,
        }
        #[derive(serde::Deserialize)]
        struct Resp {
            id: String,
        }
        let env_id = &self.client.inner.env_id;
        let body = Body {
            level,
            message: message.into(),
            context,
        };
        let resp: Resp = self
            .client
            .inner
            .http
            .request(Method::POST, &format!("/c/{env_id}/log"), Some(&body))
            .await?;
        Ok(resp.id)
    }

    /// Send up to 100 log entries in one request. Returns the new entries'
    /// IDs, in input order.
    ///
    /// More than 100 entries fails fast with a client-side error before any
    /// request is made — chunk larger backfills yourself. An empty slice
    /// sends nothing and returns an empty `Vec`.
    pub async fn write_batch(&self, entries: &[LogEntryInput]) -> Result<Vec<String>> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        if entries.len() > MAX_BATCH_ENTRIES {
            return Err(Error::Api {
                status: 0,
                message: format!(
                    "write_batch accepts at most {MAX_BATCH_ENTRIES} entries per request, got {}",
                    entries.len()
                ),
                body: None,
            });
        }
        #[derive(serde::Deserialize)]
        struct Resp {
            ids: Vec<String>,
        }
        let env_id = &self.client.inner.env_id;
        let body = serde_json::json!({ "entries": entries });
        let resp: Resp = self
            .client
            .inner
            .http
            .request(Method::POST, &format!("/c/{env_id}/logs"), Some(&body))
            .await?;
        Ok(resp.ids)
    }

    level_method!(debug, LogLevel::Debug);
    level_method!(info, LogLevel::Info);
    level_method!(notice, LogLevel::Notice);
    level_method!(warning, LogLevel::Warning);
    level_method!(error, LogLevel::Error);
    level_method!(critical, LogLevel::Critical);
    level_method!(alert, LogLevel::Alert);
    level_method!(emergency, LogLevel::Emergency);
}
