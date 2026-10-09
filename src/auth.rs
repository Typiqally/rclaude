//! Access to the official CLI's credentials, confined to this process.
use crate::protocol::ErrorCode;
use serde_json::Value;
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
use std::{
    fmt,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

pub struct OAuthToken {
    access: Zeroizing<String>,
    refresh: Option<Zeroizing<String>>,
    expires_at: u64,
}
impl fmt::Debug for OAuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuthToken(<redacted>)")
    }
}
impl OAuthToken {
    pub fn from_json(bytes: &[u8]) -> Result<Self, ErrorCode> {
        if bytes.len() > 64 * 1024 {
            return Err(ErrorCode::AuthenticationRequired);
        }
        let document: Value =
            serde_json::from_slice(bytes).map_err(|_| ErrorCode::AuthenticationRequired)?;
        let oauth = &document["claudeAiOauth"];
        let access = oauth["accessToken"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() < 16384)
            .ok_or(ErrorCode::AuthenticationRequired)?;
        Ok(Self {
            access: Zeroizing::new(access.to_owned()),
            refresh: oauth["refreshToken"]
                .as_str()
                .map(|s| Zeroizing::new(s.to_owned())),
            expires_at: oauth["expiresAt"].as_u64().unwrap_or(0),
        })
    }
    pub(crate) fn access(&self) -> &str {
        &self.access
    }
    pub(crate) fn expired(&self) -> bool {
        self.expires_at
            <= SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
                + 60_000
    }
}

fn config_directory() -> Result<PathBuf, ErrorCode> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|s| !s.is_empty()) {
        return Ok(dir.into());
    }
    Ok(
        PathBuf::from(std::env::var_os("HOME").ok_or(ErrorCode::AuthenticationRequired)?)
            .join(".claude"),
    )
}
#[cfg(target_os = "macos")]
fn keychain_service() -> Result<String, ErrorCode> {
    let override_dir = std::env::var("CLAUDE_SECURESTORAGE_CONFIG_DIR").ok();
    let custom = override_dir
        .as_ref()
        .map(|s| !s.is_empty())
        .unwrap_or(std::env::var_os("CLAUDE_CONFIG_DIR").is_some());
    let directory = override_dir
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or(config_directory()?);
    let suffix = if custom {
        format!(
            "-{:x}",
            Sha256::digest(directory.to_string_lossy().as_bytes())
        )[..9]
            .to_owned()
    } else {
        String::new()
    };
    Ok(format!("Claude Code-credentials{suffix}"))
}

enum Location {
    #[cfg(target_os = "macos")]
    Keychain {
        service: String,
        account: String,
    },
    File(PathBuf),
}
type CredentialRecord = (Location, Zeroizing<Vec<u8>>);
fn read() -> Result<CredentialRecord, ErrorCode> {
    #[cfg(target_os = "macos")]
    {
        credential_fallback(read_keychain(), read_file)
    }
    #[cfg(not(target_os = "macos"))]
    {
        read_file()
    }
}
#[cfg(target_os = "macos")]
fn credential_fallback(
    keychain: Result<CredentialRecord, ErrorCode>,
    file: impl FnOnce() -> Result<CredentialRecord, ErrorCode>,
) -> Result<CredentialRecord, ErrorCode> {
    match keychain {
        Ok(record) => Ok(record),
        Err(keychain_error) => match file() {
            Err(ErrorCode::AuthenticationRequired) => Err(keychain_error),
            result => result,
        },
    }
}
#[cfg(target_os = "macos")]
fn read_keychain() -> Result<CredentialRecord, ErrorCode> {
    let service = keychain_service()?;
    let account = keychain_account();
    without_keychain_ui(|| {
        use security_framework::item::{ItemClass, ItemSearchOptions, SearchResult};
        let data = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(&service)
            .account(&account)
            .load_data(true)
            .skip_authenticated_items(true)
            .search();
        if let Ok(mut items) = data
            && let Some(SearchResult::Data(bytes)) = items.pop()
        {
            return Ok((
                Location::Keychain { service, account },
                Zeroizing::new(bytes),
            ));
        }
        // UISkip hides denied password items as "not found". Read only the matching
        // item's attributes to distinguish a missing login from denied access.
        // Both searches keep the process UI guard and per-query skip policy.
        let exists = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(&service)
            .account(&account)
            .load_attributes(true)
            .load_data(false)
            .skip_authenticated_items(true)
            .search()
            .is_ok_and(|items| !items.is_empty());
        Err(if exists {
            ErrorCode::CredentialAccessDenied
        } else {
            ErrorCode::AuthenticationRequired
        })
    })?
}
fn read_file() -> Result<CredentialRecord, ErrorCode> {
    let path = config_directory()?.join(".credentials.json");
    let meta = std::fs::symlink_metadata(&path).map_err(|_| ErrorCode::AuthenticationRequired)?;
    if !meta.is_file() || meta.len() > 64 * 1024 {
        return Err(ErrorCode::AuthenticationRequired);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(ErrorCode::PermissionDenied);
        }
    }
    let data = std::fs::read(&path).map_err(|_| ErrorCode::AuthenticationRequired)?;
    Ok((Location::File(path), Zeroizing::new(data)))
}

