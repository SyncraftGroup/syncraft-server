//! IPC over Unix domain sockets using newline-delimited JSON.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use crate::daemon::SyncraftServer;
use crate::log_debug;
use crate::log_error;
use crate::log_info;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::error::Error;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcRequest {
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "update")]
    Update,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcResponse {
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "status")]
    Status { pid: u32, uptime_secs: u64 },
    #[serde(rename = "error")]
    Error { message: String },
}

/// Connect to the daemon's IPC socket, send a request, and return the response.
pub async fn send_request(socket_path: &Path, request: &IpcRequest) -> Result<IpcResponse, Error> {
    let stream = UnixStream::connect(socket_path).await.map_err(|e| {
        Error::Daemon(format!(
            "Failed to connect to daemon at {}: {}",
            socket_path.display(),
            e
        ))
    })?;

    let (reader, mut writer) = stream.into_split();

    let mut json = serde_json::to_string(request)
        .map_err(|e| Error::Daemon(format!("Failed to serialize IPC request: {}", e)))?;
    json.push('\n');
    writer
        .write_all(json.as_bytes())
        .await
        .map_err(|e| Error::Daemon(format!("Failed to send IPC request: {}", e)))?;
    writer
        .shutdown()
        .await
        .map_err(|e| Error::Daemon(format!("Failed to shutdown write side: {}", e)))?;

    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();
    buf_reader
        .read_line(&mut line)
        .await
        .map_err(|e| Error::Daemon(format!("Failed to read IPC response: {}", e)))?;

    if line.is_empty() {
        return Err(Error::Daemon("Empty IPC response from daemon".into()));
    }

    serde_json::from_str(&line)
        .map_err(|e| Error::Daemon(format!("Failed to parse IPC response: {}", e)))
}

/// Check whether the daemon is alive and responsive.
///
/// First checks the PID file, then sends a Ping over the IPC socket.
/// Returns `true` only if both checks pass.
pub fn is_daemon_alive(socket_path: &Path, pid_file: &Path) -> bool {
    if !pid_file_alive(pid_file) {
        return false;
    }

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return false,
    };

    rt.block_on(async {
        matches!(
            send_request(socket_path, &IpcRequest::Ping).await,
            Ok(IpcResponse::Pong)
        )
    })
}

/// Return `true` if the PID file references a live process.
fn pid_file_alive(pid_file: &Path) -> bool {
    match fs::read_to_string(pid_file) {
        Ok(s) => match s.trim().parse::<i32>() {
            Ok(pid) => super::manager::process_exists(pid),
            Err(_) => false,
        },
        Err(_) => false,
    }
}

async fn send_response(writer: &mut (impl AsyncWriteExt + Unpin), response: &IpcResponse) {
    let mut json = match serde_json::to_string(response) {
        Ok(j) => j,
        Err(e) => {
            format!(
                r#"{{"type":"error","message":"Failed to serialize response: {}"}}"#,
                e
            )
        }
    };
    json.push('\n');
    let _ = writer.write_all(json.as_bytes()).await;
    let _ = writer.shutdown().await;
}

impl SyncraftServer {
    pub async fn run_ipc(self: Arc<Self>) -> Result<(), Error>
    where
        Self: Send + 'static,
    {
        let socket_path = self.paths.socket_path();
        let shutdown = self.shutdown.clone();

        if socket_path.exists() {
            std::fs::remove_file(&socket_path).map_err(|e| {
                Error::Daemon(format!(
                    "Failed to remove stale socket {}: {}",
                    socket_path.display(),
                    e
                ))
            })?;
        }

        let listener = UnixListener::bind(&socket_path).map_err(|e| {
            Error::Daemon(format!(
                "Failed to bind IPC socket at {}: {}",
                socket_path.display(),
                e
            ))
        })?;
        // The control socket accepts `stop` / `update`; keep it private.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600));
        }
        log_info!("IPC server listening on {}", socket_path.display());

        let start_time = Instant::now();
        let pid = std::process::id();

        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, _addr)) => {
                            log_debug!("IPC connection accepted");
                            let this = Arc::clone(&self);
                            tokio::spawn(async move {
                                this.handle_connection(stream, pid, start_time).await;
                            });
                        }
                        Err(e) => {
                            log_error!("IPC accept error: {}", e);
                        }
                    }
                }
                _ = shutdown.wait() => {
                    log_info!("IPC server shutting down");
                    break;
                }
            }
        }

        let _ = std::fs::remove_file(&socket_path);

        Ok(())
    }

    fn handle_connection(
        self: Arc<Self>,
        stream: UnixStream,
        pid: u32,
        start_time: Instant,
    ) -> impl std::future::Future<Output = ()> + Send
    where
        Self: Sync,
    {
        async move {
            let (reader, mut writer) = stream.into_split();
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();

            if buf_reader.read_line(&mut line).await.is_err() {
                return;
            }

            let request: IpcRequest = match serde_json::from_str(&line) {
                Ok(req) => req,
                Err(e) => {
                    send_response(
                        &mut writer,
                        &IpcResponse::Error {
                            message: format!("Invalid request: {}", e),
                        },
                    )
                    .await;
                    return;
                }
            };

            let response = match request {
                IpcRequest::Ping => IpcResponse::Pong,
                IpcRequest::Stop => {
                    self.shutdown.trigger();
                    IpcResponse::Ok
                }
                IpcRequest::Status => {
                    let uptime_secs = start_time.elapsed().as_secs();
                    IpcResponse::Status { pid, uptime_secs }
                }
                IpcRequest::Update => {
                    if let Err(e) = self.update().await {
                        IpcResponse::Error {
                            message: e.to_string(),
                        }
                    } else {
                        IpcResponse::Ok
                    }
                }
            };

            send_response(&mut writer, &response).await;
        }
    }

    /// Rebuild the manifest from the current on-disk state.
    ///
    /// `git publish` commits before calling this, so deriving the changed files
    /// from `git status()` would always yield an empty list.
    pub(crate) async fn update(self: Arc<Self>) -> Result<(), Error> {
        self.rebuild_manifest().await?;
        let manifest = self.manifest.read().await.clone();
        log_info!(
            "Manifest updated ({} files, version {})",
            manifest.files.len(),
            manifest.version_hash
        );
        Ok(())
    }
}
