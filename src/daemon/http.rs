use axum::{Router, middleware};
use tokio::net::TcpListener;

use crate::{daemon::SyncraftServer, error::Error, log_info, yggdrasil};

impl SyncraftServer {
    pub async fn run_http(&self) -> Result<(), Error> {

        let app = Router::new()
            .nest("/yggdrasil", yggdrasil::init(&self.paths, &self.config.yggdrasil).await?)
            .layer(middleware::from_fn(crate::log::http_trace));

        let address = format!("{}:{}", self.config.network.host.trim(), self.config.network.port);
        let listener = TcpListener::bind(&address).await.map_err(|e| {
            Error::Daemon(format!("Failed to bind HTTP listener: {}", e))
        })?;
        log_info!("HTTP server listening on {address}");

        let http_shutdown = self.shutdown.clone();

        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                http_shutdown.notified().await;
            })
            .await
            .map_err(|e| Error::Daemon(format!("HTTP server error: {}", e)))?;
        Ok(())
    }
}