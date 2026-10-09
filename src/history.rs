//! Whitelisted native metadata and bounded immutable history/discovery snapshots.
use crate::protocol::{ErrorCode, MAX_FRAME_BYTES, bounded, session_reference};
use serde_json::{Value, json};
use std::collections::VecDeque;

pub(crate) fn timestamp(value: &Value) -> Result<i64, ErrorCode> {
    value
        .as_i64()
        .or_else(|| {
            value
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp_millis())
        })
        .filter(|v| *v >= 0)
        .ok_or(ErrorCode::IncompatibleVersion)
}

pub fn session_summary(row: &Value) -> Result<Value, ErrorCode> {
    let id = session_reference(row["id"].as_str().ok_or(ErrorCode::IncompatibleVersion)?)?;
    let created = timestamp(&row["created_at"])?;
    let updated = timestamp(
        row.get("updated_at")
            .filter(|v| !v.is_null())
            .or_else(|| row.get("last_event_at").filter(|v| !v.is_null()))
            .unwrap_or(&row["created_at"]),
    )?;
    let status = match row["worker_status"]
        .as_str()
        .or(row["session_status"].as_str())
        .unwrap_or("idle")
    {
        "running" | "requires_action" => "running",
        "idle" | "completed" => "idle",
        "failed" | "error" => "failed",
        "stopped" | "cancelled" | "canceled" => "stopped",
        _ => return Err(ErrorCode::IncompatibleVersion),
    };
    let sources = row["config"]["sources"]
        .as_array()
        .or(row["session_context"]["sources"].as_array());
    let repository = sources
        .and_then(|rows| rows.iter().find(|r| r["type"] == "git_repository"))
        .and_then(|r| r["url"].as_str())
        .and_then(|raw| url::Url::parse(raw).ok())
        .and_then(|mut url| {
            if !matches!(url.scheme(), "https" | "http" | "ssh") {
                return None;
            }
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            Some(url.to_string())
        });
    Ok(
        json!({"id":id,"title":row["title"].as_str().unwrap_or("Untitled").chars().filter(|c| !c.is_control()).take(256).collect::<String>(),
        "repositoryUrl":repository,"createdAtMs":created,"updatedAtMs":updated,"archived":row["status"]=="archived" || row["archived"]==true,"status":status}),
    )
}

// Explicit SDK envelope whitelist: authentication, device metadata and private thinking never leave the broker.
pub fn canonical_event(row: &Value) -> Result<Option<Value>, ErrorCode> {
    let payload = &row["payload"];
    let kind = payload["type"].as_str().unwrap_or("");
    if !matches!(kind, "user" | "assistant") {
        return Ok(None);
    }
    if payload["message"]["role"] != kind {
        return Err(ErrorCode::IncompatibleVersion);
    }
    let id = row["id"]
        .as_str()
        .or(row["event_id"].as_str())
        .or(payload["uuid"].as_str())
        .filter(|s| bounded(s, 256))
        .ok_or(ErrorCode::IncompatibleVersion)?;
    let content = &payload["message"]["content"];
    let content = if content.is_string() {
        if kind == "assistant" {
            json!([{"type":"text","text":content}])
        } else {
            content.clone()
        }
    } else if let Some(blocks) = content.as_array() {
        let mut visible = Vec::new();
        for block in blocks {
            let fields: &[&str] = match block["type"].as_str() {
                Some("thinking" | "redacted_thinking") => continue,
                Some("text") => &["type", "text"],
                Some("tool_use") => &["type", "id", "name", "input"],
                Some("tool_result") => &["type", "tool_use_id", "content", "is_error"],
                Some("image" | "document") => &["type", "source", "title", "context", "citations"],
                _ => return Err(ErrorCode::IncompatibleVersion),
            };
            let mut clean = serde_json::Map::new();
            for field in fields {
                if let Some(value) = block.get(*field) {
                    clean.insert((*field).into(), value.clone());
                }
            }
            visible.push(Value::Object(clean));
        }
        Value::Array(visible)
    } else {
        return Err(ErrorCode::IncompatibleVersion);
    };
    let mut clean = json!({"type":kind,"message":{"role":kind,"content":content}});
    for key in ["uuid", "parent_tool_use_id", "isMeta", "isSidechain"] {
        if let Some(value) = payload.get(key) {
            clean[key] = value.clone();
        }
    }
    Ok(Some(
        json!({"id":id,"occurredAtMs":timestamp(&row["created_at"])? ,"payload":clean}),
    ))
}

