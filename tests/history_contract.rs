use rclaude::history::{
    HistoryPages, HistorySnapshot, SessionPages, canonical_event, session_summary,
};
use rclaude::protocol::{ErrorCode, MAX_FRAME_BYTES, decode_request};
use serde_json::{Value, json};

fn row(id: &str, sequence: u64, source: &str, kind: &str, content: Value) -> Value {
    json!({"id":id,"sequence_num":sequence,"source":source,"created_at":"2026-10-09T10:00:00Z","payload":{"type":kind,"uuid":id,"message":{"role":kind,"content":content}},"accessToken":"must-not-cross"})
}
#[test]
fn accepts_version_one_paged_discovery_and_history_requests() {
    for request in [
        json!({"version":1,"id":1,"type":"list_sessions","cursor":null,"limit":100}),
        json!({"version":1,"id":2,"type":"read_history","sessionId":"cse_original","cursor":null,"limit":100}),
    ] {
        assert!(decode_request(&serde_json::to_vec(&request).unwrap()).is_ok());
    }
}
#[test]
fn metadata_covers_clients_without_exporting_credentials_or_url_secrets() {
    for kind in ["bridge", "cloud", "desktop", "web"] {
        let summary = session_summary(&json!({"id":"cse_original","environment_kind":kind,"title":"Original","created_at":"2026-10-09T10:00:00Z","last_event_at":"2026-10-09T11:00:00Z","worker_status":"requires_action","config":{"sources":[{"type":"git_repository","url":"https://private:credential@github.com/team/repo.git?token=secret#private"}]},"accessToken":"secret"})).unwrap();
        assert_eq!(summary["repositoryUrl"], "https://github.com/team/repo.git");
        assert_eq!(summary["archived"], false);
        assert_eq!(summary["status"], "running");
        assert!(
            summary["updatedAtMs"].as_i64().unwrap() > summary["createdAtMs"].as_i64().unwrap()
        );
        assert!(!summary.to_string().contains("secret"));
    }
}
#[test]
fn canonical_history_keeps_every_clients_messages_and_drops_auth_and_private_thinking() {
    let user = canonical_event(&row("event-user", 3, "desktop", "user", json!("question")))
        .unwrap()
        .unwrap();
    let assistant = canonical_event(&row(
        "event-answer",
        7,
        "cli",
        "assistant",
        json!([{"type":"thinking","thinking":"private reasoning"},{"type":"text","text":"answer"}]),
    ))
    .unwrap()
    .unwrap();
    assert_eq!(user["id"], "event-user");
    assert_eq!(user["payload"]["message"]["content"], "question");
    assert_eq!(
        assistant["payload"]["message"]["content"],
        json!([{"type":"text","text":"answer"}])
    );
    assert!(!user.to_string().contains("must-not-cross"));
    assert!(!assistant.to_string().contains("private reasoning"));
    assert!(
        canonical_event(&row("system", 8, "worker", "system", json!("internal")))
            .unwrap()
            .is_none()
    );
}
#[test]
fn cursors_pin_one_immutable_snapshot_and_refuse_cross_session_pages() {
    let mut pages = HistoryPages::default();
    let events = vec![
        canonical_event(&row("u", 3, "web", "user", json!("question")))
            .unwrap()
            .unwrap(),
        canonical_event(&row(
            "a",
            7,
            "worker",
            "assistant",
            json!([{"type":"text","text":"answer"}]),
        ))
        .unwrap()
        .unwrap(),
    ];
    let first = pages
        .start(
            "cse_original",
            HistorySnapshot {
                events,
                active: false,
                through_sequence: 9,
            },
            1,
        )
        .unwrap();
    let cursor = first["nextCursor"].as_str().unwrap();
    pages
        .start(
            "cse_original",
            HistorySnapshot {
                events: vec![],
                active: true,
                through_sequence: 12,
            },
            1,
        )
        .unwrap();
    let second = pages.page("cse_original", cursor, 1).unwrap();
    assert_eq!(second["throughSequence"], 9);
    assert_eq!(second["active"], false);
    assert_eq!(second["events"][0]["id"], "a");
    assert!(second["nextCursor"].is_null());
    assert_eq!(
        pages.page("cse_other", cursor, 1).unwrap_err(),
        ErrorCode::PermissionDenied
    );
    assert!(pages.page("cse_original", "../credentials", 1).is_err());
}
#[test]
fn pages_fit_the_frame_budget_without_silently_truncating_large_events() {
    let mut pages = HistoryPages::default();
    let events = (0..4)
        .map(|index| {
            canonical_event(&row(
                &format!("event{index}"),
                index + 1,
                "worker",
                "assistant",
                json!([{"type":"text","text":"x".repeat(24*1024)}]),
            ))
            .unwrap()
            .unwrap()
        })
        .collect();
    let first = pages
        .start(
            "cse_original",
            HistorySnapshot {
                events,
                active: false,
                through_sequence: 4,
            },
            100,
        )
        .unwrap();
    assert!(
        serde_json::to_vec(&json!({"version":1,"id":u64::MAX,"type":"response","result":first}))
            .unwrap()
            .len()
            < MAX_FRAME_BYTES
    );
    assert!(first["nextCursor"].is_string());
    let giant = canonical_event(&row(
        "giant",
        1,
        "worker",
        "assistant",
        json!([{"type":"text","text":"x".repeat(70*1024)}]),
    ))
    .unwrap()
    .unwrap();
    assert_eq!(
        pages
            .start(
                "cse_original",
                HistorySnapshot {
                    events: vec![giant],
                    active: false,
                    through_sequence: 1
                },
                100
            )
            .unwrap_err(),
        ErrorCode::TooLarge
    );
    let mut sessions = SessionPages::default();
    let first = sessions
        .start(vec![json!({"id":"cse_a"}), json!({"id":"cse_b"})], 1)
        .unwrap();
    let second = sessions
        .page(first["nextCursor"].as_str().unwrap(), 1)
        .unwrap();
    assert_eq!(second["sessions"][0]["id"], "cse_b");
}
