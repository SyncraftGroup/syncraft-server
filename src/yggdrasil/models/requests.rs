use axum_extra::extract::Multipart;
use serde::Deserialize;

use crate::{
    error::Error,
    yggdrasil::models::{Profile, YggdrasilSkinType},
};

/// `POST /authserver/register` request body.
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    /// Email address used as the account identifier (Mojang legacy naming).
    pub username: String,
    pub password: String,
    /// The character name shown in-game.
    #[serde(rename = "characterName")]
    pub character_name: String,
}

/// `GET /verify` query parameters.
#[derive(Debug, Deserialize)]
pub struct VerifyQuery {
    /// The verification code sent to the user's email.
    pub code: String,
}

/// Agent metadata sent by the client (always `"Minecraft"` / `1` for Minecraft).
#[derive(Debug, Deserialize)]
pub struct Agent {
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub version: i32,
}

/// `POST /authserver/authenticate` request body.
#[derive(Debug, Deserialize)]
pub struct AuthenticateRequest {
    #[allow(dead_code)]
    pub agent: Agent,
    /// Email address used as the account identifier (Mojang legacy naming).
    pub username: String,
    pub password: String,
}

/// `POST /authserver/validate` request body.
#[derive(Debug, Deserialize)]
pub struct ValidateRequest {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    /// Optional; when present the server also checks that it matches the token's stored client token.
    #[serde(rename = "clientToken")]
    pub client_token: Option<String>,
}

/// `POST /authserver/invalidate` request body.
#[derive(Debug, Deserialize)]
pub struct InvalidateRequest {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    /// Ignored by the server — only `accessToken` is used to locate the token.
    #[serde(rename = "clientToken")]
    #[allow(dead_code)]
    pub client_token: Option<String>,
}

/// `POST /authserver/signout` request body.
#[derive(Debug, Deserialize)]
pub struct SignoutRequest {
    /// Email address of the account to sign out.
    pub username: String,
    pub password: String,
}

/// `POST /authserver/refresh` request body.
#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "clientToken")]
    pub client_token: String,
    #[serde(rename = "selectedProfile")]
    pub selected_profile: Option<Profile>,
}

/// `POST /sessionserver/session/minecraft/join` request body.
#[derive(Debug, Deserialize)]
pub struct JoinRequest {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "selectedProfile")]
    pub selected_profile: Option<Profile>,
    #[serde(rename = "serverId")]
    pub server_id: String,
}

/// `GET /sessionserver/session/minecraft/hasJoined` query parameters.
#[derive(Debug, Deserialize)]
pub struct HasJoinedQuery {
    pub username: String,
    #[serde(rename = "serverId")]
    pub server_id: String,
}

/// `GET /sessionserver/session/minecraft/profile/{uuid}` query parameters.
#[derive(Debug, Deserialize)]
pub struct ProfileQuery {
    /// When true the server omits the signature. Ignored here — signatures are always included.
    pub unsigned: Option<bool>,
}

/// `PUT /api/user/profile/{uuid}/{textureType}` — parsed multipart upload.
#[derive(Debug)]
pub struct TextureUpload {
    pub skin_type: YggdrasilSkinType,
    pub data: Vec<u8>,
}

impl TryFrom<TextureUploadBuilder> for TextureUpload {
    type Error = Error;

    fn try_from(builder: TextureUploadBuilder) -> Result<Self, Self::Error> {
        let data = builder
            .image
            .ok_or_else(|| Error::SkinUpload("missing file".into()))?;

        // Reject anything that is not a PNG early: the blob is served back as
        // `image/png` and Minecraft will refuse to load it anyway.
        const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
        if !data.starts_with(PNG_MAGIC) {
            return Err(Error::SkinUpload("texture is not a PNG file".into()));
        }

        Ok(Self {
            skin_type: match builder.model.as_deref() {
                None | Some("") => YggdrasilSkinType::Default,
                Some("slim") => YggdrasilSkinType::Slim,
                Some("default") => YggdrasilSkinType::Default,
                Some(_) => {
                    return Err(Error::SkinUpload("invalid model".into()));
                }
            },
            data,
        })
    }
}

impl TextureUpload {
    pub async fn from_multipart(mul: Multipart) -> Result<Self, Error> {
        TextureUploadBuilder::from_multipart(mul).await?.try_into()
    }
}

#[derive(Debug)]
struct TextureUploadBuilder {
    model: Option<String>,
    image: Option<Vec<u8>>,
}

impl TextureUploadBuilder {
    async fn from_multipart(mut mul: Multipart) -> Result<Self, Error> {
        let mut builder = Self {
            model: None,
            image: None,
        };

        while let Some(field) = mul.next_field().await? {
            match field.name() {
                Some("model") => {
                    builder.model = Some(field.text().await?);
                }
                Some("file") => {
                    builder.image = Some(field.bytes().await?.to_vec());
                }
                _ => (),
            }
        }

        Ok(builder)
    }
}
