pub mod db;
pub mod email;
pub mod models;
pub mod routes;
pub mod sign;

#[cfg(test)]
mod tests;

use std::{collections::HashMap, sync::Arc};

use crate::log_debug;
use crate::log_info;
use axum::{
    Router,
    routing::{get, post, put},
};
use rsa::{pkcs1v15::SigningKey, pkcs8::EncodePublicKey};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use tokio::sync::Mutex;

use crate::{
    error::Error,
    paths::SyncraftPaths,
    yggdrasil::{db::DBManager, email::Mailer, models::JoinRequestCache, sign::private_key},
};

/// Yggdrasil authentication server configuration (serialisable from the main config).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct YggdrasilConfig {
    pub server_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<Mailer>,
    pub base_url: String,
    pub skin_domains: Vec<String>,
    pub identity_falsification: bool,
}

/// Placeholder written by `syncraft init`; replaced once the operator configures
/// a real public URL. Until then the network bind address is used instead.
pub const DEFAULT_BASE_URL: &str = "http://example.com/yggdrasil";

impl Default for YggdrasilConfig {
    fn default() -> Self {
        Self {
            server_name: None,
            email: None,
            base_url: DEFAULT_BASE_URL.into(),
            skin_domains: vec!["example.com".into(), "littleskin.cn".into()],
            identity_falsification: false,
        }
    }
}

/// Shared application state passed to every Yggdrasil route handler via Axum.
#[derive(Clone)]
pub struct YggdrasilState {
    pub db: DBManager,
    pub config: YggdrasilConfig,
    /// Base URL for constructing verification links,
    /// e.g. "http://localhost:3000/yggdrasil"
    pub base_url: String,
    /// HashMap<server_id, character_name>
    pub join_requests: Arc<Mutex<HashMap<String, JoinRequestCache>>>,
    pub sign_key: SigningKey<Sha1>,
    pub public_pem: String,
}

/// Initialise the Yggdrasil subsystem: open the database, load or generate the RSA
/// key-pair, and build the Axum router.
pub async fn init(paths: &SyncraftPaths, config: &YggdrasilConfig) -> Result<Router, Error> {
    log_info!("Initialising Yggdrasil auth subsystem");
    paths.create_dirs()?;
    let db = DBManager::init(paths).await?;

    let private_key = private_key(paths).await?;
    log_debug!("RSA key-pair loaded");
    let public_pem = private_key
        .to_public_key()
        .to_public_key_pem(rsa::pkcs8::LineEnding::LF)?;

    let base_url = config.base_url.trim_end_matches("/").to_owned();
    log_info!("Yggdrasil base URL: {base_url}");

    let state = YggdrasilState {
        db,
        config: config.clone(),
        base_url,
        join_requests: Arc::new(Mutex::new(HashMap::new())),
        sign_key: SigningKey::new(private_key),
        public_pem,
    };

    Ok(router(state))
}

pub fn router(state: YggdrasilState) -> Router {
    Router::new()
        .route("/", get(routes::meta))
        .route("/register", post(routes::register))
        .route("/verify", get(routes::verify))
        .route("/authserver/authenticate", post(routes::authenticate))
        .route("/authserver/validate", post(routes::validate))
        .route("/authserver/invalidate", post(routes::invalidate))
        .route("/authserver/signout", post(routes::signout))
        .route("/authserver/refresh", post(routes::refresh))
        .route("/sessionserver/session/minecraft/join", post(routes::join))
        .route(
            "/sessionserver/session/minecraft/hasJoined",
            get(routes::has_joined),
        )
        .route(
            "/sessionserver/session/minecraft/profile/{uuid}",
            get(routes::profile),
        )
        .route("/textures/{hash}", get(routes::texture))
        .route(
            "/api/user/profile/{uuid}/{textureType}",
            put(routes::upload_texture).delete(routes::delete_skin),
        )
        .with_state(state)
}
