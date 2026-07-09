use std::sync::Arc;
use std::time::Duration;

use crate::actions::Actions;
use crate::config::Config;
use crate::error::Result;
use crate::feeds::Feed;
use crate::http::HttpClient;
use crate::logs::Logs;

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

    /// Access the configuration namespace.
    #[must_use]
    pub fn config(&self) -> Config {
        Config::new(self.clone())
    }

    /// Access the logs namespace.
    #[must_use]
    pub fn logs(&self) -> Logs {
        Logs::new(self.clone())
    }

    /// Access the actions namespace.
    #[must_use]
    pub fn actions(&self) -> Actions {
        Actions::new(self.clone())
    }

    /// Return a handle bound to the feed with the given slug.
    ///
    /// No HTTP happens on construction — an unknown slug only surfaces as
    /// [`Error::NotFound`](crate::Error::NotFound) when a method is called.
    #[must_use]
    pub fn feed(&self, slug: impl Into<String>) -> Feed {
        Feed::new(self.clone(), slug.into())
    }
}
