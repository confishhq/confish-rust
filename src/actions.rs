use std::sync::Arc;
use std::time::Duration;

use reqwest::Method;
use serde::Deserialize;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::client::Client;
use crate::error::{Error, Result};
use crate::types::{Action, ActionStatus};

/// Wraps the `/c/{env}/actions` endpoints.
#[derive(Clone)]
pub struct Actions {
    client: Client,
}

impl Actions {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    /// Return pending, non-expired actions ordered oldest first.
    pub async fn list(&self) -> Result<Vec<Action>> {
        #[derive(Deserialize)]
        struct Wrapper {
            actions: Vec<Action>,
        }
        let env_id = &self.client.inner.env_id;
        let resp: Wrapper = self
            .client
            .inner
            .http
            .request(Method::GET, &format!("/c/{env_id}/actions"), None::<&()>)
            .await?;
        Ok(resp.actions)
    }

    /// Acknowledge an action. Returns [`Error::Conflict`] if it's no longer pending.
    pub async fn ack(&self, action_id: &str) -> Result<Action> {
        let env_id = &self.client.inner.env_id;
        self.client
            .inner
            .http
            .request(
                Method::POST,
                &format!("/c/{env_id}/actions/{action_id}/ack"),
                None::<&()>,
            )
            .await
    }

    /// Append a progress note to the action's timeline, visible in the dashboard.
    pub async fn progress(
        &self,
        action_id: &str,
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) -> Result<Action> {
        let env_id = &self.client.inner.env_id;
        let mut body = serde_json::json!({ "message": message.into() });
        if let Some(d) = data {
            body["data"] = d;
        }
        self.client
            .inner
            .http
            .request(
                Method::POST,
                &format!("/c/{env_id}/actions/{action_id}/update"),
                Some(&body),
            )
            .await
    }

    /// Mark an action as completed.
    pub async fn complete(
        &self,
        action_id: &str,
        result: Option<serde_json::Value>,
    ) -> Result<Action> {
        self.resolve(action_id, "complete", result).await
    }

    /// Mark an action as failed.
    pub async fn fail(&self, action_id: &str, result: Option<serde_json::Value>) -> Result<Action> {
        self.resolve(action_id, "fail", result).await
    }

    async fn resolve(
        &self,
        action_id: &str,
        suffix: &str,
        result: Option<serde_json::Value>,
    ) -> Result<Action> {
        let env_id = &self.client.inner.env_id;
        let body = match result {
            Some(r) => serde_json::json!({ "result": r }),
            None => serde_json::json!({}),
        };
        self.client
            .inner
            .http
            .request(
                Method::POST,
                &format!("/c/{env_id}/actions/{action_id}/{suffix}"),
                Some(&body),
            )
            .await
    }

    /// Long-running consumer loop.
    ///
    /// Polls for pending actions, acknowledges them, runs `handler`, and reports
    /// completion or failure based on the handler's outcome.
    ///
    /// - Returning `Ok(Some(value))` from the handler completes the action with `value` as `result`.
    /// - Returning `Ok(None)` completes with no result.
    /// - Returning `Err(_)` fails the action with `{"error": <error message>}` as result.
    /// - A 409 conflict on ack is silently skipped (safe to run multiple consumers).
    /// - After 3 consecutive empty polls, the sleep doubles up to `max_poll_interval`,
    ///   resetting to `poll_interval` the moment any action is processed.
    /// - Cancelling `cancel_token` halts new work and waits for in-flight handlers to finish.
    pub async fn consume<F, Fut>(&self, handler: F, options: ConsumeOptions) -> Result<()>
    where
        F: Fn(Action, ActionContext) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<
                Output = std::result::Result<
                    Option<serde_json::Value>,
                    Box<dyn std::error::Error + Send + Sync>,
                >,
            > + Send
            + 'static,
    {
        let handler = Arc::new(handler);
        let cancel = options.cancel_token.unwrap_or_default();
        let semaphore = Arc::new(Semaphore::new(options.concurrency.max(1)));
        let mut empty_polls: u32 = 0;
        let mut tasks = Vec::new();

        loop {
            if cancel.is_cancelled() {
                break;
            }

            let actions = match self.list().await {
                Ok(a) => a,
                Err(err) => {
                    if let Some(on_err) = &options.on_error {
                        on_err(err, None);
                    }
                    if sleep_or_cancel(
                        backoff_delay(
                            empty_polls,
                            options.poll_interval,
                            options.max_poll_interval,
                        ),
                        &cancel,
                    )
                    .await
                    {
                        break;
                    }
                    continue;
                }
            };

            let pending: Vec<Action> = actions
                .into_iter()
                .filter(|a| a.status == ActionStatus::Pending)
                .collect();

            if pending.is_empty() {
                empty_polls = empty_polls.saturating_add(1);
                if sleep_or_cancel(
                    backoff_delay(
                        empty_polls,
                        options.poll_interval,
                        options.max_poll_interval,
                    ),
                    &cancel,
                )
                .await
                {
                    break;
                }
                continue;
            }

            empty_polls = 0;

            for action in pending {
                if cancel.is_cancelled() {
                    break;
                }
                let permit = semaphore.clone().acquire_owned().await.expect("semaphore");
                let actions = self.clone();
                let handler = handler.clone();
                let cancel = cancel.clone();
                let on_error = options.on_error.clone();

                tasks.push(tokio::spawn(async move {
                    process_action(actions, action, handler, cancel, on_error).await;
                    drop(permit);
                }));
            }
        }

        for task in tasks {
            let _ = task.await;
        }
        Ok(())
    }
}