/// Refresh only inside the isolated controller; never return credentials on IPC.
pub async fn load() -> Result<OAuthToken, ErrorCode> {
    let (location, original) = read()?;
    let token = OAuthToken::from_json(&original)?;
    if !token.expired() {
        return Ok(token);
    }
    let refresh = token
        .refresh
        .as_ref()
        .ok_or(ErrorCode::AuthenticationRequired)?;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|_| ErrorCode::Unavailable)?;
    let response = http.post("https://platform.claude.com/v1/oauth/token").json(&serde_json::json!({"grant_type":"refresh_token","refresh_token":refresh.as_str(),"client_id":"9d1c250a-e61b-44d9-88ed-5944d1962f5e"})).send().await.map_err(|_| ErrorCode::AuthenticationRequired)?;
    if !response.status().is_success() {
        return Err(ErrorCode::AuthenticationRequired);
    }
    let value: Value = response
        .json()
        .await
        .map_err(|_| ErrorCode::AuthenticationRequired)?;
    let access = value["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or(ErrorCode::AuthenticationRequired)?;
    // Re-read before updating so a concurrent official CLI refresh is not overwritten.
    let (_, current) = read()?;
    if current.as_slice() != original.as_slice() {
        return OAuthToken::from_json(&current);
    }
    let mut document: Value =
        serde_json::from_slice(&original).map_err(|_| ErrorCode::AuthenticationRequired)?;
    document["claudeAiOauth"]["accessToken"] = Value::String(access.to_owned());
    document["claudeAiOauth"]["refreshToken"] = Value::String(
        value["refresh_token"]
            .as_str()
            .unwrap_or(refresh)
            .to_owned(),
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    document["claudeAiOauth"]["expiresAt"] =
        (now + value["expires_in"].as_u64().unwrap_or(3600) * 1000).into();
    let bytes = Zeroizing::new(
        serde_json::to_vec(&document).map_err(|_| ErrorCode::AuthenticationRequired)?,
    );
    match location {
        #[cfg(target_os = "macos")]
        Location::Keychain { service, account } => without_keychain_ui(|| {
            security_framework::passwords::set_generic_password(&service, &account, &bytes)
        })?
        .map_err(|_| ErrorCode::AuthenticationRequired)?,
        Location::File(path) => {
            use std::io::Write;
            let mut file = tempfile::NamedTempFile::new_in(
                path.parent().ok_or(ErrorCode::AuthenticationRequired)?,
            )
            .map_err(|_| ErrorCode::AuthenticationRequired)?;
            file.write_all(&bytes)
                .map_err(|_| ErrorCode::AuthenticationRequired)?;
            file.as_file()
                .sync_all()
                .map_err(|_| ErrorCode::AuthenticationRequired)?;
            file.persist(path)
                .map_err(|_| ErrorCode::AuthenticationRequired)?;
        }
    }
    OAuthToken::from_json(&bytes)
}

/// An explicit CLI-only authorization. Never exposed through broker frames.
/// The OS owns the authorization dialog and its password; no password is collected here.
pub fn authorize_access() -> Result<(), ErrorCode> {
    #[cfg(target_os = "macos")]
    {
        let executable =
            std::env::current_exe().map_err(|_| ErrorCode::AuthorizationSetupRequired)?;
        let verified = std::process::Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict"])
            .arg(&executable)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|_| ErrorCode::AuthorizationSetupRequired)?;
        if !verified.success() {
            return Err(ErrorCode::AuthorizationSetupRequired);
        }
        let signature = std::process::Command::new("/usr/bin/codesign")
            .args(["--display", "--verbose=2"])
            .arg(&executable)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|_| ErrorCode::AuthorizationSetupRequired)?;
        if !signature.status.success() || signature.stderr.len() > 16 * 1024 {
            return Err(ErrorCode::AuthorizationSetupRequired);
        }
        let metadata = std::str::from_utf8(&signature.stderr)
            .map_err(|_| ErrorCode::AuthorizationSetupRequired)?;
        validate_authorization_signature(metadata)?;
        let service = keychain_service()?;
        let account = keychain_account();
        // A single native read allows the user to choose Always Allow. There is no
        // retry, refresh, export, ACL rewrite, or broker/renderer entry point.
        let (bytes, _) = security_framework::os::macos::passwords::find_generic_password(
            None, &service, &account,
        )
        .map_err(|error| {
            if error.code() == -25300 {
                ErrorCode::AuthenticationRequired
            } else {
                ErrorCode::CredentialAccessDenied
            }
        })?;
        OAuthToken::from_json(&bytes)?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(ErrorCode::Unsupported)
    }
}
/// Reject an ad-hoc or unrelated signature before any interactive Keychain call.
pub fn validate_authorization_signature(metadata: &str) -> Result<(), ErrorCode> {
    if metadata
        .lines()
        .any(|line| line == "Identifier=io.github.typiqally.rclaude")
        && metadata
            .lines()
            .any(|line| line.starts_with("Authority=") && line.len() > "Authority=".len())
        && !metadata.lines().any(|line| line == "Signature=adhoc")
    {
        Ok(())
    } else {
        Err(ErrorCode::AuthorizationSetupRequired)
    }
}
#[cfg(target_os = "macos")]
fn keychain_account() -> String {
    std::env::var("USER")
        .ok()
        .filter(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
        .unwrap_or_else(|| "claude-code-user".into())
}

/// A path-free diagnostic; no token contents or identifiers are returned.
pub fn status() -> Result<bool, ErrorCode> {
    let (_, bytes) = read()?;
    Ok(!OAuthToken::from_json(&bytes)?.expired())
}

/// Organization metadata belongs to the official CLI, and remains inside the broker.
pub(crate) fn organization() -> Option<String> {
    let path = if std::env::var_os("CLAUDE_CONFIG_DIR").is_some() {
        config_directory().ok()?.join(".claude.json")
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".claude.json")
    };
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
        return None;
    }
    let value: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let org = value["oauthAccount"]["organizationUuid"].as_str()?;
    uuid::Uuid::parse_str(org).ok().map(|id| id.to_string())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn absent_fallback_preserves_denied_keychain_access() {
        let result =
            super::credential_fallback(Err(super::ErrorCode::CredentialAccessDenied), || {
                Err(super::ErrorCode::AuthenticationRequired)
            });
        assert!(matches!(
            result,
            Err(super::ErrorCode::CredentialAccessDenied)
        ));
    }
    #[test]
    fn missing_login_and_insecure_fallback_are_distinct_from_denied_keychain_access() {
        let absent =
            super::credential_fallback(Err(super::ErrorCode::AuthenticationRequired), || {
                Err(super::ErrorCode::AuthenticationRequired)
            });
        assert!(matches!(
            absent,
            Err(super::ErrorCode::AuthenticationRequired)
        ));
        let insecure =
            super::credential_fallback(Err(super::ErrorCode::CredentialAccessDenied), || {
                Err(super::ErrorCode::PermissionDenied)
            });
        assert!(matches!(insecure, Err(super::ErrorCode::PermissionDenied)));
    }
    #[test]
    fn an_existing_official_file_can_supply_credentials_when_keychain_access_is_denied() {
        let from_file =
            super::credential_fallback(Err(super::ErrorCode::CredentialAccessDenied), || {
                Ok((
                    super::Location::File("official-fixture".into()),
                    zeroize::Zeroizing::new(b"fixture".to_vec()),
                ))
            });
        assert!(matches!(from_file, Ok((super::Location::File(_), _))));
        let from_keychain = super::credential_fallback(
            Ok((
                super::Location::Keychain {
                    service: "fixture".into(),
                    account: "fixture".into(),
                },
                zeroize::Zeroizing::new(b"fixture".to_vec()),
            )),
            || panic!("a usable credential must not probe another source"),
        );
        assert!(matches!(
            from_keychain,
            Ok((super::Location::Keychain { .. }, _))
        ));
    }
    #[test]
    fn keychain_reads_and_writes_cannot_open_an_access_dialog() {
        let allowed = super::without_keychain_ui(|| {
            security_framework::os::macos::keychain::SecKeychain::user_interaction_allowed()
                .unwrap()
        })
        .unwrap();
        assert!(!allowed);
    }
}

#[cfg(target_os = "macos")]
fn without_keychain_ui<T>(operation: impl FnOnce() -> T) -> Result<T, ErrorCode> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _lock = LOCK.lock().map_err(|_| ErrorCode::AuthenticationRequired)?;
    let _no_dialogs =
        security_framework::os::macos::keychain::SecKeychain::disable_user_interaction()
            .map_err(|_| ErrorCode::AuthenticationRequired)?;
    Ok(operation())
}