pub struct HistorySnapshot {
    pub events: Vec<Value>,
    pub active: bool,
    pub through_sequence: u64,
}
struct Snapshot {
    key: String,
    owner: String,
    rows: Vec<Value>,
    active: bool,
    through: u64,
    bytes: usize,
}
#[derive(Default)]
struct Pages {
    snapshots: VecDeque<Snapshot>,
    bytes: usize,
}
impl Pages {
    fn start(
        &mut self,
        owner: &str,
        rows: Vec<Value>,
        active: bool,
        through: u64,
        limit: usize,
        kind: &str,
    ) -> Result<Value, ErrorCode> {
        let bytes = serde_json::to_vec(&rows)
            .map_err(|_| ErrorCode::IncompatibleVersion)?
            .len();
        if rows.len() > 100_000 || bytes > 16 * 1024 * 1024 {
            return Err(ErrorCode::TooLarge);
        }
        let key = uuid::Uuid::new_v4().simple().to_string();
        let snapshot = Snapshot {
            key,
            owner: owner.into(),
            rows,
            active,
            through,
            bytes,
        };
        let page = Self::render(&snapshot, 0, limit, kind)?;
        while self.snapshots.len() >= 8 || self.bytes + bytes > 64 * 1024 * 1024 {
            if let Some(old) = self.snapshots.pop_front() {
                self.bytes -= old.bytes;
            } else {
                break;
            }
        }
        self.bytes += bytes;
        self.snapshots.push_back(snapshot);
        Ok(page)
    }
    fn page(
        &self,
        owner: &str,
        cursor: &str,
        limit: usize,
        kind: &str,
    ) -> Result<Value, ErrorCode> {
        let (key, offset) = cursor.split_once('.').ok_or(ErrorCode::InvalidRequest)?;
        if key.len() != 32 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ErrorCode::InvalidRequest);
        }
        let offset: usize = offset.parse().map_err(|_| ErrorCode::InvalidRequest)?;
        let snapshot = self
            .snapshots
            .iter()
            .find(|s| s.key == key)
            .ok_or(ErrorCode::SessionUnavailable)?;
        if snapshot.owner != owner {
            return Err(ErrorCode::PermissionDenied);
        }
        if offset >= snapshot.rows.len() {
            return Err(ErrorCode::InvalidRequest);
        }
        Self::render(snapshot, offset, limit, kind)
    }
    fn render(
        snapshot: &Snapshot,
        offset: usize,
        limit: usize,
        kind: &str,
    ) -> Result<Value, ErrorCode> {
        if !(1..=100).contains(&limit) {
            return Err(ErrorCode::InvalidRequest);
        }
        let mut page = json!({kind:[],"nextCursor":null});
        if kind == "events" {
            page["active"] = json!(snapshot.active);
            page["throughSequence"] = json!(snapshot.through);
        }
        let end = snapshot.rows.len().min(offset + limit);
        for index in offset..end {
            page[kind]
                .as_array_mut()
                .ok_or(ErrorCode::IncompatibleVersion)?
                .push(snapshot.rows[index].clone());
            page["nextCursor"] = if index + 1 < snapshot.rows.len() {
                json!(format!("{}.{}", snapshot.key, index + 1))
            } else {
                Value::Null
            };
            let size = serde_json::to_vec(
                &json!({"version":1,"id":u64::MAX,"type":"response","result":page}),
            )
            .map_err(|_| ErrorCode::IncompatibleVersion)?
            .len()
                + 1;
            if size > MAX_FRAME_BYTES {
                page[kind]
                    .as_array_mut()
                    .ok_or(ErrorCode::IncompatibleVersion)?
                    .pop();
                if index == offset {
                    return Err(ErrorCode::TooLarge);
                }
                page["nextCursor"] = json!(format!("{}.{}", snapshot.key, index));
                break;
            }
        }
        Ok(page)
    }
    fn remove(&mut self, owner: &str) {
        self.snapshots.retain(|s| s.owner != owner);
        self.bytes = self.snapshots.iter().map(|s| s.bytes).sum();
    }
}
#[derive(Default)]
pub struct HistoryPages(Pages);
impl HistoryPages {
    pub fn start(
        &mut self,
        session: &str,
        snapshot: HistorySnapshot,
        limit: usize,
    ) -> Result<Value, ErrorCode> {
        self.0.start(
            session,
            snapshot.events,
            snapshot.active,
            snapshot.through_sequence,
            limit,
            "events",
        )
    }
    pub fn page(&self, session: &str, cursor: &str, limit: usize) -> Result<Value, ErrorCode> {
        self.0.page(session, cursor, limit, "events")
    }
    pub fn close(&mut self, session: &str) {
        self.0.remove(session);
    }
}
#[derive(Default)]
pub struct SessionPages(Pages);
impl SessionPages {
    pub fn start(&mut self, rows: Vec<Value>, limit: usize) -> Result<Value, ErrorCode> {
        self.0.start("", rows, false, 0, limit, "sessions")
    }
    pub fn page(&self, cursor: &str, limit: usize) -> Result<Value, ErrorCode> {
        self.0.page("", cursor, limit, "sessions")
    }
}
