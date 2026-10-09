use futures_util::StreamExt;
use rclaude::{client::Client, protocol::ErrorCode, stream::SseDecoder};
use sha2::{Digest, Sha256};
#[tokio::main]
async fn main() {
    if let Err(code) = run().await {
        eprintln!("Qualification stopped: {code}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), ErrorCode> {
    let parent = std::env::args().nth(1).ok_or(ErrorCode::InvalidRequest)?;
    let client = Client::new()?;
    let value = client.session(&parent).await?;
    println!(
        "Response fields: {:?}",
        value.as_object().map(|v| v.keys().collect::<Vec<_>>())
    );
    let session = value
        .get("response_shape")
        .or_else(|| value.get("session"))
        .unwrap_or(&value);
    println!(
        "Session fields: {:?}",
        session.as_object().map(|v| v.keys().collect::<Vec<_>>())
    );
    println!(
        "Parent metadata available; bound device: {}; environment: {}",
        session.get("bound_device_id").is_some_and(|s| !s.is_null()),
        session.get("environment_id").is_some()
    );
    let created = client
        .create(&parent, "rclaude attachment qualification")
        .await?;
    println!("Created isolated qualification session: {created}");
    let bytes = (0..(1024 * 1024 + 31))
        .map(|index| ((index * 17 + 41) % 256) as u8)
        .collect::<Vec<_>>();
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let attachment = client
        .upload(
            &created,
            "qualification.bin",
            "application/octet-stream",
            bytes,
        )
        .await?;
    println!(
        "Binary attachment staged: {} bytes",
        attachment["byteLength"]
    );
    let path = attachment["mountPath"]
        .as_str()
        .ok_or(ErrorCode::IncompatibleVersion)?;
    let mut stream = client.subscribe(&created, 0).await?;
    let mut decoder = SseDecoder::default();
    client.send(&created,&uuid::Uuid::new_v4().to_string(),&format!("This is an attachment transport qualification. Read only the file {path}. Report its size in bytes and SHA256 hex digest. Do not modify repository files. Expected size is 1048607 bytes. Compute the digest from the file, do not infer it.")).await?;
    let started = std::time::Instant::now();
    while let Some(chunk) = tokio::time::timeout(std::time::Duration::from_secs(60), stream.next())
        .await
        .map_err(|_| ErrorCode::Unavailable)?
    {
        let events = decoder.push(&chunk.map_err(|_| ErrorCode::Unavailable)?)?;
        for event in events {
            let payload = &event["payload"];
            if payload["type"] == "assistant" || payload["type"] == "result" {
                let text = payload.to_string();
                if text.contains(&hash) {
                    println!("PASS: remote agent read back exact >1 MiB binary SHA256.");
                    return Ok(());
                }
                if payload["type"] == "result" {
                    return Err(ErrorCode::FileUnavailable);
                }
            }
        }
        if started.elapsed() > std::time::Duration::from_secs(180) {
            return Err(ErrorCode::Unavailable);
        }
    }
    Err(ErrorCode::Unavailable)
}
