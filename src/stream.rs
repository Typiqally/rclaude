//! Bounded incremental SSE decoder, accepting execution results from workers only.
use crate::protocol::ErrorCode;
use serde_json::{Value, json};
#[derive(Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
}
impl SseDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Value>, ErrorCode> {
        if self.buffer.len() + bytes.len() >= 128 * 1024 {
            return Err(ErrorCode::TooLarge);
        }
        self.buffer.extend_from_slice(bytes);
        let mut result = Vec::new();
        loop {
            let boundary = self
                .buffer
                .windows(2)
                .position(|b| b == b"\n\n")
                .map(|p| (p, 2))
                .or_else(|| {
                    self.buffer
                        .windows(4)
                        .position(|b| b == b"\r\n\r\n")
                        .map(|p| (p, 4))
                });
            let Some((end, separator)) = boundary else {
                break;
            };
            let frame = String::from_utf8(self.buffer.drain(..end + separator).collect())
                .map_err(|_| ErrorCode::InvalidRequest)?;
            let mut event = "";
            let mut sequence = 0;
            let mut data = String::new();
            for line in frame.lines() {
                if let Some(value) = line.strip_prefix("event:") {
                    event = value.trim();
                }
                if let Some(value) = line.strip_prefix("id:") {
                    sequence = value.trim().parse().unwrap_or(0);
                }
                if let Some(value) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(value.trim_start());
                }
            }
            if data.is_empty() {
                continue;
            }
            let value: Value =
                serde_json::from_str(&data).map_err(|_| ErrorCode::IncompatibleVersion)?;
            if event == "catch_up_truncated" {
                return Err(ErrorCode::HistoryIncomplete);
            }
            if event == "security_update" && value["security_tier"] == "elevated" {
                return Err(ErrorCode::EnrollmentRequired);
            }
            let payload = &value["payload"];
            let kind = payload["type"].as_str().unwrap_or("");
            let accepted = match event {
                "client_event" => {
                    (kind == "user"
                        && value["source"] != "worker"
                        && payload["message"]["role"] == "user"
                        && (payload["message"]["content"].is_string()
                            || payload["message"]["content"].is_array())
                        && payload["uuid"].as_str().is_some())
                        || (value["source"] == "worker"
                            && matches!(
                                kind,
                                "assistant"
                                    | "user"
                                    | "result"
                                    | "system"
                                    | "stream_event"
                                    | "command_lifecycle"
                                    | "control_request"
                                    | "control_response"
                            ))
                }
                "ephemeral_event" => kind == "stream_event",
                _ => false,
            };
            if accepted {
                if sequence == 0 {
                    sequence = value["sequence_num"]
                        .as_u64()
                        .or_else(|| value["sequence_num"].as_str().and_then(|s| s.parse().ok()))
                        .unwrap_or(0);
                }
                result.push(json!({"sequence":sequence,"payload":safe_payload(payload)?}));
            }
        }
        Ok(result)
    }
}

/// Filter duplicate replay without inventing sequence numbers for ephemeral deltas.
pub struct SequenceGate {
    last: u64,
}
impl SequenceGate {
    pub fn new(after: u64) -> Self {
        Self { last: after }
    }
    pub fn accept(&mut self, event: &Value) -> Result<Option<u64>, ErrorCode> {
        let sequence = event["sequence"]
            .as_u64()
            .ok_or(ErrorCode::IncompatibleVersion)?;
        if sequence == 0 {
            return if event["payload"]["type"] == "stream_event" {
                Ok(None)
            } else {
                Err(ErrorCode::HistoryIncomplete)
            };
        }
        if sequence <= self.last {
            return Ok(None);
        }
        self.last = sequence;
        Ok(Some(sequence))
    }
}

fn safe_payload(payload: &Value) -> Result<Value, ErrorCode> {
    let kind = payload["type"]
        .as_str()
        .ok_or(ErrorCode::IncompatibleVersion)?;
    if matches!(kind, "user" | "assistant") {
        let row = json!({"id":"stream","created_at":0,"payload":payload});
        return crate::history::canonical_event(&row)?
            .map(|v| v["payload"].clone())
            .ok_or(ErrorCode::IncompatibleVersion);
    }
    let fields: &[&str] = match kind {
        "result" => &[
            "type",
            "subtype",
            "is_error",
            "result",
            "uuid",
            "session_id",
            "duration_ms",
            "num_turns",
        ],
        "system" => &["type", "subtype", "uuid", "session_id"],
        "command_lifecycle" => &["type", "command_uuid", "state", "uuid", "session_id"],
        "stream_event" => &["type", "event", "uuid", "session_id", "parent_tool_use_id"],
        "control_request" => &["type", "request_id", "request"],
        "control_response" => &["type", "response"],
        _ => return Err(ErrorCode::IncompatibleVersion),
    };
    let mut clean = serde_json::Map::new();
    for field in fields {
        if let Some(value) = payload.get(*field) {
            clean.insert((*field).into(), value.clone());
        }
    }
    Ok(Value::Object(clean))
}
