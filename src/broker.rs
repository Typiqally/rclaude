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

pub async fn run() -> Result<(), ErrorCode> {
    let client = Client::new()?;
    let mut uploads = Uploads::default();
    let mut sessions = BTreeSet::new();
    let mut attached: BTreeMap<String, (String, Value)> = BTreeMap::new();
    let mut subscriptions: BTreeMap<String, tokio::task::JoinHandle<()>> = BTreeMap::new();
    let mut handshaken = false;
    let mut last_id = 0;
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(4);
    let input_task = tokio::spawn(async move {
        let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
        loop {
            let value = line(&mut stdin).await;
            let done = !matches!(value, Ok(Some(_)));
            if input_tx.send(value).await.is_err() || done {
                break;
            }
        }
    });
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<Value>(16);
    let mut stdout = tokio::io::stdout();
    let outcome=async {
 loop{
  let response=tokio::select!{
   value=event_rx.recv()=>{let Some(value)=value else {break};value},
   value=input_rx.recv()=>{
    let bytes=match value {Some(Ok(Some(bytes)))=>bytes,Some(Err(error))=>return Err(error),_=>break};
    let request=match protocol::decode_request(&bytes){Ok(request)=>request,Err(code)=>{write(&mut stdout,protocol::error_response(0,code)).await?;continue}};
    let id=request.id();
    if id==0||id<=last_id {write(&mut stdout,protocol::error_response(id,ErrorCode::InvalidRequest)).await?;continue;}last_id=id;
    if !handshaken&&!matches!(request,BrokerRequest::Handshake{..}){write(&mut stdout,protocol::error_response(id,ErrorCode::IncompatibleVersion)).await?;continue;}
    let result:Result<Value,ErrorCode>=async {
    match request {
     BrokerRequest::Handshake{minimum_version,maximum_version,..}=>{if handshaken||minimum_version>VERSION||maximum_version<VERSION{return Err(ErrorCode::IncompatibleVersion);}handshaken=true;Ok(json!({"capabilities":{"uploads":true,"transcript":true,"interrupt":true,"createSession":true},"maximumFrameBytes":protocol::MAX_FRAME_BYTES,"chunkBytes":CHUNK_BYTES}))},
     BrokerRequest::AuthStatus{..}=>{auth::load().await?;Ok(json!({"authenticated":true}))},
     BrokerRequest::ListSessions{..}=>Ok(json!({"sessions":client.sessions().await?})),
     BrokerRequest::OpenSession{session_id,..}=>{if sessions.len() >= 32 { return Err(ErrorCode::TooLarge); }let session=protocol::session_reference(&session_id)?;client.session(&session).await?;sessions.insert(session.clone());Ok(json!({"sessionId":session}))},
     BrokerRequest::CreateSession{parent_session_id,title,..}=>{if sessions.len() >= 32 { return Err(ErrorCode::TooLarge); }let session=client.create(&parent_session_id,&title).await?;sessions.insert(session.clone());Ok(json!({"sessionId":session}))},
     BrokerRequest::UploadBegin{session_id,name,media_type,byte_length,..}=>{if !sessions.contains(&session_id){return Err(ErrorCode::SessionUnavailable);}Ok(json!({"uploadId":uploads.begin(&session_id,&name,&media_type,byte_length)?}))},
     BrokerRequest::UploadChunk{upload_id,offset,data,..}=>{uploads.chunk(&upload_id,offset,&data)?;Ok(json!({}))},
     BrokerRequest::UploadCommit{upload_id,..}=>{if attached.len() >= MAX_FILES { return Err(ErrorCode::TooLarge); }let(session,name,media,bytes)=uploads.take(&upload_id)?;if !sessions.contains(&session){return Err(ErrorCode::SessionUnavailable);}let value=client.upload(&session,&name,&media,bytes).await?;attached.insert(upload_id.clone(),(session,value.clone()));Ok(json!({"attachmentId":upload_id,"attachment":value}))},
     BrokerRequest::DiscardUpload{upload_id,..}=>{uploads.discard(&upload_id);attached.remove(&upload_id);Ok(json!({}))},
     BrokerRequest::SendTurn{session_id,prompt,attachment_ids,message_id,..}=>{
      if !sessions.contains(&session_id)||attachment_ids.len()>MAX_FILES{return Err(ErrorCode::InvalidRequest);}let mut prompt=prompt;
      for id in &attachment_ids{let(owner,value)=attached.get(id).ok_or(ErrorCode::FileUnavailable)?;if owner!=&session_id{return Err(ErrorCode::PermissionDenied);}prompt.push_str(&format!("\nAttached file: {}",value["mountPath"].as_str().ok_or(ErrorCode::FileUnavailable)?));}
      let message_id=message_id.unwrap_or_else(||uuid::Uuid::new_v4().to_string());client.send(&session_id,&message_id,&prompt).await?;for id in attachment_ids { attached.remove(&id); }Ok(json!({"messageId":message_id}))},
     BrokerRequest::Subscribe{session_id,after_sequence,..}=>{
      if !sessions.contains(&session_id){return Err(ErrorCode::SessionUnavailable);}
      if subscriptions.contains_key(&session_id){return Err(ErrorCode::InvalidRequest);}let mut stream=client.subscribe(&session_id,after_sequence).await?;let tx=event_tx.clone();let session=session_id.clone();
      let task=tokio::spawn(async move{use futures_util::StreamExt;let mut decoder=SseDecoder::default();while let Some(chunk)=stream.next().await{let events=match chunk{Ok(chunk)=>decoder.push(&chunk),Err(_)=>Err(ErrorCode::Unavailable)};match events{Ok(events)=>{for event in events{if tx.send(json!({"version":VERSION,"type":"event","sessionId":session,"sequence":event["sequence"],"payload":event["payload"]})).await.is_err(){return;}}},Err(code)=>{let _=tx.send(json!({"version":VERSION,"type":"stream_error","sessionId":session,"code":code})).await;return;}}}let _=tx.send(json!({"version":VERSION,"type":"stream_error","sessionId":session,"code":ErrorCode::Unavailable})).await;});subscriptions.insert(session_id,task);Ok(json!({}))},
     BrokerRequest::Interrupt{session_id,..}=>{if !sessions.contains(&session_id){return Err(ErrorCode::SessionUnavailable);}client.interrupt(&session_id).await?;Ok(json!({}))},
     BrokerRequest::CloseSession{session_id,..}=>{sessions.remove(&session_id);uploads.close_session(&session_id);if let Some(task)=subscriptions.remove(&session_id){task.abort();}attached.retain(|_,(owner,_)|owner!=&session_id);Ok(json!({}))},
    }}.await;
    match result{Ok(value)=>json!({"version":VERSION,"id":id,"type":"response","result":value}),Err(code)=>protocol::error_response(id,code)}
   }
  };
  write(&mut stdout,response).await?;
 }
 Ok(())
 }.await;
    input_task.abort();
    for task in subscriptions.values() {
        task.abort();
    }
    outcome
}
async fn write(stdout: &mut tokio::io::Stdout, value: Value) -> Result<(), ErrorCode> {
    let mut bytes = serde_json::to_vec(&value).map_err(|_| ErrorCode::InvalidRequest)?;
    if bytes.len() > protocol::MAX_FRAME_BYTES {
        return Err(ErrorCode::TooLarge);
    }
    bytes.push(b'\n');
    stdout
        .write_all(&bytes)
        .await
        .map_err(|_| ErrorCode::Unavailable)?;
    stdout.flush().await.map_err(|_| ErrorCode::Unavailable)
}
