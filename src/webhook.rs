//! Webhook signature verification.
//!
//! Always pass the raw, unparsed request body to [`verify`] — re-serializing
//! parsed JSON alters byte order and breaks signature comparison.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

/// Default tolerance window — rejects timestamps older or newer than 5 minutes.
pub const DEFAULT_TOLERANCE: Duration = Duration::from_secs(300);

/// Options for [`verify`].
pub struct VerifyOptions {
    /// Tolerance window. Pass `Duration::ZERO` to disable timestamp checking entirely.
    pub tolerance: Duration,
    /// Override the current time (for testing).
    pub now: Option<u64>,
}

impl Default for VerifyOptions {
    fn default() -> Self {
        Self {
            tolerance: DEFAULT_TOLERANCE,
            now: None,
        }
    }
}

/// Why [`verify`] rejected a webhook.
#[derive(Debug, Error)]
pub enum WebhookError {
    /// No signature header was provided.
    #[error("missing signature header")]
    MissingSignature,

    /// The signature header doesn't match the `ts=<digits>;sig=<hex>` format.
    #[error("malformed signature header")]
    MalformedSignature,

    /// The signature doesn't match the body — wrong secret or tampered payload.
    #[error("invalid signature")]
    InvalidSignature,

    /// The signed timestamp is outside the tolerance window.
    #[error("timestamp outside tolerance: drift of {drift_secs}s exceeds {tolerance_secs}s")]
    TimestampOutsideTolerance {
        /// Absolute difference between the signed timestamp and now, in seconds.
        drift_secs: u64,
        /// The configured tolerance window, in seconds.
        tolerance_secs: u64,
    },

    /// The signature checked out but the body isn't a valid webhook payload.
    #[error("invalid payload: {0}")]
    InvalidPayload(#[from] serde_json::Error),
}

/// The application block of a webhook [`Payload`].
#[derive(Debug, Clone, Deserialize)]
#[allow(missing_docs)] // fields document themselves
pub struct PayloadApplication {
    pub name: String,
}

/// The environment block of a webhook [`Payload`].
#[derive(Debug, Clone, Deserialize)]
#[allow(missing_docs)] // fields document themselves
pub struct PayloadEnvironment {
    pub name: String,
    pub env_id: String,
    pub url: String,
}

/// A webhook payload, returned by [`verify`] once the signature and timestamp check out.
#[derive(Debug, Clone, Deserialize)]
#[allow(missing_docs)] // fields document themselves
pub struct Payload {
    /// Event name, e.g. `environment.updated` or `environment.deleted`.
    pub event: String,
    /// ISO 8601 delivery timestamp.
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub application: Option<PayloadApplication>,
    #[serde(default)]
    pub environment: Option<PayloadEnvironment>,
    /// Changed fields — present on `environment.updated` when the server includes them.
    #[serde(default)]
    pub changes: Option<serde_json::Value>,
    /// Full configuration values — present when "include values" is enabled.
    #[serde(default)]
    pub values: Option<serde_json::Value>,
}

/// Verify a confish webhook signature and parse the payload in one operation.
///
/// Returns the parsed [`Payload`] only if the signature matches AND the
/// timestamp is within the tolerance window; the error says why verification
/// failed. Uses constant-time comparison.
pub fn verify(
    body: &[u8],
    signature: Option<&str>,
    secret: &str,
    opts: &VerifyOptions,
) -> Result<Payload, WebhookError> {
    let Some(signature) = signature else {
        return Err(WebhookError::MissingSignature);
    };
    if signature.is_empty() {
        return Err(WebhookError::MissingSignature);
    }
    if secret.is_empty() {
        return Err(WebhookError::InvalidSignature);
    }

    let trimmed = signature.trim();
    let Some((ts_str, sig_hex)) = parse_signature(trimmed) else {
        return Err(WebhookError::MalformedSignature);
    };

    let Ok(ts) = ts_str.parse::<u64>() else {
        return Err(WebhookError::MalformedSignature);
    };

    // HMAC before tolerance, so TimestampOutsideTolerance always means
    // "authentic but stale" - a forged payload must never report a
    // timestamp problem.
    let Ok(provided) = hex::decode(sig_hex) else {
        return Err(WebhookError::MalformedSignature);
    };

    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return Err(WebhookError::InvalidSignature);
    };
    mac.update(format!("{ts}:").as_bytes());
    mac.update(body);

    if mac.verify_slice(&provided).is_err() {
        return Err(WebhookError::InvalidSignature);
    }

    if opts.tolerance > Duration::ZERO {
        let now = opts.now.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });
        let drift = now.abs_diff(ts);
        if drift > opts.tolerance.as_secs() {
            return Err(WebhookError::TimestampOutsideTolerance {
                drift_secs: drift,
                tolerance_secs: opts.tolerance.as_secs(),
            });
        }
    }

    Ok(serde_json::from_slice(body)?)
}

fn parse_signature(s: &str) -> Option<(&str, &str)> {
    // Format: ts=<digits>;sig=<hex>
    let mut ts: Option<&str> = None;
    let mut sig: Option<&str> = None;
    for part in s.split(';') {
        if let Some(value) = part.strip_prefix("ts=") {
            if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
                ts = Some(value);
            } else {
                return None;
            }
        } else if let Some(value) = part.strip_prefix("sig=") {
            if !value.is_empty() && value.bytes().all(|b| b.is_ascii_hexdigit()) {
                sig = Some(value);
            } else {
                return None;
            }
        } else {
            return None;
        }
    }
    Some((ts?, sig?))
}
