use anyhow::Result;
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use rpc::proto::Envelope;

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub struct MessageId(pub u32);

pub type MessageLen = u32;
pub const MESSAGE_LEN_SIZE: usize = size_of::<MessageLen>();

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

#[cfg(any(test, feature = "test-support"))]
const COMPRESSION_LEVEL: i32 = -7;

#[cfg(not(any(test, feature = "test-support")))]
const COMPRESSION_LEVEL: i32 = 4;

pub fn message_len_from_buffer(buffer: &[u8]) -> MessageLen {
    MessageLen::from_le_bytes(buffer.try_into().unwrap())
}

fn decode_envelope(buffer: &[u8]) -> Result<Envelope> {
    // FORK:zstd-remote — dual-decode so a new GUI can attach to an old uncompressed daemon.
    if buffer.len() >= ZSTD_MAGIC.len() && buffer.starts_with(&ZSTD_MAGIC) {
        let mut decoded = Vec::new();
        zstd::stream::copy_decode(buffer, &mut decoded)?;
        return Ok(Envelope::decode_from_slice(&decoded)?);
    }
    Ok(Envelope::decode_from_slice(buffer)?)
    // FORK:end
}

pub async fn read_message_with_len<S: AsyncRead + Unpin>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
    message_len: MessageLen,
) -> Result<Envelope> {
    buffer.resize(message_len as usize, 0);
    stream.read_exact(buffer).await?;
    decode_envelope(buffer.as_slice())
}

pub async fn read_message<S: AsyncRead + Unpin>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
) -> Result<Envelope> {
    buffer.resize(MESSAGE_LEN_SIZE, 0);
    stream.read_exact(buffer).await?;

    let len = message_len_from_buffer(buffer);

    read_message_with_len(stream, buffer, len).await
}

pub async fn write_message<S: AsyncWrite + Unpin>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
    message: Envelope,
) -> Result<()> {
    // FORK:zstd-remote
    buffer.clear();
    buffer.reserve(message.encoded_size());
    message.encode_to_buffer(buffer)?;
    let compressed = zstd::stream::encode_all(buffer.as_slice(), COMPRESSION_LEVEL)?;
    let message_len = compressed.len() as u32;
    stream
        .write_all(message_len.to_le_bytes().as_slice())
        .await?;
    stream.write_all(&compressed).await?;
    Ok(())
    // FORK:end
}

pub async fn write_size_prefixed_buffer<S: AsyncWrite + Unpin>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
) -> Result<()> {
    let len = buffer.len() as u32;
    stream.write_all(len.to_le_bytes().as_slice()).await?;
    stream.write_all(buffer).await?;
    Ok(())
}

pub async fn read_message_raw<S: AsyncRead + Unpin>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
) -> Result<()> {
    buffer.resize(MESSAGE_LEN_SIZE, 0);
    stream.read_exact(buffer).await?;

    let message_len = message_len_from_buffer(buffer);
    buffer.resize(message_len as usize, 0);
    stream.read_exact(buffer).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rpc::proto::{Envelope, envelope};

    #[test]
    fn roundtrip_compresses_and_reads() {
        smol::block_on(async {
            let original = Envelope {
                id: 7,
                payload: Some(envelope::Payload::Ack(rpc::proto::Ack {})),
                ..Default::default()
            };
            let mut writer = smol::io::Cursor::new(Vec::new());
            let mut encode_buffer = Vec::new();
            write_message(&mut writer, &mut encode_buffer, original.clone())
                .await
                .unwrap();
            let written = writer.into_inner();
            assert!(
                written[MESSAGE_LEN_SIZE..].starts_with(&ZSTD_MAGIC),
                "payload should be zstd-framed"
            );
            let mut reader = smol::io::Cursor::new(written);
            let mut decode_buffer = Vec::new();
            let decoded = read_message(&mut reader, &mut decode_buffer).await.unwrap();
            assert_eq!(decoded.id, original.id);
        });
    }

    #[test]
    fn reads_uncompressed_legacy_frames() {
        smol::block_on(async {
            let original = Envelope {
                id: 11,
                payload: Some(envelope::Payload::Ack(rpc::proto::Ack {})),
                ..Default::default()
            };
            let mut raw = Vec::new();
            original.encode_to_buffer(&mut raw).unwrap();
            let mut framed = (raw.len() as u32).to_le_bytes().to_vec();
            framed.extend_from_slice(&raw);
            let mut reader = smol::io::Cursor::new(framed);
            let mut decode_buffer = Vec::new();
            let decoded = read_message(&mut reader, &mut decode_buffer).await.unwrap();
            assert_eq!(decoded.id, original.id);
        });
    }
}
