use super::ClientFrame;
use anyhow::{Result, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use wow_srp::wrath_header::{ClientEncrypterHalf, ServerDecrypterHalf};

pub async fn read_client_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    crypto: &mut ServerDecrypterHalf,
) -> Result<ClientFrame> {
    let mut raw = [0u8; 6];
    reader.read_exact(&mut raw).await?;
    let header = crypto.decrypt_client_header(raw);
    if header.size < 4 {
        bail!("invalid client frame size {}", header.size)
    }
    let mut body = vec![0u8; header.size.saturating_sub(4) as usize];
    reader.read_exact(&mut body).await?;
    Ok(ClientFrame {
        opcode: header.opcode,
        body,
    })
}
pub async fn write_client_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    crypto: &mut ClientEncrypterHalf,
    frame: &ClientFrame,
) -> Result<()> {
    let size = u16::try_from(frame.body.len() + 4)?;
    let header = crypto.encrypt_client_header(size, frame.opcode);
    writer.write_all(&header).await?;
    writer.write_all(&frame.body).await?;
    writer.flush().await?;
    Ok(())
}
