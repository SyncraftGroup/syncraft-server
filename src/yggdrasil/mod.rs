pub mod models;
pub mod db;
pub mod routes;
pub mod email;
pub mod sign;

use std::{collections::HashMap, sync::Arc};

use axum::{Router, routing::{get, post, put}};
use rsa::{pkcs1v15::SigningKey, pkcs8::EncodePublicKey};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use tokio::sync::Mutex;

use crate::{error::Error, paths::SyncraftPaths, yggdrasil::{db::DBManager, email::Mailer, models::JoinRequestCache, sign::private_key}};

/// Yggdrasil authentication server configuration (serialisable from the main config).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct YggdrasilConfig {
    server_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<Mailer>,
    base_url: String,
    skin_domains: Vec<String>
}

impl Default for YggdrasilConfig {
    fn default() -> Self {
        Self {
            server_name: None,
            email: None,
            base_url: "http://example.com/yggdrasil".into(),
            skin_domains: vec![
                "example.com".into(),
                "littleskin.cn".into()
            ]
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
    pub sign_key: SigningKey::<Sha1>,
    pub public_pem: String
}

/// Initialise the Yggdrasil subsystem: open the database, load or generate the RSA
/// key-pair, and build the Axum router.
pub async fn init(paths: &SyncraftPaths, config: YggdrasilConfig) -> Result<Router, Error> {
    paths.create_dirs().await?;
    let db = DBManager::init(paths).await?;

    let private_key = private_key(paths).await?;
    let public_pem = private_key
        .to_public_key()
        .to_public_key_pem(rsa::pkcs8::LineEnding::LF)?;

    let base_url = config.base_url.trim_end_matches("/").to_owned();

    let state = YggdrasilState {
        db,
        config,
        base_url: base_url,
        join_requests: Arc::new(Mutex::new(HashMap::new())),
        sign_key: SigningKey::new(private_key),
        public_pem
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
        .route("/sessionserver/session/minecraft/hasJoined", get(routes::has_joined))
        .route("/sessionserver/session/minecraft/profile/{uuid}", get(routes::profile))
        .route("/textures/{hash}", get(routes::texture))
        .route("/api/user/profile/{uuid}/{textureType}", put(routes::upload_texture).delete(routes::delete_skin))
        .with_state(state)
}