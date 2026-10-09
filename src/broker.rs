use crate::{
    auth,
    client::Client,
    protocol::{
        self, BrokerRequest, CHUNK_BYTES, ErrorCode, MAX_FILE_BYTES, MAX_FILES, MAX_TOTAL_BYTES,
        VERSION,
    },
    stream::SseDecoder,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
struct Upload {
    session: String,
    name: String,
    media_type: String,
    length: u64,
    file: tempfile::NamedTempFile,
    received: u64,
}
#[derive(Default)]
pub struct Uploads {
    items: BTreeMap<String, Upload>,
    total: u64,
}
impl Uploads {
    pub fn begin(
        &mut self,
        session: &str,
        name: &str,
        media_type: &str,
        length: u64,
    ) -> Result<String, ErrorCode> {
        let session = protocol::session_reference(session)?;
        if !protocol::bounded(name, 255)
            || name.contains(['/', '\\'])
            || matches!(name, "." | "..")
            || !protocol::bounded(media_type, 128)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        if length == 0
            || length > MAX_FILE_BYTES
            || self.total + length > MAX_TOTAL_BYTES
            || self.items.len() >= MAX_FILES
        {
            return Err(ErrorCode::TooLarge);
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let file = tempfile::NamedTempFile::new().map_err(|_| ErrorCode::FileUnavailable)?;
        self.items.insert(
            id.clone(),
            Upload {
                session,
                name: name.into(),
                media_type: media_type.into(),
                length,
                file,
                received: 0,
            },
        );
        self.total += length;
        Ok(id)
    }
    pub fn chunk(&mut self, id: &str, offset: u64, data: &str) -> Result<(), ErrorCode> {
        if data.len() > CHUNK_BYTES.div_ceil(3) * 4 {
            return Err(ErrorCode::TooLarge);
        }
        let bytes = STANDARD
            .decode(data)
            .map_err(|_| ErrorCode::InvalidRequest)?;
        let upload = self.items.get_mut(id).ok_or(ErrorCode::FileUnavailable)?;
        if offset != upload.received
            || bytes.is_empty()
            || upload.received + bytes.len() as u64 > upload.length
        {
            return Err(ErrorCode::InvalidRequest);
        }
        upload
            .file
            .write_all(&bytes)
            .map_err(|_| ErrorCode::FileUnavailable)?;
        upload.received += bytes.len() as u64;
        Ok(())
    }
    pub fn take(&mut self, id: &str) -> Result<(String, String, String, Vec<u8>), ErrorCode> {
        let upload = self.items.get(id).ok_or(ErrorCode::FileUnavailable)?;
        if upload.length != upload.received {
            return Err(ErrorCode::InvalidRequest);
        }
        let upload = self.items.remove(id).ok_or(ErrorCode::FileUnavailable)?;
        self.total -= upload.length;
        let mut bytes = Vec::with_capacity(upload.length as usize);
        upload
            .file
            .reopen()
            .map_err(|_| ErrorCode::FileUnavailable)?
            .read_to_end(&mut bytes)
            .map_err(|_| ErrorCode::FileUnavailable)?;
        Ok((upload.session, upload.name, upload.media_type, bytes))
    }
    pub fn close_session(&mut self, session: &str) {
        self.items.retain(|_, upload| upload.session != session);
        self.total = self.items.values().map(|upload| upload.length).sum();
    }
    fn discard(&mut self, id: &str) {
        if let Some(upload) = self.items.remove(id) {
            self.total -= upload.length;
        }
    }
}

async fn line(
    reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
) -> Result<Option<Vec<u8>>, ErrorCode> {
    let mut bytes = Vec::new();
    loop {
        let buf = reader
            .fill_buf()
            .await
            .map_err(|_| ErrorCode::Unavailable)?;
        if buf.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(ErrorCode::InvalidRequest)
            };
        }
        let count = buf
            .iter()
            .position(|b| *b == b'\n')
            .map(|p| p + 1)
            .unwrap_or(buf.len());
        if bytes.len() + count > protocol::MAX_FRAME_BYTES {
            return Err(ErrorCode::TooLarge);
        }
        let done = buf[count - 1] == b'\n';
        bytes.extend_from_slice(&buf[..count]);
        reader.consume(count);
        if done {
            return Ok(Some(bytes));
        }
    }
}

