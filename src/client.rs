//! Independently implemented Claude Code controller API. No raw upstream errors leave this module.
use crate::{
    auth,
    protocol::{ErrorCode, MAX_FILE_BYTES, bounded, session_reference, turn_event},
};
use futures_util::{StreamExt, stream::BoxStream};
use serde_json::{Value, json};
use std::time::Duration;

pub fn classify_status(status: u16, code: Option<&str>) -> ErrorCode {
    match status {
        400 => ErrorCode::InvalidRequest,
        401 => ErrorCode::AuthenticationRequired,
        403 if matches!(
            code,
            Some("untrusted_device" | "bound_unattested" | "signature_wanted")
        ) =>
        {
            ErrorCode::EnrollmentRequired
        }
        403 => ErrorCode::PermissionDenied,
        404 => ErrorCode::SessionUnavailable,
        413 => ErrorCode::TooLarge,
        429 => ErrorCode::RateLimited,
        _ => ErrorCode::Unavailable,
    }
}
async fn bounded_json(response: reqwest::Response, maximum: usize) -> Result<Value, ErrorCode> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(ErrorCode::TooLarge);
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ErrorCode::Unavailable)?;
        if bytes.len() + chunk.len() > maximum {
            return Err(ErrorCode::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| ErrorCode::IncompatibleVersion)
}
fn collection_path(base: &str, limit: usize, sort: Option<&str>, cursor: Option<&str>) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("limit", &limit.to_string());
    if let Some(sort) = sort {
        query.append_pair("sort_order", sort);
    }
    if let Some(cursor) = cursor {
        query.append_pair("cursor", cursor);
    }
    format!("{base}?{}", query.finish())
}
fn sequence(row: &Value) -> Result<u64, ErrorCode> {
    row["sequence_num"]
        .as_u64()
        .or_else(|| row["sequence_num"].as_str().and_then(|s| s.parse().ok()))
        .ok_or(ErrorCode::IncompatibleVersion)
}
fn reject_truncated(value: &Value) -> Result<(), ErrorCode> {
    if value["truncated"] == true
        || value["catch_up_truncated"] == true
        || value["type"] == "catch_up_truncated"
    {
        Err(ErrorCode::HistoryIncomplete)
    } else {
        Ok(())
    }
}
fn next_cursor(value: &Value) -> Result<Option<String>, ErrorCode> {
    match value.get("next_cursor") {
        Some(Value::String(s)) if bounded(s, 4096) => Ok(Some(s.clone())),
        Some(Value::Null) | None if value["has_more"] != true => Ok(None),
        _ => Err(ErrorCode::HistoryIncomplete),
    }
}
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    endpoint: String,
    credentials: std::sync::Arc<tokio::sync::Mutex<Option<auth::OAuthToken>>>,
}
impl Client {
    #[cfg(test)]
    pub(crate) fn fixture(endpoint: String) -> Self {
        let token = auth::OAuthToken::from_json(
            br#"{"claudeAiOauth":{"accessToken":"fake-access","expiresAt":4102444800000}}"#,
        )
        .unwrap();
        Self {
            http: reqwest::Client::new(),
            credentials: std::sync::Arc::new(tokio::sync::Mutex::new(Some(token))),
            endpoint,
        }
    }
    pub fn new() -> Result<Self, ErrorCode> {
        Ok(Self {
            endpoint: "https://api.anthropic.com".into(),
            credentials: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .user_agent("rclaude/0.1.0")
                .build()
                .map_err(|_| ErrorCode::Unavailable)?,
        })
    }
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, ErrorCode> {
        let mut cached = self.credentials.lock().await;
        if cached.as_ref().is_none_or(|token| token.expired()) {
            *cached = Some(auth::load().await?);
        }
        let token = cached.as_ref().ok_or(ErrorCode::AuthenticationRequired)?;
        let request = self
            .http
            .request(method, format!("{}{path}", self.endpoint))
            .bearer_auth(token.access())
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "ccr-byoc-2025-07-29,oauth-2025-04-20")
            .header("anthropic-client-platform", "cli");
        Ok(if let Some(org) = auth::organization() {
            request.header("x-organization-uuid", org)
        } else {
            request
        })
    }
    async fn checked(&self, response: reqwest::Response) -> Result<reqwest::Response, ErrorCode> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        if status == 401 {
            *self.credentials.lock().await = None;
        }
        let data = bounded_json(response, 64 * 1024)
            .await
            .unwrap_or(Value::Null);
        Err(classify_status(
            status,
            data["error"]["code"]
                .as_str()
                .or(data["error"]["type"].as_str())
                .or(data["code"].as_str()),
        ))
    }
    async fn json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ErrorCode> {
        let mut request = self
            .request(method, path)
            .await?
            .timeout(Duration::from_secs(30));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = self
            .checked(request.send().await.map_err(|_| ErrorCode::Unavailable)?)
            .await?;
        bounded_json(response, 8 * 1024 * 1024).await
    }
    pub async fn sessions(&self) -> Result<Value, ErrorCode> {
        let mut rows = Vec::new();
        let mut cursor = None;
        let mut seen = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        let mut bytes = 0usize;
        loop {
            let path = collection_path("/v1/code/sessions", 100, None, cursor.as_deref());
            let result = self.json(reqwest::Method::GET, &path, None).await?;
            reject_truncated(&result)?;
            let data = result["data"]
                .as_array()
                .ok_or(ErrorCode::IncompatibleVersion)?;
            for row in data {
                let summary = crate::history::session_summary(row)?;
                if ids.insert(
                    summary["id"]
                        .as_str()
                        .ok_or(ErrorCode::IncompatibleVersion)?
                        .to_owned(),
                ) {
                    bytes += summary.to_string().len();
                    rows.push(summary);
                    if rows.len() > 10_000 || bytes > 16 * 1024 * 1024 {
                        return Err(ErrorCode::TooLarge);
                    }
                }
            }
            cursor = next_cursor(&result)?;
            let Some(next) = &cursor else {
                break;
            };
            if data.is_empty() || !seen.insert(next.clone()) {
                return Err(ErrorCode::HistoryIncomplete);
            }
        }
        Ok(Value::Array(rows))
    }
    pub async fn history_snapshot(
        &self,
        session: &str,
    ) -> Result<crate::history::HistorySnapshot, ErrorCode> {
        let session = session_reference(session)?;
        let value = self.session(&session).await?;
        let metadata = value
            .get("response_shape")
            .or_else(|| value.get("session"))
            .unwrap_or(&value);
        let active = crate::history::session_summary(metadata)?["status"] == "running";
        let base = format!("/v1/code/sessions/{session}/events");
        let latest = self
            .json(
                reqwest::Method::GET,
                &collection_path(&base, 1, Some("desc"), None),
                None,
            )
            .await?;
        reject_truncated(&latest)?;
        let newest = latest["data"]
            .as_array()
            .ok_or(ErrorCode::IncompatibleVersion)?;
        if newest.len() > 1 {
            return Err(ErrorCode::IncompatibleVersion);
        }
        let through = newest.first().map(sequence).transpose()?.unwrap_or(0);
        if through == 0 && !newest.is_empty() {
            return Err(ErrorCode::IncompatibleVersion);
        }
        let mut events = Vec::new();
        let mut cursor = None;
        let mut seen = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        let mut bytes = 0usize;
        let mut last = 0;
        let mut count = 0;
        while last < through {
            let page = self
                .json(
                    reqwest::Method::GET,
                    &collection_path(&base, 100, Some("asc"), cursor.as_deref()),
                    None,
                )
                .await?;
            reject_truncated(&page)?;
            let rows = page["data"]
                .as_array()
                .ok_or(ErrorCode::IncompatibleVersion)?;
            for row in rows {
                let seq = sequence(row)?;
                if seq > through {
                    break;
                }
                if seq <= last {
                    return Err(ErrorCode::HistoryIncomplete);
                }
                last = seq;
                count += 1;
                if count > 100_000 {
                    return Err(ErrorCode::TooLarge);
                }
                if let Some(event) = crate::history::canonical_event(row)? {
                    if !ids.insert(
                        event["id"]
                            .as_str()
                            .ok_or(ErrorCode::IncompatibleVersion)?
                            .to_owned(),
                    ) {
                        return Err(ErrorCode::HistoryIncomplete);
                    }
                    bytes += event.to_string().len();
                    if bytes > 16 * 1024 * 1024 {
                        return Err(ErrorCode::TooLarge);
                    }
                    events.push(event);
                }
            }
            if last == through {
                break;
            }
            cursor = next_cursor(&page)?;
            let Some(next) = &cursor else {
                return Err(ErrorCode::HistoryIncomplete);
            };
            if rows.is_empty() || !seen.insert(next.clone()) {
                return Err(ErrorCode::HistoryIncomplete);
            }
        }
        Ok(crate::history::HistorySnapshot {
            events,
            active,
            through_sequence: through,
        })
    }
    pub async fn session(&self, session: &str) -> Result<Value, ErrorCode> {
        let session = session_reference(session)?;
        self.json(
            reqwest::Method::GET,
            &format!("/v1/code/sessions/{session}"),
            None,
        )
        .await
    }
    pub async fn create(&self, parent: &str, title: &str) -> Result<String, ErrorCode> {
        if !bounded(title, 256) {
            return Err(ErrorCode::InvalidRequest);
        }
        let value = self.session(parent).await?;
        let session = value
            .get("response_shape")
            .or_else(|| value.get("session"))
            .unwrap_or(&value);
        let environment = session["environment_id"]
            .as_str()
            .ok_or(ErrorCode::Unsupported)?;
        let sources = session["config"]["sources"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let created=self.json(reqwest::Method::POST,"/v1/code/sessions",Some(json!({"title":title,"events":[],"environment_id":environment,"config":{"sources":sources}}))).await?;
        session_reference(
            created["response_shape"]["id"]
                .as_str()
                .or(created["session"]["id"].as_str())
                .or(created["id"].as_str())
                .ok_or(ErrorCode::IncompatibleVersion)?,
        )
    }
    pub async fn upload(
        &self,
        session: &str,
        name: &str,
        media_type: &str,
        bytes: Vec<u8>,
    ) -> Result<Value, ErrorCode> {
        let session = session_reference(session)?;
        if bytes.is_empty()
            || bytes.len() as u64 > MAX_FILE_BYTES
            || !bounded(name, 255)
            || name.contains(['/', '\\'])
            || name == "."
            || name == ".."
            || !bounded(media_type, 128)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        let length = bytes.len();
        let mount_path = format!(
            "/mnt/user-data/uploads/{}-{name}",
            uuid::Uuid::new_v4().simple()
        );
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(name.to_owned())
            .mime_str(media_type)
            .map_err(|_| ErrorCode::InvalidRequest)?;
        let response = self
            .checked(
                self.request(reqwest::Method::POST, "/v1/files")
                    .await?
                    .header("anthropic-beta", "files-api-2025-04-14,oauth-2025-04-20")
                    .timeout(Duration::from_secs(90))
                    .multipart(reqwest::multipart::Form::new().part("file", part))
                    .send()
                    .await
                    .map_err(|_| ErrorCode::Unavailable)?,
            )
            .await?;
        let file = bounded_json(response, 64 * 1024).await?;
        let id = file["id"]
            .as_str()
            .filter(|s| {
                bounded(s, 128) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
            .ok_or(ErrorCode::IncompatibleVersion)?;
        let result = self
            .json(
                reqwest::Method::POST,
                &format!("/v1/code/sessions/{session}/files"),
                Some(json!({"file_id":id,"mount_path":mount_path})),
            )
            .await;
        // The session keeps its staged copy. Clean up the temporary Files API object on every attach outcome.
        let _ = self
            .request(reqwest::Method::DELETE, &format!("/v1/files/{id}"))
            .await?
            .header("anthropic-beta", "files-api-2025-04-14,oauth-2025-04-20")
            .timeout(Duration::from_secs(20))
            .send()
            .await;
        let staged = result?;
        if staged["stage_dispatched"] != true
            || staged["file"]["size_bytes"].as_u64() != Some(length as u64)
        {
            return Err(ErrorCode::IncompatibleVersion);
        }
        Ok(json!({"mountPath":mount_path,"name":name,"byteLength":length}))
    }
    pub async fn send(
        &self,
        session: &str,
        message_id: &str,
        prompt: &str,
    ) -> Result<(), ErrorCode> {
        let session = session_reference(session)?;
        if prompt.is_empty()
            || prompt.len() > 48 * 1024
            || uuid::Uuid::parse_str(message_id).is_err()
        {
            return Err(ErrorCode::InvalidRequest);
        }
        self.json(
            reqwest::Method::POST,
            &format!("/v1/code/sessions/{session}/events"),
            Some(turn_event(&session, message_id, prompt)),
        )
        .await?;
        Ok(())
    }
    pub async fn interrupt(&self, session: &str) -> Result<(), ErrorCode> {
        let session = session_reference(session)?;
        let request_id = uuid::Uuid::new_v4().to_string();
        self.json(reqwest::Method::POST,&format!("/v1/code/sessions/{session}/events"),Some(json!({"events":[{"payload":{"uuid":uuid::Uuid::new_v4().to_string(),"type":"control_request","request_id":request_id,"request":{"subtype":"interrupt"}}}]}))).await?;
        Ok(())
    }
    pub async fn subscribe(
        &self,
        session: &str,
        after: u64,
    ) -> Result<BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>, ErrorCode> {
        let session = session_reference(session)?;
        let response = self
            .checked(
                self.request(
                    reqwest::Method::GET,
                    &format!("/v1/code/sessions/{session}/events/stream?from_sequence_num={after}"),
                )
                .await?
                .header("Accept", "text/event-stream")
                .send()
                .await
                .map_err(|_| ErrorCode::Unavailable)?,
            )
            .await?;
        Ok(response.bytes_stream().boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn files_are_uploaded_attached_and_deleted_without_credentials_in_results() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for index in 0..3 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut block = [0u8; 4096];
                let header_end = loop {
                    let read = socket.read(&mut block).await.unwrap();
                    assert!(read > 0);
                    bytes.extend_from_slice(&block[..read]);
                    if let Some(pos) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&bytes[..header_end]).to_lowercase();
                assert!(headers.contains("authorization: bearer fake-access"));
                let size = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                while bytes.len() < header_end + size {
                    let read = socket.read(&mut block).await.unwrap();
                    assert!(read > 0);
                    bytes.extend_from_slice(&block[..read]);
                }
                let body = &bytes[header_end..];
                let response = match index {
                    0 => {
                        assert!(headers.starts_with("post /v1/files "));
                        assert!(body.windows(5).any(|s| s == b"hello"));
                        r#"{"id":"file_test"}"#.to_owned()
                    }
                    1 => {
                        assert!(headers.starts_with("post /v1/code/sessions/session_test/files "));
                        let value: Value = serde_json::from_slice(body).unwrap();
                        assert_eq!(value["file_id"], "file_test");
                        assert!(
                            value["mount_path"]
                                .as_str()
                                .unwrap()
                                .starts_with("/mnt/user-data/uploads/")
                        );
                        r#"{"file":{"size_bytes":5},"stage_dispatched":true}"#.to_owned()
                    }
                    _ => {
                        assert!(headers.starts_with("delete /v1/files/file_test "));
                        "{}".to_owned()
                    }
                };
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await.unwrap();
            }
        });
        let token = auth::OAuthToken::from_json(
            br#"{"claudeAiOauth":{"accessToken":"fake-access","expiresAt":4102444800000}}"#,
        )
        .unwrap();
        let client = Client {
            http: reqwest::Client::new(),
            credentials: std::sync::Arc::new(tokio::sync::Mutex::new(Some(token))),
            endpoint: format!("http://{address}"),
        };
        let result = client
            .upload(
                "session_test",
                "hello.bin",
                "application/octet-stream",
                b"hello".to_vec(),
            )
            .await
            .unwrap();
        assert_eq!(result["byteLength"], 5);
        assert!(!result.to_string().contains("fake-access"));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn native_discovery_and_history_follow_all_pages_and_pin_the_high_water_mark() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let metadata = |id: &str, kind: &str| json!({"id":id,"environment_kind":kind,"title":kind,"created_at":"2026-10-09T10:00:00Z","worker_status":"idle"});
            let event = |id: &str, sequence: u64, kind: &str| json!({"id":id,"sequence_num":sequence,"created_at":"2026-10-09T10:00:00Z","source":"desktop","payload":{"type":kind,"uuid":id,"message":{"role":kind,"content":id}}});
            for index in 0..5 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut block = [0u8; 4096];
                loop {
                    let n = socket.read(&mut block).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&block[..n]);
                    if bytes.windows(4).any(|s| s == b"\r\n\r\n") {
                        break;
                    }
                }
                let headers = String::from_utf8_lossy(&bytes);
                let response = match index {
                    0 => { assert!(headers.starts_with("GET /v1/code/sessions?limit=100 ")); json!({"data":[metadata("cse_web","web")],"next_cursor":"private-upstream"}) },
                    1 => { assert!(headers.contains("cursor=private-upstream")); json!({"data":[metadata("cse_desktop","desktop")],"next_cursor":null}) },
                    2 => { assert!(headers.starts_with("GET /v1/code/sessions/cse_web ")); metadata("cse_web","web") },
                    3 => { assert!(headers.contains("limit=1&sort_order=desc")); json!({"data":[event("answer",7,"assistant")],"next_cursor":null}) },
                    _ => { assert!(headers.contains("limit=100&sort_order=asc")); json!({"data":[event("question",3,"user"),event("answer",7,"assistant"),event("later",9,"user")],"next_cursor":null}) },
                }.to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await.unwrap();
            }
        });
        let client = fixture_client(address);
        let sessions = client.sessions().await.unwrap();
        assert_eq!(sessions.as_array().unwrap().len(), 2);
        assert!(!sessions.to_string().contains("fake-access"));
        let history = client.history_snapshot("cse_web").await.unwrap();
        assert_eq!(history.through_sequence, 7);
        assert!(!history.active);
        assert_eq!(history.events.len(), 2);
        assert_eq!(history.events[1]["id"], "answer");
        server.await.unwrap();
    }
    fn fixture_client(address: std::net::SocketAddr) -> Client {
        let token = auth::OAuthToken::from_json(
            br#"{"claudeAiOauth":{"accessToken":"fake-access","expiresAt":4102444800000}}"#,
        )
        .unwrap();
        Client {
            http: reqwest::Client::new(),
            credentials: std::sync::Arc::new(tokio::sync::Mutex::new(Some(token))),
            endpoint: format!("http://{address}"),
        }
    }
    #[tokio::test]
    async fn missing_high_water_and_repeated_upstream_cursor_never_return_partial_history() {
        let metadata = json!({"id":"cse_original","title":"Original","created_at":"2026-10-09T10:00:00Z","worker_status":"idle"});
        let row = |sequence: u64| json!({"id":format!("event{sequence}"),"sequence_num":sequence,"created_at":"2026-10-09T10:00:00Z","payload":{"type":"user","uuid":format!("event{sequence}"),"message":{"role":"user","content":"question"}}});
        for pages in [
            vec![json!({"data":[row(3)],"next_cursor":null})],
            vec![
                json!({"data":[row(3)],"next_cursor":"repeat"}),
                json!({"data":[row(5)],"next_cursor":"repeat"}),
            ],
            vec![json!({"data":[],"truncated":true})],
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let responses = std::iter::once(metadata.clone())
                .chain(std::iter::once(json!({"data":[row(7)],"next_cursor":null})))
                .chain(pages)
                .collect::<Vec<_>>();
            let server = tokio::spawn(async move {
                for response in responses {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut bytes = Vec::new();
                    let mut block = [0; 4096];
                    loop {
                        let n = socket.read(&mut block).await.unwrap();
                        assert!(n > 0);
                        bytes.extend_from_slice(&block[..n]);
                        if bytes.windows(4).any(|b| b == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let response = response.to_string();
                    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await.unwrap();
                }
            });
            assert!(matches!(
                Client::fixture(format!("http://{address}"))
                    .history_snapshot("cse_original")
                    .await,
                Err(ErrorCode::HistoryIncomplete)
            ));
            server.await.unwrap();
        }
    }
}
