#![allow(missing_docs)] // enum variants and struct fields document themselves

use serde::{Deserialize, Serialize};

/// Log severity levels accepted by the logging API (the full RFC 5424 set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Notice,
    Warning,
    Error,
    Critical,
    Alert,
    Emergency,
}

/// An action's lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionStatus {
    Pending,
    Acknowledged,
    Completed,
    Failed,
    Expired,
}

/// A single timeline entry on an [`Action`].
#[derive(Debug, Clone, Deserialize)]
pub struct ActionUpdate {
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// An item returned by the feeds API.
///
/// `data` is generic so callers can decode items into their own types — use
/// [`FeedItem<serde_json::Value>`] to keep it untyped.
#[derive(Debug, Clone, Deserialize)]
pub struct FeedItem<T> {
    pub id: String,
    pub external_id: String,
    pub data: T,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// Counts returned by [`Feed::replace`](crate::Feed::replace).
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct FeedReplaceResult {
    pub created: u64,
    pub updated: u64,
    pub deleted: u64,
}

/// An action returned by the actions API.
///
/// `params` and `result` stay as `serde_json::Value` so callers can decode them into
/// their own types via [`serde_json::from_value`].
#[derive(Debug, Clone, Deserialize)]
pub struct Action {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: ActionStatus,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
    #[serde(default)]
    pub updates: Vec<ActionUpdate>,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub acknowledged_at: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}
