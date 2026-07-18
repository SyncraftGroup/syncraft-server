use std::sync::Arc;

use axum::{
    Json, Router, extract::{Path, State}, http::{HeaderMap, HeaderValue, StatusCode, header}, middleware, response::IntoResponse, routing::get,
};
use tokio::net::TcpListener;

use crate::{daemon::SyncraftServer, error::Error, log_info, yggdrasil};

impl SyncraftServer {
    pub async fn run_http(self: Arc<Self>) -> Result<(), Error> {

        let app = Router::new()
            .route("/manifest", get(Self::manifest))
            .route("/download/{hash}", get(Self::download))
            .route("/updateInfo/{hash}", get(Self::update_info))
            .with_state(self.clone())
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

    async fn manifest(
        State(state): State<Arc<Self>>
    ) -> impl IntoResponse {
        let manifest = Arc::clone(&*state.manifest.read().await);
        Json(manifest).into_response()
    }

    async fn download(
        State(state): State<Arc<Self>>,
        Path(hash_str): Path<String>,
    ) -> impl IntoResponse {
        // Parse blake3 hash from hex.  Invalid hex → 400.
        let hash = match hash_str.parse::<blake3::Hash>() {
            Ok(h) => h,
            Err(_) => {
                return (StatusCode::BAD_REQUEST, "Invalid hash").into_response();
            }
        };

        // Resolve hash → relative path.  Unknown hash → 404.
        let relative_path = {
            let guard = state.hash_mapping.lock().await;
            match guard.get(&hash) {
                Some(p) => p.clone(),
                None => {
                    return (StatusCode::NOT_FOUND, "Unknown hash").into_response();
                }
            }
        };

        // Read file from disk.  Missing file → 404.
        let abs_path = state.paths.root().join(&relative_path);
        let data = match tokio::fs::read(&abs_path).await {
            Ok(d) => d,
            Err(_) => {
                return (StatusCode::NOT_FOUND, "File missing on disk").into_response();
            }
        };

        // Build response with filename hint so the client can name it.
        let filename = relative_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");

        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
                .unwrap_or(HeaderValue::from_static("attachment")),
        );

        let code = {
            if let Ok(status) = state.repository.lock().await.status()
            && !status.is_empty() { StatusCode::CONFLICT }
            else { StatusCode::OK }
        };
        (code, headers, data).into_response()
    }

    async fn update_info(
        State(state): State<Arc<Self>>,
        Path(hash): Path<String>
    ) -> impl IntoResponse {
        let updates = {
            if let Ok(updates) = state.repository
            .lock().await.commits_after(&hash) { updates }
            else {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to get commit info"
                ).into_response();
            }
        };
        Json(updates).into_response()
    }
}