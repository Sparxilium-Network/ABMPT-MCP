//! Minimal Source RCON client (as used by Minecraft).

use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const TYPE_AUTH: i32 = 3;
const TYPE_EXEC: i32 = 2;
const TYPE_SENTINEL: i32 = 0;

async fn send(stream: &mut TcpStream, id: i32, ty: i32, body: &str) -> Result<()> {
    let body = body.as_bytes();
    let len = (4 + 4 + body.len() + 2) as i32;
    let mut buf = Vec::with_capacity(len as usize + 4);
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(&id.to_le_bytes());
    buf.extend_from_slice(&ty.to_le_bytes());
    buf.extend_from_slice(body);
    buf.extend_from_slice(&[0, 0]);
    stream.write_all(&buf).await?;
    Ok(())
}

async fn recv(stream: &mut TcpStream) -> Result<(i32, i32, String)> {
    let len = stream.read_i32_le().await?;
    if !(10..=1 << 20).contains(&len) {
        bail!("invalid RCON packet length {len}");
    }
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf).await?;
    let id = i32::from_le_bytes(buf[0..4].try_into().unwrap());
    let ty = i32::from_le_bytes(buf[4..8].try_into().unwrap());
    let body = String::from_utf8_lossy(&buf[8..buf.len() - 2]).into_owned();
    Ok((id, ty, body))
}

/// Run a single command and return its (raw) output. The password never leaves this module.
pub async fn exec(port: u16, password: &str, command: &str) -> Result<String> {
    let fut = async {
        let mut s = TcpStream::connect(("127.0.0.1", port))
            .await
            .context("cannot connect to RCON")?;
        send(&mut s, 1, TYPE_AUTH, password).await?;
        loop {
            let (id, ty, _) = recv(&mut s).await?;
            if id == -1 {
                bail!("RCON authentication failed");
            }
            // Some servers send an empty RESPONSE_VALUE before the AUTH_RESPONSE.
            if ty == 2 && id == 1 {
                break;
            }
        }
        send(&mut s, 2, TYPE_EXEC, command).await?;
        // Long output is split into several packets; an unknown-type packet afterwards
        // acts as a sentinel because the server answers requests in order.
        send(&mut s, 3, TYPE_SENTINEL, "").await?;
        let mut out = String::new();
        loop {
            let (id, _, body) = recv(&mut s).await?;
            match id {
                2 => out.push_str(&body),
                3 => break,
                _ => {}
            }
        }
        Ok(out)
    };
    timeout(Duration::from_secs(30), fut)
        .await
        .context("RCON command timed out")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn round_trip() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let (id, ty, body) = recv(&mut s).await.unwrap();
            assert_eq!((ty, body.as_str()), (TYPE_AUTH, "pw"));
            send(&mut s, id, 2, "").await.unwrap();
            let (id, ty, body) = recv(&mut s).await.unwrap();
            assert_eq!((ty, body.as_str()), (TYPE_EXEC, "list"));
            send(&mut s, id, 0, "§aThere are 0").await.unwrap();
            send(&mut s, id, 0, " players").await.unwrap();
            let (id, _, _) = recv(&mut s).await.unwrap();
            send(&mut s, id, 0, "Unknown request 0").await.unwrap();
        });
        let out = exec(port, "pw", "list").await.unwrap();
        assert_eq!(out, "§aThere are 0 players");
    }
}
