use anyhow::Result;
use tokio::io::{AsyncRead, AsyncWrite};
pub async fn relay<A, B>(a: &mut A, b: &mut B) -> Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    Ok(tokio::io::copy_bidirectional(a, b).await?)
}
