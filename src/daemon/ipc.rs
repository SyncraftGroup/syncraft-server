//! IPC over Unix domain sockets using newline-delimited JSON.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use syncraft_protocol::FileState;
use syncraft_protocol::SyncFile;
use syncraft_protocol::SyncMode;
use crate::config::Config;
use crate::daemon::SyncraftServer;
use crate::git::Change;
use crate::git::GitRepository;
use crate::log_debug;
use crate::log_error;
use crate::log_info;
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
    Update
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

async fn send_response(
    writer: &mut (impl AsyncWriteExt + Unpin),
    response: &IpcResponse,
) {
    let mut json = match serde_json::to_string(response) {
        Ok(j) => j,
        Err(e) => {
            format!(r#"{{"type":"error","message":"Failed to serialize response: {}"}}"#, e)
        }
    };
    json.push('\n');
    let _ = writer.write_all(json.as_bytes()).await;
    let _ = writer.shutdown().await;
}

impl SyncraftServer {
    pub async fn run_ipc(self: Arc<Self>) -> Result<(), Error>
    where Self: Send + 'static
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
                _ = shutdown.notified() => {
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
    where Self: Sync
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
                    send_response(&mut writer, &IpcResponse::Error {
                        message: format!("Invalid request: {}", e),
                    }).await;
                    return;
                }
            };

            let response = match request {
                IpcRequest::Ping => IpcResponse::Pong,
                IpcRequest::Stop => {
                    self.shutdown.notify_waiters();
                    IpcResponse::Ok
                }
                IpcRequest::Status => {
                    let uptime_secs = start_time.elapsed().as_secs();
                    IpcResponse::Status { pid, uptime_secs }
                }
                IpcRequest::Update => {
                    if let Err(e) = self.update().await {
                        IpcResponse::Error { message: e.to_string() }
                    } else {
                        IpcResponse::Ok
                    }
                }
            };

            send_response(&mut writer, &response).await;
        }
    }

    async fn update(self: Arc<Self>) -> Result<(), Error> {
        let repository = GitRepository::open(&self.paths.root())?;
        let changes = repository.status()?;

        let mut to_update = Vec::new();

        for change in changes {
            match change {
                Change::Added(path)
                | Change::Modified(path) => {
                    let state = FileState::from_path(&path).await?;
                    let mode = infer_sync_mode(&path, &self.config);
                    to_update.push((
                        path,
                        SyncFile { state: Some(state), mode }
                    ));
                }
                Change::Renamed { from, to } => {
                    to_update.push((
                        from,
                        SyncFile { state: None, mode: SyncMode::Forbidden }
                    ));
                    let state = FileState::from_path(&to).await?;
                    let mode = infer_sync_mode(&to, &self.config);
                    to_update.push((
                        to,
                        SyncFile { state: Some(state), mode }
                    ));
                }
                Change::Deleted(path) => {
                    to_update.push((
                        path,
                        SyncFile { state: None, mode: SyncMode::Forbidden }
                    ));
                }
                _ => ()
            }
        }

        let mut file_mapping = self.file_mapping.lock().await;
        let mut hash_mapping = self.hash_mapping.lock().await;

        for (path, file) in to_update {
            if let Some(state) = &file.state {
                hash_mapping.insert(state.hash.clone(), path.clone());
            }
            file_mapping.insert(path, file);
        }

        Ok(())
    }
}

fn infer_sync_mode(path: &Path, config: &Config) -> SyncMode {
    for object in &config.sync {
        if object.path.starts_with(path) {
            return object.mode.clone();
        }
    }
    SyncMode::Strict
}
