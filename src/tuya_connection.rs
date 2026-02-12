use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::config::MeacoConfig;
use crate::tuya_protocol::{
    self, TuyaFrame, TuyaMessage, ProtocolError,
    HEADER_SIZE, PREFIX,
    CMD_HEART_BEAT, CMD_CONTROL, CMD_DP_QUERY,
};

/// Shared connection data. Not an object — just data that systems operate on.
pub struct TuyaConnection {
    pub stream: Mutex<TcpStream>,
    pub device_id: String,
    pub local_key: [u8; 16],
    seqno: AtomicU32,
}

impl std::fmt::Debug for TuyaConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TuyaConnection")
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum ConnectionError {
    Tcp(std::io::Error),
    Protocol(ProtocolError),
    Timeout,
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionError::Tcp(e) => write!(f, "TCP error: {e}"),
            ConnectionError::Protocol(e) => write!(f, "Protocol error: {e}"),
            ConnectionError::Timeout => write!(f, "Connection timed out"),
        }
    }
}

impl std::error::Error for ConnectionError {}

impl From<std::io::Error> for ConnectionError {
    fn from(e: std::io::Error) -> Self {
        ConnectionError::Tcp(e)
    }
}

impl From<ProtocolError> for ConnectionError {
    fn from(e: ProtocolError) -> Self {
        ConnectionError::Protocol(e)
    }
}

fn next_seqno(conn: &TuyaConnection) -> u32 {
    conn.seqno.fetch_add(1, Ordering::Relaxed)
}

fn local_key_from_config(config: &MeacoConfig) -> [u8; 16] {
    let mut key = [0u8; 16];
    key.copy_from_slice(config.local_key.as_bytes());
    key
}

/// Connect to the Tuya device over TCP port 6668.
pub async fn connect(config: &MeacoConfig) -> Result<Arc<TuyaConnection>, ConnectionError> {
    let addr = format!("{}:6668", config.device_ip);

    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        TcpStream::connect(&addr),
    )
    .await
    .map_err(|_| ConnectionError::Timeout)?
    .map_err(ConnectionError::Tcp)?;

    tracing::info!(addr = %addr, "Connected to Tuya device");

    Ok(Arc::new(TuyaConnection {
        stream: Mutex::new(stream),
        device_id: config.device_id.to_owned(),
        local_key: local_key_from_config(config),
        seqno: AtomicU32::new(1),
    }))
}

/// Write a frame to the TCP stream.
async fn write_frame(stream: &mut TcpStream, frame: &TuyaFrame) -> Result<(), ConnectionError> {
    stream.write_all(&frame.bytes).await?;
    stream.flush().await?;
    Ok(())
}

/// Read a complete frame from the TCP stream.
/// Reads the 16-byte header first to get the length, then reads the rest.
async fn read_frame(
    stream: &mut TcpStream,
    local_key: &[u8; 16],
) -> Result<TuyaMessage, ConnectionError> {
    // Read header (16 bytes)
    let mut header = [0u8; HEADER_SIZE];
    stream.read_exact(&mut header).await?;

    // Validate prefix
    let prefix = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    if prefix != PREFIX {
        return Err(ProtocolError::InvalidPrefix(prefix).into());
    }

    // Extract length to know how much more to read
    let length = u32::from_be_bytes([header[12], header[13], header[14], header[15]]) as usize;

    // Read the rest: retcode + payload + crc + suffix
    let mut rest = vec![0u8; length];
    stream.read_exact(&mut rest).await?;

    // Reassemble complete frame for parsing
    let mut full_frame = Vec::with_capacity(HEADER_SIZE + length);
    full_frame.extend_from_slice(&header);
    full_frame.extend_from_slice(&rest);

    tuya_protocol::parse_frame(&full_frame, local_key).map_err(ConnectionError::Protocol)
}

/// Send a frame and receive the response.
/// Holds the stream lock for the duration to ensure request-response pairing.
pub async fn send_receive(
    conn: &TuyaConnection,
    cmd: u32,
    json_payload: &[u8],
) -> Result<TuyaMessage, ConnectionError> {
    let seqno = next_seqno(conn);
    let frame = tuya_protocol::build_frame(seqno, cmd, json_payload, &conn.local_key);

    let mut stream = conn.stream.lock().await;

    write_frame(&mut stream, &frame).await?;

    // Read response, with a timeout
    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        read_frame(&mut stream, &conn.local_key),
    )
    .await
    .map_err(|_| ConnectionError::Timeout)??;

    Ok(msg)
}

/// Query all data points from the device.
pub async fn query_dps(conn: &TuyaConnection) -> Result<serde_json::Value, ConnectionError> {
    let json = tuya_protocol::build_dp_query_json(&conn.device_id);
    let msg = send_receive(conn, CMD_DP_QUERY, &json).await?;

    let response: serde_json::Value =
        serde_json::from_slice(&msg.payload).unwrap_or(serde_json::Value::Null);

    Ok(response)
}

/// Set data points on the device.
pub async fn set_dps(
    conn: &TuyaConnection,
    dps: serde_json::Value,
) -> Result<serde_json::Value, ConnectionError> {
    let json = tuya_protocol::build_control_json(&conn.device_id, &dps);
    let msg = send_receive(conn, CMD_CONTROL, &json).await?;

    let response: serde_json::Value =
        serde_json::from_slice(&msg.payload).unwrap_or(serde_json::Value::Null);

    Ok(response)
}

/// Spawn a heartbeat task that pings the device every `interval_secs` seconds.
pub fn spawn_heartbeat(
    conn: Arc<TuyaConnection>,
    interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));

        loop {
            interval.tick().await;

            let json = tuya_protocol::build_heartbeat_json();
            match send_receive(&conn, CMD_HEART_BEAT, &json).await {
                Ok(_) => tracing::trace!("Heartbeat OK"),
                Err(e) => tracing::warn!("Heartbeat failed: {e}"),
            }
        }
    })
}