#[derive(Default)]
struct State {
    uploads: Uploads,
    history: crate::history::HistoryPages,
    discovery: crate::history::SessionPages,
    sessions: BTreeSet<String>,
    attached: BTreeMap<String, (String, Value)>,
    subscriptions: BTreeMap<String, tokio::task::JoinHandle<()>>,
    handshaken: bool,
    last_id: u64,
}
impl Drop for State {
    fn drop(&mut self) {
        for task in self.subscriptions.values() {
            task.abort();
        }
    }
}
impl State {
    fn require_session(&self, id: &str) -> Result<(), ErrorCode> {
        if self.sessions.contains(id) {
            Ok(())
        } else {
            Err(ErrorCode::SessionUnavailable)
        }
    }
    async fn handle(
        &mut self,
        client: &Client,
        request: BrokerRequest,
        events: &tokio::sync::mpsc::Sender<Value>,
    ) -> Result<Value, ErrorCode> {
        let id = request.id();
        if id == 0 || id <= self.last_id {
            return Err(ErrorCode::InvalidRequest);
        }
        self.last_id = id;
        if !self.handshaken && !matches!(request, BrokerRequest::Handshake { .. }) {
            return Err(ErrorCode::IncompatibleVersion);
        }
        match request {
            BrokerRequest::Handshake {
                minimum_version,
                maximum_version,
                ..
            } => {
                if self.handshaken || minimum_version > VERSION || maximum_version < VERSION {
                    return Err(ErrorCode::IncompatibleVersion);
                }
                self.handshaken = true;
                Ok(
                    json!({"capabilities":{"uploads":true,"sessionDiscovery":true,"history":true,"transcript":true,"interrupt":true,"createSession":true},"maximumFrameBytes":protocol::MAX_FRAME_BYTES,"chunkBytes":CHUNK_BYTES}),
                )
            }
            BrokerRequest::AuthStatus { .. } => {
                auth::load().await?;
                Ok(json!({"authenticated":true}))
            }
            BrokerRequest::ListSessions { cursor, limit, .. } => match cursor {
                Some(cursor) => self.discovery.page(&cursor, limit),
                None => self.discovery.start(
                    client
                        .sessions()
                        .await?
                        .as_array()
                        .ok_or(ErrorCode::IncompatibleVersion)?
                        .clone(),
                    limit,
                ),
            },
            BrokerRequest::ReadHistory {
                session_id,
                cursor,
                limit,
                ..
            } => {
                self.require_session(&session_id)?;
                match cursor {
                    Some(cursor) => self.history.page(&session_id, &cursor, limit),
                    None => self.history.start(
                        &session_id,
                        client.history_snapshot(&session_id).await?,
                        limit,
                    ),
                }
            }
            BrokerRequest::OpenSession { session_id, .. } => {
                let session = protocol::session_reference(&session_id)?;
                if !self.sessions.contains(&session) && self.sessions.len() >= 32 {
                    return Err(ErrorCode::TooLarge);
                }
                client.session(&session).await?;
                self.sessions.insert(session.clone());
                Ok(json!({"sessionId":session}))
            }
            BrokerRequest::CreateSession {
                parent_session_id,
                title,
                ..
            } => {
                if self.sessions.len() >= 32 {
                    return Err(ErrorCode::TooLarge);
                }
                let session = client.create(&parent_session_id, &title).await?;
                self.sessions.insert(session.clone());
                Ok(json!({"sessionId":session}))
            }
            BrokerRequest::UploadBegin {
                session_id,
                name,
                media_type,
                byte_length,
                ..
            } => {
                self.require_session(&session_id)?;
                Ok(
                    json!({"uploadId":self.uploads.begin(&session_id,&name,&media_type,byte_length)?}),
                )
            }
            BrokerRequest::UploadChunk {
                upload_id,
                offset,
                data,
                ..
            } => {
                self.uploads.chunk(&upload_id, offset, &data)?;
                Ok(json!({}))
            }
            BrokerRequest::UploadCommit { upload_id, .. } => {
                if self.attached.len() >= MAX_FILES {
                    return Err(ErrorCode::TooLarge);
                }
                let (session, name, media, bytes) = self.uploads.take(&upload_id)?;
                self.require_session(&session)?;
                let value = client.upload(&session, &name, &media, bytes).await?;
                self.attached
                    .insert(upload_id.clone(), (session, value.clone()));
                Ok(json!({"attachmentId":upload_id,"attachment":value}))
            }
            BrokerRequest::DiscardUpload { upload_id, .. } => {
                self.uploads.discard(&upload_id);
                self.attached.remove(&upload_id);
                Ok(json!({}))
            }
            BrokerRequest::SendTurn {
                session_id,
                mut prompt,
                attachment_ids,
                message_id,
                ..
            } => {
                self.require_session(&session_id)?;
                if attachment_ids.len() > MAX_FILES {
                    return Err(ErrorCode::TooLarge);
                }
                for id in &attachment_ids {
                    let (owner, value) = self.attached.get(id).ok_or(ErrorCode::FileUnavailable)?;
                    if owner != &session_id {
                        return Err(ErrorCode::PermissionDenied);
                    }
                    prompt.push_str(&format!(
                        "\nAttached file: {}",
                        value["mountPath"]
                            .as_str()
                            .ok_or(ErrorCode::FileUnavailable)?
                    ));
                }
                let message_id = message_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                client.send(&session_id, &message_id, &prompt).await?;
                for id in attachment_ids {
                    self.attached.remove(&id);
                }
                Ok(json!({"sessionId":session_id,"messageId":message_id}))
            }
            BrokerRequest::Subscribe {
                session_id,
                after_sequence,
                ..
            } => {
                self.require_session(&session_id)?;
                if self
                    .subscriptions
                    .get(&session_id)
                    .is_some_and(|task| !task.is_finished())
                {
                    return Err(ErrorCode::InvalidRequest);
                }
                let stream = client.subscribe(&session_id, after_sequence).await?;
                self.subscriptions.insert(
                    session_id.clone(),
                    tokio::spawn(subscription(
                        stream,
                        events.clone(),
                        session_id,
                        after_sequence,
                    )),
                );
                Ok(json!({}))
            }
            BrokerRequest::Interrupt { session_id, .. } => {
                self.require_session(&session_id)?;
                client.interrupt(&session_id).await?;
                Ok(json!({"sessionId":session_id}))
            }
            BrokerRequest::CloseSession { session_id, .. } => {
                self.sessions.remove(&session_id);
                self.history.close(&session_id);
                self.uploads.close_session(&session_id);
                if let Some(task) = self.subscriptions.remove(&session_id) {
                    task.abort();
                }
                self.attached.retain(|_, (owner, _)| owner != &session_id);
                Ok(json!({}))
            }
        }
    }
}
async fn subscription(
    mut stream: futures_util::stream::BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>,
    events: tokio::sync::mpsc::Sender<Value>,
    session: String,
    after: u64,
) {
    use futures_util::StreamExt;
    let outcome: Result<(), ErrorCode> = async {
        let mut decoder = SseDecoder::default();
        let mut gate = crate::stream::SequenceGate::new(after);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ErrorCode::Unavailable)?;
            for event in decoder.push(&chunk)? {
                let Some(sequence) = gate.accept(&event)? else { continue; };
                let frame = json!({"version":VERSION,"type":"event","sessionId":session,"sequence":sequence,"payload":event["payload"]});
                if frame.to_string().len() + 1 > protocol::MAX_FRAME_BYTES { return Err(ErrorCode::TooLarge); }
                if events.send(frame).await.is_err() { return Ok(()); }
            }
        }
        Err(ErrorCode::Unavailable)
    }.await;
    if let Err(code) = outcome {
        let _ = events
            .send(json!({"version":VERSION,"type":"stream_error","sessionId":session,"code":code}))
            .await;
    }
}
pub async fn run() -> Result<(), ErrorCode> {
    serve(
        Client::new()?,
        tokio::io::BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
    )
    .await
}
async fn serve(
    client: Client,
    mut input: impl tokio::io::AsyncBufRead + Unpin + Send + 'static,
    mut output: impl tokio::io::AsyncWrite + Unpin,
) -> Result<(), ErrorCode> {
    let mut state = State::default();
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(4);
    let input_task = tokio::spawn(async move {
        loop {
            let value = line(&mut input).await;
            let done = !matches!(value, Ok(Some(_)));
            if input_tx.send(value).await.is_err() || done {
                break;
            }
        }
    });
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<Value>(16);
    let outcome = async {
        loop {
            let response = tokio::select! {
                value = event_rx.recv() => { let Some(value) = value else { break; }; value },
                value = input_rx.recv() => {
                    let bytes = match value { Some(Ok(Some(bytes))) => bytes, Some(Err(error)) => return Err(error), _ => break };
                    let request = match protocol::decode_request(&bytes) { Ok(request) => request, Err(code) => { write(&mut output, protocol::error_response(0, code)).await?; continue; } };
                    let id = request.id();
                    match state.handle(&client, request, &event_tx).await {
                        Ok(value) => json!({"version":VERSION,"id":id,"type":"response","result":value}),
                        Err(code) => protocol::error_response(id,code),
                    }
                }
            };
            write(&mut output, response).await?;
        }
        Ok(())
    }.await;
    input_task.abort();
    outcome
}
async fn write(
    stdout: &mut (impl tokio::io::AsyncWrite + Unpin),
    value: Value,
) -> Result<(), ErrorCode> {
    let mut bytes = serde_json::to_vec(&value).map_err(|_| ErrorCode::InvalidRequest)?;
    if bytes.len() + 1 > protocol::MAX_FRAME_BYTES {
        bytes = serde_json::to_vec(&protocol::error_response(
            value["id"].as_u64().unwrap_or(0),
            ErrorCode::TooLarge,
        ))
        .map_err(|_| ErrorCode::InvalidRequest)?;
    }
    bytes.push(b'\n');
    stdout
        .write_all(&bytes)
        .await
        .map_err(|_| ErrorCode::Unavailable)?;
    stdout.flush().await.map_err(|_| ErrorCode::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn native_history_subscription_continuation_and_interrupt_keep_the_original_session() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let message = "00000000-0000-4000-8000-000000000001";
        let server = tokio::spawn(async move {
            let metadata = json!({"id":"cse_original","title":"Original","created_at":"2026-10-09T10:00:00Z","worker_status":"idle"});
            let event = |id: &str, seq: u64, kind: &str| json!({"id":id,"sequence_num":seq,"created_at":"2026-10-09T10:00:00Z","source":"desktop","payload":{"type":kind,"uuid":id,"message":{"role":kind,"content":id}}});
            for index in 0..8 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut block = [0u8; 4096];
                let header_end = loop {
                    let n = socket.read(&mut block).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&block[..n]);
                    if let Some(end) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&bytes[..header_end]).to_lowercase();
                assert!(headers.contains("authorization: bearer fake-access"));
                let length = headers
                    .lines()
                    .find_map(|s| s.strip_prefix("content-length: "))
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(0);
                while bytes.len() < header_end + length {
                    let n = socket.read(&mut block).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&block[..n]);
                }
                assert!(headers.contains("/v1/code/sessions/cse_original"));
                let response = match index {
                    0 | 1 => {
                        assert!(headers.starts_with("get /v1/code/sessions/cse_original "));
                        metadata.to_string()
                    }
                    2 => {
                        assert!(headers.contains("limit=1&sort_order=desc"));
                        json!({"data":[event("old-answer",7,"assistant")],"next_cursor":null})
                            .to_string()
                    }
                    3 => {
                        assert!(headers.contains("limit=100&sort_order=asc"));
                        json!({"data":[event("old-question",3,"user")],"next_cursor":"provider-secret-cursor"}).to_string()
                    }
                    4 => {
                        assert!(headers.contains("cursor=provider-secret-cursor"));
                        json!({"data":[event("old-answer",7,"assistant")],"next_cursor":null})
                            .to_string()
                    }
                    5 => {
                        assert!(headers.contains("/events/stream?from_sequence_num=7"));
                        let mut body = String::new();
                        for (seq, payload) in [
                            (7, json!({"type":"result","result":"old replay"})),
                            (9, json!({"type":"result","result":"unrelated"})),
                            (
                                10,
                                json!({"type":"user","uuid":message,"message":{"role":"user","content":"continue"}}),
                            ),
                            (
                                11,
                                json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"new answer"}]}}),
                            ),
                            (10, json!({"type":"result","result":"duplicate"})),
                        ] {
                            body.push_str(&format!(
                                "event: client_event\nid: {seq}\ndata: {}\n\n",
                                json!({"source":"worker","payload":payload})
                            ));
                        }
                        body
                    }
                    6 => {
                        assert!(headers.starts_with("post /v1/code/sessions/cse_original/events "));
                        let body: Value = serde_json::from_slice(&bytes[header_end..]).unwrap();
                        assert_eq!(body["events"][0]["payload"]["uuid"], message);
                        assert_eq!(body["events"][0]["payload"]["session_id"], "cse_original");
                        "{}".into()
                    }
                    _ => {
                        assert!(headers.starts_with("post /v1/code/sessions/cse_original/events "));
                        let body: Value = serde_json::from_slice(&bytes[header_end..]).unwrap();
                        assert_eq!(
                            body["events"][0]["payload"]["request"]["subtype"],
                            "interrupt"
                        );
                        "{}".into()
                    }
                };
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",if index==5 {"text/event-stream"} else {"application/json"},response.len()).as_bytes()).await.unwrap();
            }
        });
        let client = Client::fixture(format!("http://{address}"));
        let mut state = State::default();
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let request =
            |value: Value| protocol::decode_request(&serde_json::to_vec(&value).unwrap()).unwrap();
        state.handle(&client,request(json!({"version":1,"id":1,"type":"handshake","minimumVersion":1,"maximumVersion":1})),&tx).await.unwrap();
        let opened = state
            .handle(
                &client,
                request(
                    json!({"version":1,"id":2,"type":"open_session","sessionId":"cse_original"}),
                ),
                &tx,
            )
            .await
            .unwrap();
        assert_eq!(opened["sessionId"], "cse_original");
        let first=state.handle(&client,request(json!({"version":1,"id":3,"type":"read_history","sessionId":"cse_original","cursor":null,"limit":1})),&tx).await.unwrap();
        assert_eq!(first["throughSequence"], 7);
        assert!(!first.to_string().contains("provider-secret-cursor"));
        let second=state.handle(&client,request(json!({"version":1,"id":4,"type":"read_history","sessionId":"cse_original","cursor":first["nextCursor"],"limit":1})),&tx).await.unwrap();
        assert_eq!(second["events"][0]["id"], "old-answer");
        assert_eq!(second["throughSequence"], 7);
        state.handle(&client,request(json!({"version":1,"id":5,"type":"subscribe","sessionId":"cse_original","afterSequence":7})),&tx).await.unwrap();
        let sent=state.handle(&client,request(json!({"version":1,"id":6,"type":"send_turn","sessionId":"cse_original","messageId":message,"prompt":"continue"})),&tx).await.unwrap();
        assert_eq!(sent["messageId"], message);
        assert_eq!(sent["sessionId"], "cse_original");
        for sequence in [9, 10, 11] {
            let frame = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(frame["sequence"], sequence);
            assert_eq!(frame["sessionId"], "cse_original");
            assert!(!frame.to_string().contains("fake-access"));
        }
        state
            .handle(
                &client,
                request(json!({"version":1,"id":7,"type":"interrupt","sessionId":"cse_original"})),
                &tx,
            )
            .await
            .unwrap();
        state
            .handle(
                &client,
                request(
                    json!({"version":1,"id":8,"type":"close_session","sessionId":"cse_original"}),
                ),
                &tx,
            )
            .await
            .unwrap();
        assert!(state.subscriptions.is_empty());
        server.await.unwrap();
    }
}
