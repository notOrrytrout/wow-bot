use crate::codec::{decode, encode, CodecError, MAX_FRAME};
use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, Error)]
pub enum NetCodecError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error("frame length {0} exceeds limit")]
    TooLarge(usize),
}

pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<(), NetCodecError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let body = encode(value)?;
    writer.write_u32(body.len() as u32).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_frame<R, T>(reader: &mut R) -> Result<T, NetCodecError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let len = reader.read_u32().await? as usize;
    if len > MAX_FRAME {
        return Err(NetCodecError::TooLarge(len));
    }
    let mut body = vec![0_u8; len];
    reader.read_exact(&mut body).await?;
    Ok(decode(&body)?)
}
