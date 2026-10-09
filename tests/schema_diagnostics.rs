use rclaude::diagnostics::collection_schema;
use serde_json::json;

#[test]
fn native_schema_diagnostics_identify_bad_fields_without_exporting_their_values() {
    let report = collection_schema(&json!({"accessToken":"secret-token", "data":[
        {"id":"cse_example","title":"private conversation","created_at":"2026-10-09T10:00:00Z","worker_status":"idle"},
        {"id":"cse_second","title":"another private title","created_at":null,"worker_status":"offline","refreshToken":"secret-refresh"}
    ],"next_cursor":"private-provider-cursor"}));
    assert_eq!(report["dataType"], "array");
    assert_eq!(report["rowCount"], 2);
    assert_eq!(report["missingCreatedAt"], 1);
    assert_eq!(report["knownWorkerStates"]["offline"], 1);
    assert_eq!(report["summaryErrors"], 1);
    let encoded = report.to_string();
    for value in ["private", "secret", "cse_example", "cse_second"] {
        assert!(!encoded.contains(value));
    }
}

#[test]
fn unknown_native_states_and_collection_keys_never_become_diagnostic_values() {
    let report = collection_schema(&json!({"unknown-secret-key":"secret-value","data":[
        {"id":"cse_example","created_at":"2026-10-09T10:00:00Z","worker_status":"private-unknown-state"}
    ]}));
    assert_eq!(report["unknownWorkerStates"], 1);
    assert!(!report.to_string().contains("private"));
    assert_eq!(
        collection_schema(&json!({"data":{"secret":"value"}}))["dataType"],
        "object"
    );
}

#[test]
fn diagnostics_separate_archived_worker_states_without_disclosing_them() {
    let report = collection_schema(&json!({"data":[
        {"id":"cse_a","created_at":"2026-10-09T10:00:00Z","status":"archived","worker_status":"private-retired-state"},
        {"id":"cse_b","created_at":"2026-10-09T10:00:00Z","archived":true,"worker_status":"another-private-state"}
    ]}));
    assert_eq!(report["unknownArchivedStates"], 2);
    assert!(!report.to_string().contains("private"));
}
