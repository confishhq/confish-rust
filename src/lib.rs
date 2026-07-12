//! Official Rust SDK for [confish](https://confi.sh).
//!
//! ## Quick start
//!
//! ```no_run
//! use confish::Client;
//! use serde::Deserialize;
//!
//! #[derive(Deserialize, Debug)]
//! struct MyConfig {
//!     site_name: String,
//!     max_upload_mb: u32,
//!     maintenance_mode: bool,
//! }
//!
//! # async fn run() -> confish::Result<()> {
//! let client = Client::builder("env_id", "confish_sk_...").build()?;
//! let config: MyConfig = client.config().fetch().await?;
//! println!("{config:?}");
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod actions;
mod client;
mod config;
mod error;
mod feeds;
mod http;
mod logs;
#[cfg(feature = "tracing")]
mod tracing_layer;
mod types;

pub mod webhook;

pub use actions::{ActionContext, Actions, ConsumeOptions, ErrorCallback};
pub use client::{Client, ClientBuilder, DEFAULT_BASE_URL};
pub use config::Config;
pub use error::{Error, Result};
pub use feeds::{Feed, FeedItemInput};
pub use logs::{LogEntryInput, Logs};
#[cfg(feature = "tracing")]
pub use tracing_layer::{TracingGuard, TracingLayer, TracingLayerBuilder};
pub use types::{Action, ActionStatus, ActionUpdate, FeedItem, FeedReplaceResult, LogLevel};
