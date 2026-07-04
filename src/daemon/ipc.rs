//! Lightweight IPC over Unix domain sockets.
//!
//! The daemon listens on a Unix socket for control commands from the CLI.
//! Communication uses newline-delimited JSON messages.
//!
//! # Protocol
//!
//! Client sends one of:
//! ```json
//! {"type":"ping"}
//! {"type":"stop"}
//! {"type":"status"}
//! ```
//!
//! Server responds with:
//! ```json
//! {"type":"pong"}
//! {"type":"status","pid":12345,"uptime_secs":3600}
//! {"type":"ok"}
//! {"type":"error","message":"..."}
//! ```

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use crate::log_debug;
use crate::log_error;
use crate::log_info;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

use crate::error::Error;

// ── Message types ────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcRequest {
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "status")]
    Status,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcResponse {
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "status")]
    Status {
        pid: u32,
        uptime_secs: u64,
    },
    #[serde(rename = "error")]
    Error {
        message: String,
    },
}

// ── Client ───────────────────────────────────────────────────────────────

/// Connect to the daemon's IPC socket, send a request, and return the response.
pub async fn send_request(
    socket_path: &Path,
    request: &IpcRequest,
) -> Result<IpcResponse, Error> {
    let stream = UnixStream::connect(socket_path).await.map_err(|e| {
        Error::Daemon(format!(
            "Failed to connect to daemon at {}: {}",
            socket_path.display(),
            e
        ))
    })?;

    let (reader, mut writer) = stream.into_split();

    // Send request as a single JSON line.
    let mut json = serde_json::to_string(request).map_err(|e| {
        Error::Daemon(format!("Failed to serialize IPC request: {}", e))
    })?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await.map_err(|e| {
        Error::Daemon(format!("Failed to send IPC request: {}", e))
    })?;
    writer.shutdown().await.map_err(|e| {
        Error::Daemon(format!("Failed to shutdown write side: {}", e))
    })?;

    // Read response — a single JSON line.
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();
    buf_reader.read_line(&mut line).await.map_err(|e| {
        Error::Daemon(format!("Failed to read IPC response: {}", e))
    })?;

    if line.is_empty() {
        return Err(Error::Daemon("Empty IPC response from daemon".into()));
    }

    serde_json::from_str(&line).map_err(|e| {
        Error::Daemon(format!("Failed to parse IPC response: {}", e))
    })
}

// ── Server ───────────────────────────────────────────────────────────────

/// Run the IPC server on `socket_path`.
///
/// Listens for incoming connections and handles them concurrently.
/// When `shutdown` is notified, the server stops accepting new connections
/// and returns.
pub async fn run_server(
    socket_path: &Path,
    shutdown: Arc<Notify>,
) -> Result<(), Error> {
    // Remove stale socket file if present.
    if socket_path.exists() {
        std::fs::remove_file(socket_path).map_err(|e| {
            Error::Daemon(format!(
                "Failed to remove stale socket {}: {}",
                socket_path.display(),
                e
            ))
        })?;
    }

    let listener = UnixListener::bind(socket_path).map_err(|e| {
        Error::Daemon(format!(
            "Failed to bind IPC socket at {}: {}",
            socket_path.display(),
            e
        ))
    })?;
    log_info!("IPC server listening on {}", socket_path.display());

    let start_time = Instant::now();
    let pid = std::process::id();

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _addr)) => {
                        log_debug!("IPC connection accepted");
                        let shutdown = Arc::clone(&shutdown);
                        tokio::spawn(async move {
                            handle_connection(stream, pid, start_time, shutdown).await;
                        });
                    }
                    Err(e) => {
                        log_error!("IPC accept error: {}", e);
                    }
                }
            }
            _ = shutdown.notified() => {
                log_info!("IPC server shutting down");
                break;
            }
        }
    }

    // Clean up the socket file on exit.
    let _ = std::fs::remove_file(socket_path);

    Ok(())
}

/// Handle a single IPC client connection.
async fn handle_connection(
    stream: UnixStream,
    pid: u32,
    start_time: Instant,
    shutdown: Arc<Notify>,
) {
    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    // Read the request line.
    if buf_reader.read_line(&mut line).await.is_err() {
        return;
    }

    let request: IpcRequest = match serde_json::from_str(&line) {
        Ok(req) => req,
        Err(e) => {
            send_response(&mut writer, &IpcResponse::Error {
                message: format!("Invalid request: {}", e),
            }).await;
            return;
        }
    };

    let response = match request {
        IpcRequest::Ping => IpcResponse::Pong,
        IpcRequest::Stop => {
            // Signal shutdown, then confirm.
            shutdown.notify_waiters();
            IpcResponse::Ok
        }
        IpcRequest::Status => {
            let uptime_secs = start_time.elapsed().as_secs();
            IpcResponse::Status { pid, uptime_secs }
        }
    };

    send_response(&mut writer, &response).await;
}

/// Write a JSON-line response to the client.
async fn send_response(
    writer: &mut (impl AsyncWriteExt + Unpin),
    response: &IpcResponse,
) {
    let mut json = match serde_json::to_string(response) {
        Ok(j) => j,
        Err(e) => {
            // Best-effort fallback error.
            format!(r#"{{"type":"error","message":"Failed to serialize response: {}"}}"#, e)
        }
    };
    json.push('\n');
    let _ = writer.write_all(json.as_bytes()).await;
    let _ = writer.shutdown().await;
}
