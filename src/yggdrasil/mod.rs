pub mod models;
pub mod db;
pub mod routes;
pub mod email;

use axum::{Router, routing::{get, post}};

use crate::{error::Error, paths::SyncraftPaths, yggdrasil::{db::DBManager, email::Mailer}};

#[derive(Clone)]
pub struct YggdrasilState {
    pub db: DBManager,
    pub mailer: Option<Mailer>,
    /// Base URL for constructing verification links,
    /// e.g. "http://localhost:3000/yggdrasil"
    pub base_url: String,
}

pub async fn init(paths: SyncraftPaths) -> Result<Router, Error> {
    paths.create_dirs().await?;
    let db = DBManager::init(&paths).await?;

    // TODO: load email config from a config file
    let state = YggdrasilState {
        db,
        mailer: None,
        base_url: "http://localhost:3000/yggdrasil".into(),
    };

    Ok(router(state))
}

pub fn router(state: YggdrasilState) -> Router {
    Router::new()
        .route("/authenticate", post(routes::authenticate))
        .route("/register", post(routes::register))
        .route("/refresh", post(routes::refresh))
        .route("/verify", get(routes::verify))
        .with_state(state)
}
