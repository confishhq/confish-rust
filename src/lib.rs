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
//! let config: MyConfig = client.fetch().await?;
//! println!("{config:?}");
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

mod actions;
mod client;
mod error;
mod http;
mod types;

pub mod webhook;

pub use actions::{ActionContext, Actions, ConsumeOptions, ErrorCallback};
pub use client::{Client, ClientBuilder, Logger, DEFAULT_BASE_URL};
pub use error::{Error, Result};
pub use types::{Action, ActionStatus, ActionUpdate, LogLevel};
