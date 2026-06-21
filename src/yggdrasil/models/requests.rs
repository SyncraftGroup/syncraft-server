use serde::Deserialize;

use crate::yggdrasil::models::Profile;

/// Agent info sent by the client (always "Minecraft" / 1 for Minecraft)
#[derive(Debug, Deserialize)]
pub struct Agent {
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub version: i32,
}

/// POST /authserver/authenticate request body
#[derive(Debug, Deserialize)]
pub struct AuthenticateRequest {
    #[allow(dead_code)]
    pub agent: Agent,
    /// Email address used as the account identifier (Mojang legacy naming)
    pub username: String,
    pub password: String,
}

/// POST /authserver/register request body (account creation)
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    /// Optional character name; defaults to the email prefix if not provided
    #[serde(rename = "characterName")]
    pub character_name: String,
}

/// POST /authserver/refresh request body
#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "clientToken")]
    pub client_token: String,
    #[serde(rename = "selectedProfile")]
    pub selected_profile: Option<Profile>,
}

/// Query parameter for GET /verify
#[derive(Debug, Deserialize)]
pub struct VerifyQuery {
    pub code: String,
}