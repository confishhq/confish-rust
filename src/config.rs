use reqwest::Method;
use serde::{de::DeserializeOwned, Serialize};

use crate::client::Client;
use crate::error::Result;

/// Wraps the `/c/{env}` configuration endpoints.
#[derive(Clone)]
pub struct Config {
    client: Client,
}

impl Config {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    /// Fetch the environment's typed configuration.
    pub async fn fetch<T: DeserializeOwned>(&self) -> Result<T> {
        let env_id = &self.client.inner.env_id;
        self.client
            .inner
            .http
            .request(Method::GET, &format!("/c/{env_id}"), None::<&()>)
            .await
    }

    /// Partially update configuration values (PATCH). Returns the full updated config.
    pub async fn update<T: DeserializeOwned, V: Serialize>(&self, values: &V) -> Result<T> {
        let env_id = &self.client.inner.env_id;
        let body = serde_json::json!({ "values": values });
        self.client
            .inner
            .http
            .request(Method::PATCH, &format!("/c/{env_id}"), Some(&body))
            .await
    }

    /// Replace all configuration values (PUT). Omitted fields reset to defaults.
    pub async fn replace<T: DeserializeOwned, V: Serialize>(&self, values: &V) -> Result<T> {
        let env_id = &self.client.inner.env_id;
        let body = serde_json::json!({ "values": values });
        self.client
            .inner
            .http
            .request(Method::PUT, &format!("/c/{env_id}"), Some(&body))
            .await
    }
}
