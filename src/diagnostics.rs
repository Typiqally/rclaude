//! Counts and static schema labels only. Never serialize native record values.
use serde_json::{Value, json};

pub fn collection_schema(value: &Value) -> Value {
    let data = &value["data"];
    let kind = match data {
        Value::Null => "null",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::String(_) => "string",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
    };
    let known = [
        "idle",
        "running",
        "requires_action",
        "completed",
        "failed",
        "error",
        "stopped",
        "cancelled",
        "canceled",
        "archived",
        "pending",
        "created",
        "ready",
        "starting",
        "initializing",
        "waiting",
        "waiting_for_worker",
        "paused",
        "queued",
        "not_started",
        "done",
        "offline",
        "inactive",
        "unreachable",
        "disconnected",
        "not_running",
        "suspended",
    ];
    let mut states = serde_json::Map::new();
    let mut missing_created = 0;
    let mut invalid_created = 0;
    let mut invalid_ids = 0;
    let mut errors = 0;
    let mut unknown_states = 0;
    let mut absent_states = 0;
    for row in data.as_array().into_iter().flatten() {
        if row["created_at"].is_null() {
            missing_created += 1;
        } else if crate::history::timestamp(&row["created_at"]).is_err() {
            invalid_created += 1;
        }
        if row["id"]
            .as_str()
            .is_none_or(|id| crate::protocol::session_reference(id).is_err())
        {
            invalid_ids += 1;
        }
        if crate::history::session_summary(row).is_err() {
            errors += 1;
        }
        match row["worker_status"]
            .as_str()
            .or(row["session_status"].as_str())
        {
            Some(status) => {
                if let Some(label) = known.iter().find(|label| **label == status) {
                    let count = states.get(*label).and_then(Value::as_u64).unwrap_or(0) + 1;
                    states.insert((*label).into(), json!(count));
                } else {
                    unknown_states += 1;
                }
            }
            None => absent_states += 1,
        }
    }
    json!({"dataType":kind,"rowCount":data.as_array().map_or(0,Vec::len),
        "responseShapePresent":value.get("response_shape").is_some(),"sessionsArray":value["sessions"].is_array(),
        "pageCursorPresent":!value["next_cursor"].is_null(),"hasMore":value["has_more"]==true,
        "missingCreatedAt":missing_created,"invalidCreatedAt":invalid_created,"invalidSessionIds":invalid_ids,
        "summaryErrors":errors,"knownWorkerStates":states,"unknownWorkerStates":unknown_states,"absentWorkerStates":absent_states})
}
