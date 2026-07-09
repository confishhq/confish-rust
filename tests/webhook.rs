use confish::webhook::{verify, VerifyOptions, WebhookError, DEFAULT_TOLERANCE};
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
fn returns_parsed_payload_for_valid_signature() {
    let body = br#"{
        "event": "environment.updated",
        "timestamp": "2026-07-09T12:00:00+00:00",
        "application": {"name": "My App"},
        "environment": {"name": "production", "env_id": "env_test", "url": "https://confi.sh/c/env_test"},
        "changes": ["maintenance_mode"],
        "values": {"maintenance_mode": true}
    }"#;
    let ts = 1_700_000_000;
    let sig = sign("whsec_test", ts, body);
    let header = format!("ts={ts};sig={sig}");
    let opts = VerifyOptions {
        tolerance: DEFAULT_TOLERANCE,
        now: Some(ts),
    };

    let payload = verify(body, Some(&header), "whsec_test", &opts).expect("verify");
    assert_eq!(payload.event, "environment.updated");
    assert_eq!(
        payload.timestamp.as_deref(),
        Some("2026-07-09T12:00:00+00:00")
    );
    assert_eq!(payload.application.unwrap().name, "My App");
    let environment = payload.environment.unwrap();
    assert_eq!(environment.name, "production");
    assert_eq!(environment.env_id, "env_test");
    assert_eq!(
        payload.changes,
        Some(serde_json::json!(["maintenance_mode"]))
    );
    assert_eq!(
        payload.values,
        Some(serde_json::json!({"maintenance_mode": true}))
    );
}

#[test]
fn rejects_wrong_secret_as_invalid_signature() {
    let body = br#"{"event":"environment.updated"}"#;
    let ts = 1_700_000_000;
    let sig = sign("other", ts, body);
    let header = format!("ts={ts};sig={sig}");
    let opts = VerifyOptions {
        tolerance: DEFAULT_TOLERANCE,
        now: Some(ts),
    };
    let err = verify(body, Some(&header), "whsec_test", &opts).unwrap_err();
    assert!(matches!(err, WebhookError::InvalidSignature));
}

#[test]
fn rejects_tampered_body_as_invalid_signature() {
    let secret = "whsec_test";
    let ts = 1_700_000_000;
    let sig = sign(secret, ts, br#"{"event":"a"}"#);
    let header = format!("ts={ts};sig={sig}");
    let opts = VerifyOptions {
        tolerance: DEFAULT_TOLERANCE,
        now: Some(ts),
    };
    let err = verify(br#"{"event":"b"}"#, Some(&header), secret, &opts).unwrap_err();
    assert!(matches!(err, WebhookError::InvalidSignature));
}

#[test]
fn rejects_stale_timestamp_with_drift_details() {
    let secret = "whsec_test";
    let ts = 1_700_000_000;
    let sig = sign(secret, ts, br#"{"event":"environment.updated"}"#);
    let header = format!("ts={ts};sig={sig}");
    let opts = VerifyOptions {
        tolerance: Duration::from_secs(300),
        now: Some(ts + 600),
    };
    let err = verify(
        br#"{"event":"environment.updated"}"#,
        Some(&header),
        secret,
        &opts,
    )
    .unwrap_err();
    match err {
        WebhookError::TimestampOutsideTolerance {
            drift_secs,
            tolerance_secs,
        } => {
            assert_eq!(drift_secs, 600);
            assert_eq!(tolerance_secs, 300);
        }
        other => panic!("expected TimestampOutsideTolerance, got {other:?}"),
    }
}

#[test]
fn accepts_stale_when_tolerance_disabled() {
    let secret = "whsec_test";
    let ts = 1_700_000_000;
    let body = br#"{"event":"environment.deleted"}"#;
    let sig = sign(secret, ts, body);
    let header = format!("ts={ts};sig={sig}");
    let opts = VerifyOptions {
        tolerance: Duration::ZERO,
        now: Some(ts + 99_999),
    };
    let payload = verify(body, Some(&header), secret, &opts).expect("verify");
    assert_eq!(payload.event, "environment.deleted");
}

#[test]
fn rejects_malformed_headers() {
    for header in ["garbage", "ts=abc;sig=def", "ts=1;sig="] {
        let err = verify(
            br#"{"event":"a"}"#,
            Some(header),
            "whsec_test",
            &VerifyOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(err, WebhookError::MalformedSignature),
            "expected MalformedSignature for header {header:?}, got {err:?}"
        );
    }
}

#[test]
fn rejects_missing_or_empty_signature() {
    for signature in [None, Some("")] {
        let err = verify(
            br#"{"event":"a"}"#,
            signature,
            "whsec_test",
            &VerifyOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(err, WebhookError::MissingSignature),
            "expected MissingSignature for {signature:?}, got {err:?}"
        );
    }
}

#[test]
fn rejects_unparseable_payload_after_valid_signature() {
    let secret = "whsec_test";
    let ts = 1_700_000_000;
    let body = b"not json";
    let sig = sign(secret, ts, body);
    let header = format!("ts={ts};sig={sig}");
    let opts = VerifyOptions {
        tolerance: DEFAULT_TOLERANCE,
        now: Some(ts),
    };
    let err = verify(body, Some(&header), secret, &opts).unwrap_err();
    assert!(matches!(err, WebhookError::InvalidPayload(_)));
}