/// Passed to handlers so they can append progress notes to the action's timeline.
pub struct ActionContext {
    actions: Actions,
    action_id: String,
    /// The cancellation token shared with [`ConsumeOptions::cancel_token`]. Handlers can poll
    /// it (or `select!` on it) to bail out early when the consumer is shutting down.
    pub cancel_token: CancellationToken,
}

impl ActionContext {
    /// Append a progress note to the action's timeline, visible in the dashboard.
    pub async fn progress(
        &self,
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) -> Result<Action> {
        self.actions.progress(&self.action_id, message, data).await
    }
}

/// Callback invoked when listing fails, when ack fails (other than 409), or when a
/// handler returns an error. The second parameter is `None` if the failure happened
/// before an action was picked up (e.g. listing failed).
pub type ErrorCallback = Arc<dyn Fn(Error, Option<Action>) + Send + Sync>;

/// Configuration for [`Actions::consume`].
#[derive(Clone)]
pub struct ConsumeOptions {
    /// Base delay between polls when no actions are pending. Default: 15 seconds.
    pub poll_interval: Duration,
    /// Cap on the adaptive backoff delay. Default: 60 seconds.
    pub max_poll_interval: Duration,
    /// Maximum number of actions processed in parallel. Default: 1 (sequential).
    pub concurrency: usize,
    /// Cancellation token. Cancel it to stop the loop. Default: a fresh token (never cancelled).
    pub cancel_token: Option<CancellationToken>,
    /// Optional error callback.
    pub on_error: Option<ErrorCallback>,
}

impl Default for ConsumeOptions {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(15),
            max_poll_interval: Duration::from_secs(60),
            concurrency: 1,
            cancel_token: None,
            on_error: None,
        }
    }
}

async fn process_action<F, Fut>(
    actions: Actions,
    action: Action,
    handler: Arc<F>,
    cancel: CancellationToken,
    on_error: Option<ErrorCallback>,
) where
    F: Fn(Action, ActionContext) -> Fut + Send + Sync,
    Fut: std::future::Future<
            Output = std::result::Result<
                Option<serde_json::Value>,
                Box<dyn std::error::Error + Send + Sync>,
            >,
        > + Send,
{
    if let Err(err) = actions.ack(&action.id).await {
        if err.is_conflict() {
            return;
        }
        if let Some(cb) = &on_error {
            cb(err, Some(action));
        }
        return;
    }

    let ctx = ActionContext {
        actions: actions.clone(),
        action_id: action.id.clone(),
        cancel_token: cancel.clone(),
    };

    let result = handler(action.clone(), ctx).await;

    // Once acknowledged, always finalize — don't orphan in-flight actions on cancel.
    let _ = cancel; // suppress unused-var warning

    match result {
        Ok(value) => {
            if let Err(err) = actions.complete(&action.id, value).await {
                if let Some(cb) = &on_error {
                    cb(err, Some(action));
                }
            }
        }
        Err(handler_err) => {
            let message = handler_err.to_string();
            if let Some(cb) = &on_error {
                // Wrap arbitrary handler errors into our Error type for the callback.
                cb(
                    Error::Api {
                        status: 0,
                        message: message.clone(),
                        body: None,
                    },
                    Some(action.clone()),
                );
            }
            if let Err(fail_err) = actions
                .fail(&action.id, Some(serde_json::json!({ "error": message })))
                .await
            {
                if let Some(cb) = &on_error {
                    cb(fail_err, Some(action));
                }
            }
        }
    }
}

/// Returns true if the wait was cancelled (caller should break out of the loop).
async fn sleep_or_cancel(d: Duration, cancel: &CancellationToken) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => false,
        _ = cancel.cancelled() => true,
    }
}

/// Holds at base for the first 3 empty polls, then doubles each subsequent empty poll
/// up to max. Exposed for testing.
pub(crate) fn backoff_delay(empty_polls: u32, base: Duration, max: Duration) -> Duration {
    if empty_polls <= 3 {
        return base;
    }
    let factor = 1u64 << (empty_polls - 3).min(20); // cap shift to avoid overflow
    let candidate = base.saturating_mul(factor.try_into().unwrap_or(u32::MAX));
    candidate.min(max)
}
