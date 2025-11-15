use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum frame payload supported by the framing helpers.
pub const MAX_FRAME_BYTES: usize = 256 * 1024;

/// Wire-level dezap messages transported over QUIC streams.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WireMessage {
    Text(TextMessage),
    FileMeta(FileMetadata),
    FileChunk(FileChunk),
    Ack(Ack),
    Control(ControlMessage),
}

/// Text chat payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TextMessage {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub timestamp: i64,
}

/// Metadata describing an incoming file stream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileMetadata {
    pub id: u64,
    pub name: String,
    pub size: u64,
}

/// File chunk message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileChunk {
    pub id: u64,
    pub offset: u64,
    pub bytes: Vec<u8>,
    pub last: bool,
}

/// ACK/control payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Ack {
    pub id: u64,
    pub kind: AckKind,
}

/// ACK kinds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AckKind {
    Received,
    Completed,
}

/// Control messages for future extensions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ControlMessage {
    Hello(HelloMessage),
    Denied(String),
    Info(String),
}

/// Hello handshake contents.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HelloMessage {
    pub username: String,
    pub password: Option<String>,
}

/// Serializes a [`WireMessage`] into bytes.
pub fn encode_message(message: &WireMessage) -> Result<Vec<u8>> {
    bincode::serde::encode_to_vec(message, bincode::config::standard())
        .context("failed to encode message")
}

/// Deserializes a [`WireMessage`] from bytes.
pub fn decode_message(bytes: &[u8]) -> Result<WireMessage> {
    let (message, _len) = bincode::serde::decode_from_slice(bytes, bincode::config::standard())
        .context("failed to decode message")?;
    Ok(message)
}

/// Writes a framed message to a QUIC stream.
pub async fn write_message<W>(writer: &mut W, message: &WireMessage) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let payload = encode_message(message)?;
    if payload.len() > MAX_FRAME_BYTES {
        bail!(
            "message frame ({} bytes) exceeds max {}",
            payload.len(),
            MAX_FRAME_BYTES
        );
    }

    let len = payload.len() as u32;
    writer
        .write_all(&len.to_be_bytes())
        .await
        .context("failed to write frame header")?;
    writer
        .write_all(&payload)
        .await
        .context("failed to write frame payload")?;
    writer.flush().await.context("failed to flush frame")
}

/// Reads a framed message from a QUIC stream.
pub async fn read_message<R>(reader: &mut R) -> Result<Option<WireMessage>>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err).context("failed to read frame header"),
    }

    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        bail!("incoming frame ({len} bytes) exceeds allowed size");
    }

    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .await
        .context("failed to read frame body")?;

    decode_message(&buf).map(Some)
}

/// Returns a Unix timestamp (seconds).
pub fn utc_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frame_round_trip() {
        let message = WireMessage::Text(TextMessage {
            id: 42,
            author: "tester".into(),
            body: "hello".into(),
            timestamp: utc_timestamp(),
        });

        let mut buffer = Vec::new();
        write_message(&mut buffer, &message).await.unwrap();
        let mut slice = buffer.as_slice();
        let decoded = read_message(&mut slice).await.unwrap().unwrap();
        assert_eq!(message, decoded);
    }

    #[test]
    fn encode_decode() {
        let meta = WireMessage::FileMeta(FileMetadata {
            id: 7,
            name: "file.bin".into(),
            size: 128,
        });

        let bytes = encode_message(&meta).unwrap();
        let decoded = decode_message(&bytes).unwrap();
        assert_eq!(meta, decoded);
    }
}
