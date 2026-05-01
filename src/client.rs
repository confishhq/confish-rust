use std::sync::Arc;
use std::time::Duration;

use reqwest::Method;
use serde::{de::DeserializeOwned, Serialize};

use crate::actions::Actions;
use crate::error::Result;
use crate::http::HttpClient;
use crate::types::LogLevel;

/// The default API base URL (`https://confi.sh`).
pub const DEFAULT_BASE_URL: &str = "https://confi.sh";

/// Builder for [`Client`].
pub struct ClientBuilder {
    env_id: String,
    api_key: String,
    base_url: String,
    user_agent: String,
    http: Option<reqwest::Client>,
    max_retries: u32,
    max_retry_delay: Duration,
}

impl ClientBuilder {
    /// Create a new builder with the required env_id and api_key.
    pub fn new(env_id: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            env_id: env_id.into(),
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_string(),
            user_agent: "confish-rust".to_string(),
            http: None,
            max_retries: 2,
            max_retry_delay: Duration::from_secs(30),
        }
    }

    /// Override the API base URL (e.g. for self-hosted deployments).
    #[must_use]
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Override the User-Agent header.
    #[must_use]
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    /// Inject a pre-configured [`reqwest::Client`].
    #[must_use]
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.http = Some(client);
        self
    }

    /// Maximum number of retry attempts beyond the initial request for 429/5xx responses.
    /// Defaults to 2.
    #[must_use]
    pub fn max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Cap on the delay between retries (e.g. when honoring `Retry-After`).
    /// Defaults to 30 seconds.
    #[must_use]
    pub fn max_retry_delay(mut self, max_retry_delay: Duration) -> Self {
        self.max_retry_delay = max_retry_delay;
        self
    }

    /// Build the [`Client`].
    pub fn build(self) -> Result<Client> {
        if self.env_id.is_empty() {
            return Err(crate::error::Error::Api {
                status: 0,
                message: "env_id is required".to_string(),
                body: None,
            });
        }
        if self.api_key.is_empty() {
            return Err(crate::error::Error::Api {
                status: 0,
                message: "api_key is required".to_string(),
                body: None,
            });
        }

        let http = self.http.unwrap_or_else(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("default reqwest client")
        });

        let inner = HttpClient::new(
            self.base_url,
            self.api_key,
            self.user_agent,
            http,
            self.max_retries,
            self.max_retry_delay,
        );

        Ok(Client {
            inner: Arc::new(ClientInner {
                http: inner,
                env_id: self.env_id,
            }),
        })
    }
}

pub(crate) struct ClientInner {
    pub http: HttpClient,
    pub env_id: String,
}

/// Asynchronous client for the confish API.
///
/// Cheap to clone — internally an `Arc`.
#[derive(Clone)]
pub struct Client {
    pub(crate) inner: Arc<ClientInner>,
}

impl Client {
    /// Start configuring a new client.
    pub fn builder(env_id: impl Into<String>, api_key: impl Into<String>) -> ClientBuilder {
        ClientBuilder::new(env_id, api_key)
    }

    /// Fetch the environment's typed configuration.
    pub async fn fetch<T: DeserializeOwned>(&self) -> Result<T> {
        self.inner
            .http
            .request(
                Method::GET,
                &format!("/c/{}", self.inner.env_id),
                None::<&()>,
            )
            .await
    }

    /// Partially update configuration values (PATCH). Returns the full updated config.
    pub async fn update<T: DeserializeOwned, V: Serialize>(&self, values: &V) -> Result<T> {
        let body = serde_json::json!({ "values": values });
        self.inner
            .http
            .request(
                Method::PATCH,
                &format!("/c/{}", self.inner.env_id),
                Some(&body),
            )
            .await
    }

    /// Replace all configuration values (PUT). Omitted fields reset to defaults.
    pub async fn replace<T: DeserializeOwned, V: Serialize>(&self, values: &V) -> Result<T> {
        let body = serde_json::json!({ "values": values });
        self.inner
            .http
            .request(
                Method::PUT,
                &format!("/c/{}", self.inner.env_id),
                Some(&body),
            )
            .await
    }

    /// Send a log entry. Returns the new log entry's ID.
    pub async fn log(
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
        let body = Body {
            level,
            message: message.into(),
            context,
        };
        let resp: Resp = self
            .inner
            .http
            .request(
                Method::POST,
                &format!("/c/{}/log", self.inner.env_id),
                Some(&body),
            )
            .await?;
        Ok(resp.id)
    }

    /// Convenience wrapper around [`Client::log`] with one method per level.
    #[must_use]
    pub fn logger(&self) -> Logger<'_> {
        Logger { client: self }
    }

    /// Access the actions namespace.
    #[must_use]
    pub fn actions(&self) -> Actions {
        Actions::new(self.clone())
    }
}

/// Convenience wrapper around [`Client::log`] with one method per level.
pub struct Logger<'a> {
    client: &'a Client,
}

macro_rules! level_method {
    ($name:ident, $level:expr) => {
        /// Send a log entry at this level. Returns the new log entry's ID.
        pub async fn $name(
            &self,
            message: impl Into<String>,
            context: Option<serde_json::Value>,
        ) -> Result<String> {
            self.client.log($level, message, context).await
        }
    };
}

impl Logger<'_> {
    level_method!(debug, LogLevel::Debug);
    level_method!(info, LogLevel::Info);
    level_method!(notice, LogLevel::Notice);
    level_method!(warning, LogLevel::Warning);
    level_method!(error, LogLevel::Error);
    level_method!(critical, LogLevel::Critical);
    level_method!(alert, LogLevel::Alert);
}
