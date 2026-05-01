use std::time::Duration;

use reqwest::{header, Client as ReqwestClient, Method};
use serde::Serialize;

use crate::error::{from_response, Error, Result};

#[derive(Clone)]
pub(crate) struct HttpClient {
    base_url: String,
    api_key: String,
    user_agent: String,
    client: ReqwestClient,
    max_retries: u32,
    max_retry_delay: Duration,
}

impl HttpClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        user_agent: impl Into<String>,
        client: ReqwestClient,
        max_retries: u32,
        max_retry_delay: Duration,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            user_agent: user_agent.into(),
            client,
            max_retries,
            max_retry_delay,
        }
    }

    pub async fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&(impl Serialize + ?Sized)>,
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);

        let mut attempt: u32 = 0;
        loop {
            let mut req = self
                .client
                .request(method.clone(), &url)
                .header(header::AUTHORIZATION, format!("Bearer {}", self.api_key))
                .header(header::ACCEPT, "application/json")
                .header(header::USER_AGENT, &self.user_agent);

            if let Some(body) = body {
                req = req.json(body);
            }

            let response = req.send().await?;
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            let text = response.text().await?;

            if (200..300).contains(&status) {
                if text.is_empty() {
                    // Caller asked for T but server returned nothing — only OK for unit-like T.
                    return serde_json::from_str("null").map_err(Error::Deserialize);
                }
                return serde_json::from_str::<T>(&text).map_err(Error::Deserialize);
            }

            let err = from_response(status, &text, &headers);
            if !self.should_retry(attempt, &err) {
                return Err(err);
            }

            tokio::time::sleep(self.retry_delay(attempt, &err)).await;
            attempt += 1;
        }
    }

    fn should_retry(&self, attempt: u32, err: &Error) -> bool {
        if attempt >= self.max_retries {
            return false;
        }
        matches!(err, Error::RateLimit { .. } | Error::Server { .. })
    }

    fn retry_delay(&self, attempt: u32, err: &Error) -> Duration {
        if let Error::RateLimit {
            retry_after: Some(ra),
            ..
        } = err
        {
            return Duration::from_secs(*ra).min(self.max_retry_delay);
        }
        Duration::from_secs(2u64.saturating_pow(attempt)).min(self.max_retry_delay)
    }
}
