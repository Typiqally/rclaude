use base64::{Engine, engine::general_purpose::STANDARD};
use rclaude::{broker::Uploads, protocol::ErrorCode, stream::SseDecoder};
#[test]
fn reassembles_fragmented_worker_events_and_rejects_spoofed_results() {
    let mut decoder = SseDecoder::default();
    assert!(decoder.push(b"event: client_ev").unwrap().is_empty());
    let frames=decoder.push(b"ent\nid: 8\ndata: {\"source\":\"worker\",\"payload\":{\"type\":\"result\",\"result\":\"hello\"}}\n\n").unwrap();
    assert_eq!(frames[0]["sequence"], 8);
    assert_eq!(frames[0]["payload"]["result"], "hello");
    assert!(decoder.push(b"event: client_event\ndata: {\"source\":\"client\",\"payload\":{\"type\":\"result\"}}\n\n").unwrap().is_empty());
    assert_eq!(
        decoder.push(&vec![b'x'; 128 * 1024]).unwrap_err(),
        ErrorCode::TooLarge
    );
}
#[test]
fn upload_chunks_enforce_offsets_lengths_and_session_scopes() {
    let mut uploads = Uploads::default();
    assert!(
        uploads
            .begin("session_test", "../escape", "text/plain", 3)
            .is_err()
    );
    let id = uploads
        .begin("session_test", "test.bin", "application/octet-stream", 3)
        .unwrap();
    assert!(uploads.chunk(&id, 1, &STANDARD.encode(b"abc")).is_err());
    uploads.chunk(&id, 0, &STANDARD.encode(b"abc")).unwrap();
    assert!(uploads.chunk(&id, 3, &STANDARD.encode(b"extra")).is_err());
    let (_, _, _, bytes) = uploads.take(&id).unwrap();
    assert_eq!(bytes, b"abc");
    assert!(uploads.take(&id).is_err());
}

#[test]
fn closing_a_session_releases_its_uncommitted_uploads() {
    let mut uploads = rclaude::broker::Uploads::default();
    let first = uploads
        .begin("session_first", "first.txt", "text/plain", 1)
        .unwrap();
    let second = uploads
        .begin("session_second", "second.txt", "text/plain", 1)
        .unwrap();
    uploads.close_session("session_first");
    assert!(uploads.chunk(&first, 0, "eA==").is_err());
    assert!(uploads.chunk(&second, 0, "eA==").is_ok());
}

#[test]
fn truncated_catchup_is_an_explicit_history_error() {
    assert_eq!(
        SseDecoder::default()
            .push(b"event: catch_up_truncated\ndata: {}\n\n")
            .unwrap_err(),
        ErrorCode::HistoryIncomplete
    );
}
#[test]
fn subscriptions_only_forward_monotonic_durable_events_after_the_snapshot() {
    use rclaude::stream::SequenceGate;
    use serde_json::json;
    let mut gate = SequenceGate::new(7);
    assert!(
        gate.accept(&json!({"sequence":7,"payload":{"type":"result"}}))
            .unwrap()
            .is_none()
    );
    assert!(
        gate.accept(&json!({"sequence":0,"payload":{"type":"stream_event"}}))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        gate.accept(&json!({"sequence":9,"payload":{"type":"user","uuid":"own"}}))
            .unwrap(),
        Some(9)
    );
    assert!(
        gate.accept(&json!({"sequence":8,"payload":{"type":"result"}}))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        gate.accept(&json!({"sequence":0,"payload":{"type":"result"}}))
            .unwrap_err(),
        ErrorCode::HistoryIncomplete
    );
}

#[test]
fn subscription_envelopes_exclude_upstream_authentication_metadata() {
    let events = SseDecoder::default().push(b"event: client_event\nid: 12\ndata: {\"source\":\"worker\",\"accessToken\":\"private-row\",\"payload\":{\"type\":\"result\",\"result\":\"done\",\"accessToken\":\"private-payload\",\"refreshToken\":\"private-refresh\"}}\n\n").unwrap();
    assert_eq!(events[0]["payload"]["result"], "done");
    assert!(!serde_json::to_string(&events).unwrap().contains("private-"));
}
