use reqwest::Method;
use serde::Serialize;

use crate::client::Client;
use crate::error::Result;
use crate::types::LogLevel;

/// Wraps the `/c/{env}/log` endpoint, with one convenience method per level.
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

    level_method!(debug, LogLevel::Debug);
    level_method!(info, LogLevel::Info);
    level_method!(notice, LogLevel::Notice);
    level_method!(warning, LogLevel::Warning);
    level_method!(error, LogLevel::Error);
    level_method!(critical, LogLevel::Critical);
    level_method!(alert, LogLevel::Alert);
    level_method!(emergency, LogLevel::Emergency);
}
