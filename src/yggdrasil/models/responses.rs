use serde::Serialize;

use crate::yggdrasil::models::Profile;

/// POST /authserver/authenticate response body
#[derive(Debug, Serialize)]
pub struct AuthenticateResponse {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "clientToken")]
    pub client_token: String,
    #[serde(rename = "availableProfiles")]
    pub available_profiles: Vec<Profile>,
    #[serde(rename = "selectedProfile")]
    pub selected_profile: Option<Profile>,
}

/// POST /authserver/register response body
#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub id: String,
    pub email: String
}

/// POST /authserver/refresh response body
#[derive(Debug, Serialize)]
pub struct RefreshResponse {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "clientToken")]
    pub client_token: String,
    #[serde(rename = "selectedProfile")]
    pub selected_profile: Option<Profile>,
}
