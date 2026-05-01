//! Webhook signature verification.
//!
//! Always pass the raw, unparsed request body to [`verify`] — re-serializing
//! parsed JSON alters byte order and breaks signature comparison.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;

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

/// Verify a confish webhook signature.
///
/// Returns `true` only if the signature matches AND the timestamp is within the
/// tolerance window. Uses constant-time comparison.
pub fn verify(body: &[u8], signature: Option<&str>, secret: &str, opts: &VerifyOptions) -> bool {
    let Some(signature) = signature else {
        return false;
    };
    if signature.is_empty() || secret.is_empty() {
        return false;
    }

    let trimmed = signature.trim();
    let Some((ts_str, sig_hex)) = parse_signature(trimmed) else {
        return false;
    };

    let Ok(ts) = ts_str.parse::<u64>() else {
        return false;
    };

    if opts.tolerance > Duration::ZERO {
        let now = opts.now.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });
        let drift = now.abs_diff(ts);
        if drift > opts.tolerance.as_secs() {
            return false;
        }
    }

    let Ok(provided) = hex::decode(sig_hex) else {
        return false;
    };

    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(format!("{ts}:").as_bytes());
    mac.update(body);

    mac.verify_slice(&provided).is_ok()
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
