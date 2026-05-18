use std::net::SocketAddr;
use std::time::{Duration, Instant};

use bitcoin::consensus::{deserialize_partial, Encodable};
use bitcoin::p2p::address::Address;
use bitcoin::p2p::message::{NetworkMessage, RawNetworkMessage};
use bitcoin::p2p::message_network::VersionMessage;
use bitcoin::p2p::{Magic, ServiceFlags};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time;

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub user_agent: String,
    pub services: u64,
    pub version: i32,
    pub height: i32,
    pub latency_ms: u64,
}

#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("connect timeout")]
    ConnectTimeout,
    #[error("connect refused")]
    ConnectRefused,
    #[error("read timeout")]
    ReadTimeout,
    #[error("eof before handshake")]
    Eof,
    #[error("io: {0}")]
    Io(String),
    #[error("bad magic")]
    BadMagic,
    #[error("decode: {0}")]
    Decode(String),
    #[error("no version received")]
    NoVersion,
}

impl ProbeError {
    pub fn kind(&self) -> &'static str {
        match self {
            ProbeError::ConnectTimeout => "connect_timeout",
            ProbeError::ConnectRefused => "connect_refused",
            ProbeError::ReadTimeout => "read_timeout",
            ProbeError::Eof => "eof",
            ProbeError::Io(_) => "io",
            ProbeError::BadMagic => "bad_magic",
            ProbeError::Decode(_) => "decode",
            ProbeError::NoVersion => "no_version",
        }
    }
}

impl From<std::io::Error> for ProbeError {
    fn from(e: std::io::Error) -> Self {
        use std::io::ErrorKind::*;
        match e.kind() {
            ConnectionRefused => ProbeError::ConnectRefused,
            TimedOut => ProbeError::ConnectTimeout,
            UnexpectedEof => ProbeError::Eof,
            _ => ProbeError::Io(e.to_string()),
        }
    }
}

pub async fn handshake(
    addr: SocketAddr,
    magic: Magic,
    user_agent: &str,
    timeout: Duration,
) -> Result<PeerInfo, ProbeError> {
    let started = Instant::now();
    let connect = time::timeout(timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| ProbeError::ConnectTimeout)??;
    connect.set_nodelay(true).ok();
    let mut stream = connect;

    let our_version = build_version_message(addr, user_agent);
    write_message(&mut stream, magic, NetworkMessage::Version(our_version)).await?;

    let mut their_version: Option<VersionMessage> = None;
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(ProbeError::ReadTimeout)?;
        let msg = time::timeout(remaining, read_message(&mut stream, magic))
            .await
            .map_err(|_| ProbeError::ReadTimeout)??;
        match msg {
            NetworkMessage::Version(v) => {
                their_version = Some(v);
                write_message(&mut stream, magic, NetworkMessage::Verack).await?;
            }
            NetworkMessage::Verack => break,
            _ => continue,
        }
    }

    let v = their_version.ok_or(ProbeError::NoVersion)?;
    let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let _ = stream.shutdown().await;
    Ok(PeerInfo {
        user_agent: v.user_agent,
        services: v.services.to_u64(),
        version: v.version as i32,
        height: v.start_height,
        latency_ms,
    })
}

fn build_version_message(addr: SocketAddr, user_agent: &str) -> VersionMessage {
    let services = ServiceFlags::NONE;
    let timestamp = chrono::Utc::now().timestamp();
    // We don't know our own externally-visible addr; use the unspecified addr.
    let sender = Address::new(&"0.0.0.0:0".parse().unwrap(), ServiceFlags::NONE);
    let receiver = Address::new(&addr, ServiceFlags::NETWORK);
    let nonce = rand::random();
    VersionMessage {
        version: 70016,
        services,
        timestamp,
        receiver,
        sender,
        nonce,
        user_agent: user_agent.to_string(),
        start_height: 0,
        relay: false,
    }
}

async fn write_message(
    stream: &mut TcpStream,
    magic: Magic,
    payload: NetworkMessage,
) -> Result<(), ProbeError> {
    let raw = RawNetworkMessage::new(magic, payload);
    let mut buf = Vec::with_capacity(256);
    raw.consensus_encode(&mut buf)
        .map_err(|e| ProbeError::Decode(e.to_string()))?;
    stream.write_all(&buf).await?;
    stream.flush().await?;
    Ok(())
}

async fn read_message(stream: &mut TcpStream, magic: Magic) -> Result<NetworkMessage, ProbeError> {
    // The Bitcoin message header is 24 bytes: magic(4) + command(12) + length(4) + checksum(4).
    let mut header = [0u8; 24];
    stream.read_exact(&mut header).await?;
    if &header[0..4] != magic.to_bytes().as_slice() {
        return Err(ProbeError::BadMagic);
    }
    let length = u32::from_le_bytes([header[16], header[17], header[18], header[19]]) as usize;
    // Sanity cap: reject anything absurd (4 MiB is the protocol max for a single message).
    if length > 4 * 1024 * 1024 {
        return Err(ProbeError::Decode("payload too large".into()));
    }
    let mut payload = vec![0u8; length];
    if length > 0 {
        stream.read_exact(&mut payload).await?;
    }
    // Reassemble header + payload and let the bitcoin crate decode the whole thing.
    let mut full = Vec::with_capacity(24 + length);
    full.extend_from_slice(&header);
    full.extend_from_slice(&payload);
    let (raw, _): (RawNetworkMessage, usize) =
        deserialize_partial(&full).map_err(|e| ProbeError::Decode(e.to_string()))?;
    Ok(raw.payload().clone())
}
