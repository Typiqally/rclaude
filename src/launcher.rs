//! ADR-0001: immutable launcher, verified private library copy, no credential IPC.
use std::{
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use thiserror::Error;

const ENGINE_IDENTIFIER: &str = "io.github.typiqally.rclaude.engine";
const MAX_ENGINE_BYTES: u64 = 32 * 1024 * 1024;
const CERTIFICATE: &str = match option_env!("RCLAUDE_SIGNING_CERTIFICATE_SHA1") {
    Some(value) => value,
    None => "",
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LaunchError {
    #[error("the broker launcher signing identity is not configured")]
    InvalidIdentity,
    #[error("the broker engine is unavailable or its file is unsafe")]
    UnsafeEngine,
    #[error("the broker engine signature is invalid")]
    InvalidSignature,
    #[error("the broker engine ABI is incompatible")]
    IncompatibleEngine,
}

pub fn engine_requirement(certificate: &str) -> Result<String, LaunchError> {
    if certificate.len() != 40 || !certificate.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(LaunchError::InvalidIdentity);
    }
    Ok(format!(
        "=identifier \"{ENGINE_IDENTIFIER}\" and certificate leaf = H\"{}\"",
        certificate.to_ascii_uppercase()
    ))
}

pub struct EngineCopy {
    _directory: tempfile::TempDir,
    path: PathBuf,
}
impl EngineCopy {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn private_engine_copy(source: &Path) -> Result<EngineCopy, LaunchError> {
    let denied = |_| LaunchError::UnsafeEngine;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut input = options.open(source).map_err(denied)?;
    let metadata = input.metadata().map_err(denied)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_ENGINE_BYTES {
        return Err(LaunchError::UnsafeEngine);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.nlink() != 1 || metadata.permissions().mode() & 0o022 != 0 {
            return Err(LaunchError::UnsafeEngine);
        }
    }
    let directory = tempfile::tempdir().map_err(denied)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(denied)?;
    }
    let path = directory.path().join("engine.dylib");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(denied)?;
    let count = std::io::copy(&mut input.by_ref().take(MAX_ENGINE_BYTES + 1), &mut output)
        .map_err(denied)?;
    if count == 0 || count > MAX_ENGINE_BYTES {
        return Err(LaunchError::UnsafeEngine);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).map_err(denied)?;
    }
    Ok(EngineCopy {
        _directory: directory,
        path,
    })
}

fn verify_engine(path: &Path, certificate: &str) -> Result<(), LaunchError> {
    let requirement = engine_requirement(certificate)?;
    let status = Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "--test-requirement", &requirement])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| LaunchError::InvalidSignature)?;
    if status.success() {
        Ok(())
    } else {
        Err(LaunchError::InvalidSignature)
    }
}

fn invoke_engine(
    source: &Path,
    verify: impl FnOnce(&Path) -> Result<(), LaunchError>,
) -> Result<i32, LaunchError> {
    let copy = private_engine_copy(source)?;
    verify(copy.path())?;
    // SAFETY (ADR-0001): only the signed private copy is loaded. Retain the
    // library until process exit, including when symbol lookup fails.
    let library = std::mem::ManuallyDrop::new(
        unsafe { libloading::Library::new(copy.path()) }
            .map_err(|_| LaunchError::IncompatibleEngine)?,
    );
    // SAFETY: the trusted engine's versioned C ABI takes no Rust values and
    // catches panics. Its code remains mapped for the entire process lifetime.
    let entry: libloading::Symbol<'_, unsafe extern "C" fn() -> i32> =
        unsafe { library.get(b"rclaude_engine_main_v1\0") }
            .map_err(|_| LaunchError::IncompatibleEngine)?;
    Ok(unsafe { entry() })
}

pub fn run() -> Result<i32, LaunchError> {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--launcher-info")
    {
        println!(
            "{}",
            serde_json::json!({"launcherVersion":1,"engineAbiVersion":1,"certificateSha1":CERTIFICATE})
        );
        return Ok(0);
    }
    engine_requirement(CERTIFICATE)?;
    let executable = std::env::current_exe().map_err(|_| LaunchError::UnsafeEngine)?;
    let source = executable
        .parent()
        .ok_or(LaunchError::UnsafeEngine)?
        .join(".rclaude/engine.dylib");
    invoke_engine(&source, |path| verify_engine(path, CERTIFICATE))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn signature_rejection_happens_before_library_loading() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("engine.dylib");
        std::fs::write(&source, b"not a library").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            invoke_engine(&source, |_| Err(LaunchError::InvalidSignature)).unwrap_err(),
            LaunchError::InvalidSignature
        );
    }

    #[test]
    fn the_versioned_c_entry_runs_from_the_verified_copy_after_source_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("engine.dylib");
        let mut compiler = Command::new("/usr/bin/cc")
            .args(["-dynamiclib", "-x", "c", "-o"])
            .arg(&source)
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        compiler
            .stdin
            .take()
            .unwrap()
            .write_all(b"int rclaude_engine_main_v1(void) { return 37; }\n")
            .unwrap();
        assert!(compiler.wait().unwrap().success());
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
        let status = invoke_engine(&source, |copy| {
            assert_ne!(copy, source);
            std::fs::write(&source, b"unverified replacement").unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(status, 37);
    }
}
