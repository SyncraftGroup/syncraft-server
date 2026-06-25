use base64::Engine;
use chrono::Utc;
use serde::Serialize;

use crate::yggdrasil::{YggdrasilState, models::{Profile, YggdrasilSkinType}, sign::sign};

/// `POST /authserver/register` response body.
#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub id: String,
    pub email: String
}

/// `POST /authserver/authenticate` response body.
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

/// `POST /authserver/refresh` response body.
#[derive(Debug, Serialize)]
pub struct RefreshResponse {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "clientToken")]
    pub client_token: String,
    #[serde(rename = "selectedProfile")]
    pub selected_profile: Option<Profile>,
}

#[derive(Debug, Serialize)]
pub struct GameProfile {
    pub id: String,
    pub name: String,
    pub properties: Vec<Property>
}

impl GameProfile {
    pub async fn generate(state: YggdrasilState, profile: Profile) -> Self {
        let (skin_url, skin_type) = 
        if let Ok(Some((hash, skin_type))) = state.db.get_user_skin_meta(&profile.id).await {
            (format!("{}/textures/{}", state.base_url, hash), skin_type)
        } else if let Ok((skin_type, url)) = mc_skin::get_skin_url(&profile.name, mc_skin::SkinSource::Mojang).await {
            (url, YggdrasilSkinType::from(skin_type))
        } else if let Ok((skin_type, url)) = mc_skin::get_skin_url(&profile.name, mc_skin::SkinSource::LittleSkin).await {
            (url, YggdrasilSkinType::from(skin_type))
        } else {
            return Self {
                id: profile.id,
                name: profile.name,
                properties: vec![]
            };
        };

        let textures = serde_json::json!({
            "timestamp": Utc::now().timestamp_millis(),
            "profileId": profile.id,
            "profileName": profile.name,
            "textures": {
                "SKIN": {
                    "url": skin_url,
                    "metadata": {
                        "model": &skin_type.to_string()
                    }
                }
            }
        });
        let textures_json = textures.to_string();
        let value = base64::engine::general_purpose::STANDARD.encode(textures_json);
        let signature = sign(&state.sign_key, value.as_bytes());

        Self {
            id: profile.id.clone(),
            name: profile.name.clone(),
            properties: vec![
                Property {
                    name: "textures".into(),
                    value,
                    signature
                }
            ]
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Property {
    pub name: String,
    /// Base64-encoded JSON texture payload.
    pub value: String,
    /// Base64-encoded RSA signature of `value`.
    pub signature: String
}

/// `GET /` root metadata response.
#[derive(Debug, Serialize)]
pub struct MetaResponse {
    pub meta: YggdrasilMeta,
    #[serde(rename = "skinDomains")]
    pub skin_domains: Vec<String>,
    #[serde(rename = "signaturePublickey")]
    pub signature_publickey: String
}

#[derive(Debug, Serialize)]
pub struct YggdrasilMeta {
    #[serde(rename = "serverName")]
    pub server_name: String,
    #[serde(rename = "implementationName")]
    pub implementation_name: String,
    #[serde(rename = "implementationVersion")]
    pub implementation_version: String
}