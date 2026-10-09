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

#[test]
fn broker_advertises_native_cloud_history_without_reading_credentials() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_rclaude"))
        .arg("broker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"{\"version\":1,\"id\":1,\"type\":\"handshake\",\"minimumVersion\":1,\"maximumVersion\":1}\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for name in ["sessionDiscovery", "history", "transcript", "interrupt"] {
        assert_eq!(response["result"]["capabilities"][name], true, "{name}");
    }
    assert!(
        response["result"]["maximumFrameBytes"]
            .as_u64()
            .is_some_and(|size| size > 0 && size <= 65536)
    );
}
