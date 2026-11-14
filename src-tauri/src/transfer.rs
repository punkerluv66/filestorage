use crate::wire::REQUEST_TIMEOUT;
use anyhow::{Result, ensure};
use iroh::endpoint::{RecvStream, SendStream};
use std::path::{Component, Path};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};

pub fn validate_name(name: &str) -> Result<()> {
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].into_iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    });
    let single_name = matches!(Path::new(name).components().next(), Some(Component::Normal(part)) if part == name);
    ensure!(
        single_name
            && name.len() <= 255
            && !name.starts_with(".pv-upload-")
            && !name.ends_with(['.', ' '])
            && !reserved
            && !name
                .chars()
                .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c)),
        "This filename is unavailable or unsupported"
    );
    Ok(())
}

pub async fn send_file(mut file: File, stream: &mut SendStream, size: u64) -> Result<()> {
    let mut hash = blake3::Hasher::new();
    let mut buffer = vec![0; 128 * 1024];
    let mut remaining = size;
    while remaining > 0 {
        let limit = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..limit]).await?;
        timeout(REQUEST_TIMEOUT, stream.write_all(&buffer[..limit])).await??;
        hash.update(&buffer[..limit]);
        remaining -= limit as u64;
    }
    ensure!(
        file.read(&mut [0]).await? == 0,
        "The source file changed during transfer"
    );
    timeout(
        REQUEST_TIMEOUT,
        stream.write_all(hash.finalize().as_bytes()),
    )
    .await??;
    stream.finish()?;
    Ok(())
}

pub async fn receive_file(stream: &mut RecvStream, mut file: File, size: u64) -> Result<()> {
    let mut hash = blake3::Hasher::new();
    let mut buffer = vec![0; 128 * 1024];
    let mut remaining = size;
    while remaining > 0 {
        let limit = remaining.min(buffer.len() as u64) as usize;
        timeout(REQUEST_TIMEOUT, stream.read_exact(&mut buffer[..limit])).await??;
        file.write_all(&buffer[..limit]).await?;
        hash.update(&buffer[..limit]);
        remaining -= limit as u64;
    }
    let mut digest = [0; 32];
    timeout(REQUEST_TIMEOUT, stream.read_exact(&mut digest)).await??;
    ensure!(
        timeout(REQUEST_TIMEOUT, stream.read(&mut [0]))
            .await??
            .is_none(),
        "The sender exceeded the declared file size"
    );
    ensure!(
        hash.finalize().as_bytes() == &digest,
        "File integrity verification failed"
    );
    file.flush().await?;
    file.sync_all().await?;
    Ok(())
}
