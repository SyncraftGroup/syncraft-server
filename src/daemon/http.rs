use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::get,
};
use axum_server::Handle;
use axum_server::tls_rustls::RustlsConfig;
use serde::Deserialize;
use tokio::net::TcpListener;

use crate::{daemon::SyncraftServer, daemon::signing, error::Error, log_info, log_warn, yggdrasil};

#[derive(Debug, Deserialize)]
struct ManifestQuery {
    /// Serve a specific snapshot instead of the current one (rollback/pinning).
    version: Option<String>,
}

impl SyncraftServer {
    pub async fn run_http(self: Arc<Self>) -> Result<(), Error> {
        let network = self.config.read().await.network.clone();
        let yggdrasil_config = self.config.read().await.yggdrasil.clone();

        let app = Router::new()
            .route("/manifest", get(Self::manifest))
            .route("/versions", get(Self::versions))
            .route("/download/{hash}", get(Self::download))
            .route("/updateInfo/{hash}", get(Self::update_info))
            .with_state(self.clone())
            .nest(
                "/yggdrasil",
                yggdrasil::init(&self.paths, &yggdrasil_config).await?,
            )
            .layer(middleware::from_fn(crate::log::http_trace));

        let address = format!("{}:{}", network.host.trim(), network.port);

        let http_shutdown = self.shutdown.clone();

        match network.tls.clone() {
            Some(tls) => {
                let rustls = RustlsConfig::from_pem_file(&tls.cert, &tls.key)
                    .await
                    .map_err(|e| {
                        Error::Daemon(format!(
                            "Failed to load TLS certificate/key ({} / {}): {e}",
                            tls.cert.display(),
                            tls.key.display()
                        ))
                    })?;

                let socket_addr = tokio::net::lookup_host(&address)
                    .await
                    .map_err(|e| Error::Daemon(format!("Failed to resolve {address}: {e}")))?
                    .next()
                    .ok_or_else(|| Error::Daemon(format!("No address resolved for {address}")))?;

                let handle = Handle::new();
                let shutdown_handle = handle.clone();
                tokio::spawn(async move {
                    http_shutdown.wait().await;
                    shutdown_handle.graceful_shutdown(Some(Duration::from_secs(10)));
                });

                log_info!("HTTPS server listening on {socket_addr}");
                axum_server::bind_rustls(socket_addr, rustls)
                    .handle(handle)
                    .serve(app.into_make_service())
                    .await
                    .map_err(|e| Error::Daemon(format!("HTTPS server error: {e}")))?;
            }
            None => {
                let listener = TcpListener::bind(&address)
                    .await
                    .map_err(|e| Error::Daemon(format!("Failed to bind HTTP listener: {e}")))?;
                log_info!("HTTP server listening on {address}");

                axum::serve(listener, app)
                    .with_graceful_shutdown(async move {
                        http_shutdown.wait().await;
                    })
                    .await
                    .map_err(|e| Error::Daemon(format!("HTTP server error: {e}")))?;
            }
        }

        Ok(())
    }

    /// Current manifest, or a stored snapshot via `?version=<hash>`.
    ///
    /// Supports `If-None-Match` so the (potentially large) manifest is not
    /// re-downloaded when the client already has this version.
    async fn manifest(
        State(state): State<Arc<Self>>,
        Query(query): Query<ManifestQuery>,
        headers: HeaderMap,
    ) -> Response {
        let manifest = match query.version {
            Some(hash) => match state.store.load(&hash) {
                Ok(Some(manifest)) => Arc::new(resign_if_needed(&state, manifest).await),
                Ok(None) => {
                    return (StatusCode::NOT_FOUND, "Unknown version").into_response();
                }
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                }
            },
            None => Arc::clone(&*state.manifest.read().await),
        };

        let etag = format!("\"{}\"", manifest.version_hash);
        let not_modified = headers
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(',')
                    .any(|candidate| candidate.trim() == etag || candidate.trim() == "*")
            });
        if not_modified {
            return StatusCode::NOT_MODIFIED.into_response();
        }

        let mut response = Json(manifest).into_response();
        if let Ok(value) = HeaderValue::from_str(&etag) {
            response.headers_mut().insert(header::ETAG, value);
        }
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        response
    }

    /// Stored manifest snapshots, newest first.
    async fn versions(State(state): State<Arc<Self>>) -> impl IntoResponse {
        Json(state.store.list())
    }

    async fn download(
        State(state): State<Arc<Self>>,
        Path(hash_str): Path<String>,
    ) -> impl IntoResponse {
        // Parse blake3 hash from hex.  Invalid hex → 400.
        let hash = match hash_str.parse::<blake3::Hash>() {
            Ok(hash) => hash,
            Err(_) => {
                return (StatusCode::BAD_REQUEST, "Invalid hash").into_response();
            }
        };

        // Resolve hash → the files on this server that provide it.  Unknown hash
        // → 404.
        let candidates = {
            let guard = state.hash_mapping.lock().await;
            match guard.get(&hash) {
                Some(paths) => paths.clone(),
                None => {
                    return (StatusCode::NOT_FOUND, "Unknown hash").into_response();
                }
            }
        };

        // The manifest is built from disk while files keep changing; refuse to
        // serve content that does not match the advertised hash, and fall back
        // to another copy with identical content when one has changed.
        let mut found = None;
        for candidate in &candidates {
            if let Ok(data) = tokio::fs::read(candidate).await
                && blake3::hash(&data) == hash
            {
                found = Some((candidate.clone(), data));
                break;
            }
        }

        let Some((source, data)) = found else {
            return (
                StatusCode::CONFLICT,
                "File changed on disk; the manifest is being updated",
            )
                .into_response();
        };

        // Build response with filename hint so the client can name it.
        let filename = source
            .file_name()
            .and_then(|name| name.to_str())
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

        (StatusCode::OK, headers, data).into_response()
    }

    async fn update_info(
        State(state): State<Arc<Self>>,
        Path(hash): Path<String>,
    ) -> impl IntoResponse {
        let commits = {
            let repository = state.repository.lock().await;
            match repository.commits_after(&hash) {
                Ok(Some(commits)) => commits,
                Ok(None) => {
                    return (StatusCode::NOT_FOUND, "Unknown commit").into_response();
                }
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                }
            }
        };
        Json(commits).into_response()
    }
}

/// Sign a loaded snapshot when signing was enabled after it was recorded.
async fn resign_if_needed(
    state: &SyncraftServer,
    mut manifest: syncraft_protocol::Manifest,
) -> syncraft_protocol::Manifest {
    if manifest.signature.is_none()
        && state.config.read().await.network.sign_manifest
        && let Err(e) = signing::sign_if_enabled(&state.paths, &mut manifest, true)
    {
        log_warn!("Failed to sign snapshot {}: {e}", manifest.version_hash);
    }
    manifest
}
