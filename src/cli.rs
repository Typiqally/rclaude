use crate::{
    client::Client,
    protocol::{ErrorCode, session_reference},
};
use clap::{Parser, Subcommand};
#[derive(Parser)]
#[command(
    version,
    about = "Remote Claude controller using the official CLI's login and terminal UI"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Broker,
    Sessions,
    AuthStatus,
    /// Grant Keychain access explicitly once, outside the broker protocol.
    Authorize,
    Connect {
        #[arg(long)]
        session: String,
        #[arg(long)]
        file: Vec<std::path::PathBuf>,
    },
    Upload {
        #[arg(long)]
        session: String,
        #[arg(long)]
        file: std::path::PathBuf,
    },
}
#[tokio::main]
pub async fn run() -> i32 {
    if let Err(code) = execute().await {
        eprintln!("rclaude: {code}");
        return 1;
    }
    0
}
async fn execute() -> Result<(), ErrorCode> {
    let args = Args::parse();
    let client = Client::new()?;
    match args.command {
        Command::Broker => crate::broker::run().await,
        Command::Authorize => {
            crate::auth::authorize_access()?;
            println!(
                "Official Claude credential access authorized. Broker calls remain noninteractive."
            );
            Ok(())
        }
        Command::AuthStatus => {
            println!(
                "Official credential record present; unexpired: {}",
                crate::auth::status()?
            );
            crate::auth::load().await?;
            println!("Official Claude OAuth login is available.");
            Ok(())
        }
        Command::Sessions => {
            println!("{}", client.sessions().await?);
            Ok(())
        }
        Command::Upload { session, file } => {
            let session = session_reference(&session)?;
            let (name, bytes) = read_file(&file)?;
            println!(
                "{}",
                client
                    .upload(&session, &name, "application/octet-stream", bytes)
                    .await?
            );
            Ok(())
        }
        Command::Connect { session, file } => {
            let session = session_reference(&session)?;
            if file.len() > crate::protocol::MAX_FILES {
                return Err(ErrorCode::TooLarge);
            }
            let mut total = 0;
            for path in file {
                let (name, bytes) = read_file(&path)?;
                total += bytes.len() as u64;
                if total > crate::protocol::MAX_TOTAL_BYTES {
                    return Err(ErrorCode::TooLarge);
                }
                let value = client
                    .upload(&session, &name, "application/octet-stream", bytes)
                    .await?;
                println!("Attached {}", value["mountPath"].as_str().unwrap_or("file"));
            }
            let status = tokio::process::Command::new("claude")
                .args(["--cloud", &session])
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .status()
                .await
                .map_err(|_| ErrorCode::Unavailable)?;
            if status.success() {
                Ok(())
            } else {
                Err(ErrorCode::Unavailable)
            }
        }
    }
}
fn read_file(path: &std::path::Path) -> Result<(String, Vec<u8>), ErrorCode> {
    use std::io::Read;
    let meta = std::fs::symlink_metadata(path).map_err(|_| ErrorCode::FileUnavailable)?;
    if !meta.is_file() || meta.len() > crate::protocol::MAX_FILE_BYTES {
        return Err(ErrorCode::FileUnavailable);
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(ErrorCode::FileUnavailable)?
        .to_owned();
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| ErrorCode::FileUnavailable)?
        .take(crate::protocol::MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::FileUnavailable)?;
    if bytes.len() as u64 > crate::protocol::MAX_FILE_BYTES {
        return Err(ErrorCode::TooLarge);
    }
    Ok((name, bytes))
}
