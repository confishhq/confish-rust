use confish::webhook::{verify, VerifyOptions, DEFAULT_TOLERANCE};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::Duration;

fn sign(secret: &str, ts: u64, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(format!("{ts}:").as_bytes());
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

#[test]
fn accepts_valid_signature() {
    let body = br#"{"event":"environment.updated"}"#;
    let ts = 1_700_000_000;
    let sig = sign("whsec_test", ts, body);
    let header = format!("ts={ts};sig={sig}");
    let opts = VerifyOptions {
        tolerance: DEFAULT_TOLERANCE,
        now: Some(ts),
    };
    assert!(verify(body, Some(&header), "whsec_test", &opts));
}

#[test]
fn rejects_wrong_secret() {
    let body = br#"{}"#;
    let ts = 1_700_000_000;
    let sig = sign("other", ts, body);
    let header = format!("ts={ts};sig={sig}");
    let opts = VerifyOptions {
        tolerance: DEFAULT_TOLERANCE,
        now: Some(ts),
    };
    assert!(!verify(body, Some(&header), "whsec_test", &opts));
}

#[test]
fn rejects_tampered_body() {
    let secret = "whsec_test";
    let ts = 1_700_000_000;
    let sig = sign(secret, ts, br#"{"a":1}"#);
    let header = format!("ts={ts};sig={sig}");
    let opts = VerifyOptions {
        tolerance: DEFAULT_TOLERANCE,
        now: Some(ts),
    };
    assert!(!verify(br#"{"a":2}"#, Some(&header), secret, &opts));
}

#[test]
fn rejects_stale_timestamp() {
    let secret = "whsec_test";
    let ts = 1_700_000_000;
    let sig = sign(secret, ts, b"{}");
    let header = format!("ts={ts};sig={sig}");
    let opts = VerifyOptions {
        tolerance: Duration::from_secs(300),
        now: Some(ts + 600),
    };
    assert!(!verify(b"{}", Some(&header), secret, &opts));
}

#[test]
fn accepts_stale_when_tolerance_disabled() {
    let secret = "whsec_test";
    let ts = 1_700_000_000;
    let sig = sign(secret, ts, b"{}");
    let header = format!("ts={ts};sig={sig}");
    let opts = VerifyOptions {
        tolerance: Duration::ZERO,
        now: Some(ts + 99_999),
    };
    assert!(verify(b"{}", Some(&header), secret, &opts));
}

#[test]
fn rejects_malformed_headers() {
    for header in ["", "garbage", "ts=abc;sig=def", "ts=1;sig="] {
        assert!(
            !verify(b"{}", Some(header), "whsec_test", &VerifyOptions::default()),
            "expected rejection for header {header:?}"
        );
    }
    assert!(!verify(
        b"{}",
        None,
        "whsec_test",
        &VerifyOptions::default()
    ));
}
