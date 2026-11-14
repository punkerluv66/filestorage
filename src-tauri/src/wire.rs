use anyhow::{Result, ensure};
use iroh::endpoint::{RecvStream, SendStream};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;
use tokio::time::timeout;

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
pub const DECISION_TIMEOUT: Duration = Duration::from_secs(120);

pub async fn write_message<T: Serialize>(stream: &mut SendStream, value: &T) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    ensure!(data.len() <= 4096, "Protocol message is too large");
    stream.write_all(&(data.len() as u16).to_be_bytes()).await?;
    stream.write_all(&data).await?;
    Ok(())
}

pub async fn read_message<T: DeserializeOwned>(
    stream: &mut RecvStream,
    wait: Duration,
) -> Result<T> {
    timeout(wait, async {
        let mut length = [0; 2];
        stream.read_exact(&mut length).await?;
        let length = usize::from(u16::from_be_bytes(length));
        ensure!(length <= 4096, "Protocol message is too large");
        let mut data = vec![0; length];
        stream.read_exact(&mut data).await?;
        Ok(serde_json::from_slice(&data)?)
    })
    .await?
}
