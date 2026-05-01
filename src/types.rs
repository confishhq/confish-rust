#![allow(missing_docs)] // enum variants and struct fields document themselves

use serde::{Deserialize, Serialize};

/// Log severity levels accepted by the logging API.
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
