use super::ServerFrame;
use anyhow::{Result, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use wow_srp::wrath_header::{ClientDecrypterHalf, ServerEncrypterHalf, WrathServerAttempt};

pub async fn read_server_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    crypto: &mut ClientDecrypterHalf,
) -> Result<ServerFrame> {
    let mut raw = [0u8; 4];
    reader.read_exact(&mut raw).await?;
    let header = match crypto.attempt_decrypt_server_header(raw) {
        WrathServerAttempt::Header(h) => h,
        WrathServerAttempt::AdditionalByteRequired => {
            let mut extra = [0u8; 1];
            reader.read_exact(&mut extra).await?;
            crypto.decrypt_large_server_header(extra[0])
        }
    };
    if header.size < 2 {
        bail!("invalid server frame size {}", header.size)
    }
    let mut body = vec![0u8; header.size.saturating_sub(2) as usize];
    reader.read_exact(&mut body).await?;
    Ok(ServerFrame {
        opcode: header.opcode,
        body,
    })
}
pub async fn write_server_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    crypto: &mut ServerEncrypterHalf,
    frame: &ServerFrame,
) -> Result<()> {
    let size = u32::try_from(frame.body.len() + 2)?;
    let header = crypto.encrypt_server_header(size, frame.opcode).to_vec();
    writer.write_all(&header).await?;
    writer.write_all(&frame.body).await?;
    writer.flush().await?;
    Ok(())
}
