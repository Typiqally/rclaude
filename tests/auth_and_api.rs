use rclaude::auth::OAuthToken;
use rclaude::client::classify_status;
use rclaude::protocol::ErrorCode;

#[test]
fn loads_only_official_oauth_and_keeps_debug_redacted() {
    let token = OAuthToken::from_json(br#"{"claudeAiOauth":{"accessToken":"test-access-private","refreshToken":"test-refresh-private","expiresAt":4102444800000,"scopes":["user:inference"]}}"#).unwrap();
    assert!(!format!("{token:?}").contains("test-access-private"));
    assert!(!format!("{token:?}").contains("test-refresh-private"));
    assert!(OAuthToken::from_json(br#"{"apiKey":"test-api-key"}"#).is_err());
    assert!(OAuthToken::from_json(b"bad JSON").is_err());
}

#[test]
fn classifies_upstream_failures_without_forwarding_error_details() {
    assert_eq!(
        classify_status(401, None),
        ErrorCode::AuthenticationRequired
    );
    assert_eq!(
        classify_status(403, Some("untrusted_device")),
        ErrorCode::EnrollmentRequired
    );
    assert_eq!(classify_status(403, None), ErrorCode::PermissionDenied);
    assert_eq!(classify_status(429, None), ErrorCode::RateLimited);
    assert_eq!(classify_status(404, None), ErrorCode::SessionUnavailable);
    assert_eq!(
        classify_status(500, Some("private upstream details")),
        ErrorCode::Unavailable
    );
}
