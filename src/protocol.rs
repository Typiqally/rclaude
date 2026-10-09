//! Independently implemented, bounded controller protocol.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;
use thiserror::Error;
use url::Url;

pub const VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_FILES: usize = 10;
pub const CHUNK_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, Error, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    #[error("request is invalid")]
    InvalidRequest,
    #[error("request exceeds its size limit")]
    TooLarge,
    #[error("official Claude login is required")]
    AuthenticationRequired,
    #[error("provider authentication scope is unavailable")]
    PermissionDenied,
    #[error("legitimate device enrollment is required")]
    EnrollmentRequired,
    #[error("the provider protocol is incompatible")]
    IncompatibleVersion,
    #[error("session is unavailable")]
    SessionUnavailable,
    #[error("provider is unavailable")]
    Unavailable,
    #[error("provider temporarily limited requests")]
    RateLimited,
    #[error("file is unavailable")]
    FileUnavailable,
    #[error("history catch-up is incomplete")]
    HistoryIncomplete,
    #[error("operation is unsupported")]
    Unsupported,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrokerRequest {
    Handshake {
        version: u32,
        id: u64,
        #[serde(rename = "minimumVersion")]
        minimum_version: u32,
        #[serde(rename = "maximumVersion")]
        maximum_version: u32,
    },
    AuthStatus {
        version: u32,
        id: u64,
    },
    ListSessions {
        version: u32,
        id: u64,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    ReadHistory {
        version: u32,
        id: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    OpenSession {
        version: u32,
        id: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    CreateSession {
        version: u32,
        id: u64,
        #[serde(rename = "parentSessionId")]
        parent_session_id: String,
        title: String,
    },
    UploadBegin {
        version: u32,
        id: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        name: String,
        #[serde(rename = "mediaType")]
        media_type: String,
        #[serde(rename = "byteLength")]
        byte_length: u64,
    },
    UploadChunk {
        version: u32,
        id: u64,
        #[serde(rename = "uploadId")]
        upload_id: String,
        offset: u64,
        data: String,
    },
    UploadCommit {
        version: u32,
        id: u64,
        #[serde(rename = "uploadId")]
        upload_id: String,
    },
    DiscardUpload {
        version: u32,
        id: u64,
        #[serde(rename = "uploadId")]
        upload_id: String,
    },
    SendTurn {
        version: u32,
        id: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        prompt: String,
        #[serde(rename = "attachmentIds", default)]
        attachment_ids: Vec<String>,
        #[serde(rename = "messageId", default)]
        message_id: Option<String>,
    },
    Subscribe {
        version: u32,
        id: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "afterSequence", default)]
        after_sequence: u64,
    },
    Interrupt {
        version: u32,
        id: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    CloseSession {
        version: u32,
        id: u64,
        #[serde(rename = "sessionId")]
        session_id: String,
    },
}

impl fmt::Debug for BrokerRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrokerRequest")
            .field("id", &self.id())
            .field("input", &"<redacted>")
            .finish()
    }
}

impl BrokerRequest {
    pub fn id(&self) -> u64 {
        match self {
            Self::Handshake { id, .. }
            | Self::AuthStatus { id, .. }
            | Self::ListSessions { id, .. }
            | Self::ReadHistory { id, .. }
            | Self::OpenSession { id, .. }
            | Self::CreateSession { id, .. }
            | Self::UploadBegin { id, .. }
            | Self::UploadChunk { id, .. }
            | Self::UploadCommit { id, .. }
            | Self::DiscardUpload { id, .. }
            | Self::SendTurn { id, .. }
            | Self::Subscribe { id, .. }
            | Self::Interrupt { id, .. }
            | Self::CloseSession { id, .. } => *id,
        }
    }
    pub fn version(&self) -> u32 {
        match self {
            Self::Handshake { version, .. }
            | Self::AuthStatus { version, .. }
            | Self::ListSessions { version, .. }
            | Self::ReadHistory { version, .. }
            | Self::OpenSession { version, .. }
            | Self::CreateSession { version, .. }
            | Self::UploadBegin { version, .. }
            | Self::UploadChunk { version, .. }
            | Self::UploadCommit { version, .. }
            | Self::DiscardUpload { version, .. }
            | Self::SendTurn { version, .. }
            | Self::Subscribe { version, .. }
            | Self::Interrupt { version, .. }
            | Self::CloseSession { version, .. } => *version,
        }
    }
}

pub fn decode_request(bytes: &[u8]) -> Result<BrokerRequest, ErrorCode> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(ErrorCode::TooLarge);
    }
    let request: BrokerRequest =
        serde_json::from_slice(bytes).map_err(|_| ErrorCode::InvalidRequest)?;
    if request.version() != VERSION {
        return Err(ErrorCode::IncompatibleVersion);
    }
    if let BrokerRequest::ListSessions { cursor, limit, .. }
    | BrokerRequest::ReadHistory { cursor, limit, .. } = &request
        && (!(1..=100).contains(limit) || cursor.as_ref().is_some_and(|s| !bounded(s, 128)))
    {
        return Err(ErrorCode::InvalidRequest);
    }
    Ok(request)
}
fn default_limit() -> usize {
    100
}

pub fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

pub fn session_reference(value: &str) -> Result<String, ErrorCode> {
    if !bounded(value, 4096) {
        return Err(ErrorCode::InvalidRequest);
    }
    let normalized;
    let reference = if value.starts_with("https://") {
        let url = Url::parse(value).map_err(|_| ErrorCode::InvalidRequest)?;
        if url.scheme() != "https"
            || url.host_str() != Some("claude.ai")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            return Err(ErrorCode::InvalidRequest);
        }
        normalized = url
            .path()
            .strip_prefix("/code/")
            .ok_or(ErrorCode::InvalidRequest)?
            .to_owned();
        normalized.as_str()
    } else {
        value
    };
    let suffix = reference
        .strip_prefix("session_")
        .or_else(|| reference.strip_prefix("cse_"))
        .ok_or(ErrorCode::InvalidRequest)?;
    if suffix.is_empty()
        || !bounded(reference, 128)
        || !suffix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(ErrorCode::InvalidRequest);
    }
    Ok(reference.to_owned())
}

pub fn turn_event(session_id: &str, message_id: &str, prompt: &str) -> Value {
    json!({"events":[{"payload":{"uuid":message_id,"session_id":session_id,"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":prompt}}}]})
}

pub fn error_response(id: u64, code: ErrorCode) -> Value {
    json!({"version":VERSION,"id":id,"type":"error","code":code})
}
