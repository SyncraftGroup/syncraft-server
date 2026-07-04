//! Daemon lifecycle management for the syncraft server.
//!
//! # Architecture
//!
//! ```
//! CLI (main.rs)
//!   │
//!   └─ handle_command()          ← dispatch table
//!        │
//!        ├─ manager::detect()    ← pick systemd or fallback
//!        │
//!        ├─ manager.start()      ← platform-specific
//!        ├─ manager.stop()
//!        ├─ manager.status()
//!        ├─ manager.install()
//!        ├─ manager.uninstall()
//!        │
//!        └─ cmd_run()            ← foreground (common to both)
//!             └─ run_server()    ← axum + IPC + signals
//! ```
//!
//! **Key separation:** When systemd manages the process it calls `serve run`
//! directly — the double-fork / PID-file / log-redirection code in
//! [`manager::fallback`] is never touched.  Conversely, the fallback path
//! never touches systemd units.  The two are completely independent at
//! runtime and share only `run_server()`.

mod manager;
pub(crate) mod ipc;

use std::fs;
use std::sync::Arc;

use crate::log_error;
use crate::log_info;

use axum::{middleware, Router};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use crate::config::Config;
use crate::error::Error;
use crate::paths::SyncraftPaths;
use crate::ui::message::MessageKey;
use crate::ui::Ui;
use crate::yggdrasil;

// ── CLI dispatch ─────────────────────────────────────────────────────────

/// Actions that the `serve` command can take.
pub enum ServeAction {
    /// Start the daemon via the platform service manager.
    Start,
    /// Stop a running daemon.
    Stop,
    /// Query daemon status.
    Status,
    /// Run the server in the foreground (dev / debug / systemd ExecStart).
    Run,
    /// Install the server as a platform service.
    Install,
    /// Remove the platform service.
    Uninstall,
    /// Tail daemon logs (systemd journal or log file).
    Logs,
}

/// Entry point called from `main()`. Routes to the appropriate handler
/// using the best available service manager.
///
/// `action` is `None` when no subcommand is given; defaults to `Start`.
pub fn handle_command(
    paths: &SyncraftPaths,
    action: Option<&ServeAction>,
) -> Result<(), Error> {
    let action = action.unwrap_or(&ServeAction::Start);

    match action {
        // ── `run` is manager-independent ────────────────────────
        ServeAction::Run => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;
            rt.block_on(cmd_run(paths))
        }

        // ── Everything else delegates to the service manager ────
        _ => {
            let mgr = manager::detect();
            match action {
                ServeAction::Start => mgr.start(paths),
                ServeAction::Stop => mgr.stop(paths),
                ServeAction::Status => mgr.status(paths),
                ServeAction::Install => mgr.install(paths),
                ServeAction::Uninstall => mgr.uninstall(paths),
                ServeAction::Logs => mgr.logs(paths),
                ServeAction::Run => unreachable!(),
            }
        }
    }
}

// ── Foreground runner (common to systemd and fallback) ───────────────────

/// Run the server in the foreground.
///
/// This is used by:
/// - `syncraft serve run` (manual dev/debug)
/// - systemd `ExecStart=syncraft serve run`
///
/// It contains **no** daemonization logic — the caller is responsible
/// for process lifecycle (terminal, systemd, etc.).
async fn cmd_run(paths: &SyncraftPaths) -> Result<(), Error> {
    let config =
    if let Some(config) = Config::load(paths) { config }
    else {
        Ui::get().error(MessageKey::Uninitialized);
        return Ok(());
    };

    // Enable colour-preserving log output alongside stderr.
    // Under systemd, stderr is captured by journald which strips ANSI
    // codes.  Writing to a file ensures `syncraft serve logs` can tail
    // the raw coloured output.
    let log_file_path = paths.log_file_latest();
    if log_file_path.exists() {
        let _ = fs::rename(log_file_path, paths.log_file());
    }
    crate::log::init_file(&paths.log_file_latest());

    // Write PID so that fallback `stop`/`status` can find the
    // process even when started via `serve run`.
    let pid = std::process::id();
    let _ = fs::write(paths.pid_file(), pid.to_string());
    log_info!("Server starting in foreground (PID {})", pid);

    run_server(paths, config).await
}

// ── Server core ──────────────────────────────────────────────────────────

/// Start the axum HTTP server with an IPC control socket.
///
/// Listens on `0.0.0.0:3000` for HTTP traffic and on a Unix domain
/// socket for control commands.  Graceful shutdown is triggered by:
/// - An IPC `stop` command.
/// - SIGINT / Ctrl+C.
/// - SIGTERM (sent by systemd during `systemctl stop`).
pub(crate) async fn run_server(paths: &SyncraftPaths, config: Config) -> Result<(), Error> {
    let app = Router::new()
        .nest(
            "/yggdrasil",
            yggdrasil::init(paths, config.yggdrasil).await?,
        )
        .layer(middleware::from_fn(crate::log::http_trace));

    let address = format!("{}:{}", config.network.host.trim(), config.network.port);
    let listener = TcpListener::bind(&address).await.map_err(|e| {
        Error::Daemon(format!("Failed to bind HTTP listener: {}", e))
    })?;
    log_info!("HTTP server listening on {address}");

    // A single `Notify` coordinates shutdown across IPC, signals, and the
    // HTTP server.
    let shutdown = Arc::new(Notify::new());

    // ── IPC server ──────────────────────────────────────────────
    let ipc_shutdown = Arc::clone(&shutdown);
    let ipc_socket = paths.socket_path();
    let ipc_handle = tokio::spawn(async move {
        if let Err(e) = ipc::run_server(&ipc_socket, ipc_shutdown).await {
            log_error!("IPC server error: {}", e);
        }
    });

    // ── HTTP server + graceful shutdown ─────────────────────────
    let http_shutdown = Arc::clone(&shutdown);
    let http_handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                http_shutdown.notified().await;
            })
            .await
            .map_err(|e| Error::Daemon(format!("HTTP server error: {}", e)))
    });

    // ── Signal handlers ─────────────────────────────────────────
    let signal_shutdown = Arc::clone(&shutdown);

    // Ctrl+C (SIGINT)
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        log_info!("SIGINT received, shutting down");
        signal_shutdown.notify_waiters();
    });

    // SIGTERM (sent by systemd `systemctl stop`, `kill`, etc.)
    let term_shutdown = Arc::clone(&shutdown);
    tokio::spawn(async move {
        let mut stream = match tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        stream.recv().await;
        log_info!("SIGTERM received, shutting down");
        term_shutdown.notify_waiters();
    });

    // ── Wait ────────────────────────────────────────────────────
    let _ = tokio::join!(ipc_handle, http_handle);

    log_info!("Server shutdown complete");

    // ── Cleanup ─────────────────────────────────────────────────
    let _ = fs::remove_file(paths.socket_path());
    let _ = fs::remove_file(paths.pid_file());

    Ok(())
}
