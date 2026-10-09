use rclaude::protocol::{
    BrokerRequest, MAX_FRAME_BYTES, decode_request, session_reference, turn_event,
};
use serde_json::json;

#[test]
fn accepts_only_exact_session_references_and_discards_url_metadata() {
    assert_eq!(
        session_reference("https://claude.ai/code/session_abc123?from=desktop").unwrap(),
        "session_abc123"
    );
    assert_eq!(session_reference("cse_abc123").unwrap(), "cse_abc123");
    for value in [
        "https://evil.test/code/session_abc",
        "https://claude.ai.evil.test/code/session_abc",
        "session_../secret",
        "session_a\n",
        "env_abc",
        "session_",
    ] {
        assert!(session_reference(value).is_err(), "{value:?}");
    }
}

#[test]
fn requires_bounded_exact_broker_requests_and_redacts_user_input() {
    let request = decode_request(br#"{"version":1,"id":1,"type":"send_turn","sessionId":"session_abc","prompt":"private prompt","attachmentIds":[]}"#).unwrap();
    assert!(matches!(request, BrokerRequest::SendTurn { .. }));
    assert!(!format!("{request:?}").contains("private prompt"));
    assert!(decode_request(br#"{"version":1,"id":1,"type":"handshake","minimumVersion":1,"maximumVersion":1,"token":"secret"}"#).is_err());
    assert!(decode_request(&vec![b'x'; MAX_FRAME_BYTES + 1]).is_err());
}

#[test]
fn sends_a_stable_uuid_wrapped_user_event_without_auth_material() {
    assert_eq!(
        turn_event("session_abc", "message-1", "Inspect the file"),
        json!({
            "events": [{"payload": {"uuid":"message-1", "session_id":"session_abc", "type":"user", "parent_tool_use_id":null,
                "message":{"role":"user","content":"Inspect the file"}}}]
        })
    );
}
