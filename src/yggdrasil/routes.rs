use crate::error::Error;
use crate::yggdrasil::email::verify_email_string;
use crate::yggdrasil::models::{
    Profile, TokenStatus, YggdrasilError,
    requests::{
        AuthenticateRequest, InvalidateRequest, RefreshRequest, RegisterRequest, SignoutRequest,
        ValidateRequest, VerifyQuery,
    },
    responses::{AuthenticateResponse, RefreshResponse, RegisterResponse},
};
use crate::yggdrasil::{
    YggdrasilState,
    db::unsigned_uuid,
    models::{
        JoinRequestCache,
        requests::{HasJoinedQuery, JoinRequest, ProfileQuery, TextureUpload},
        responses::{GameProfile, MetaResponse, YggdrasilMeta},
    },
};
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::{
    TypedHeader,
    extract::Multipart,
    headers::{Authorization, authorization::Bearer},
};
use chrono::{Duration, Utc};

fn error_response(status: StatusCode, error: &str, error_message: &str) -> Response {
    (
        status,
        Json(YggdrasilError {
            error: error.into(),
            error_message: error_message.into(),
        }),
    )
        .into_response()
}

fn verify_password(password: &str, hash: &str) -> Result<bool, Error> {
    let parsed_hash = PasswordHash::new(hash).map_err(Error::PasswordHash)?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

/// `GET /` — returns server metadata, skin domains, and the public signing key.
pub async fn meta(State(state): State<YggdrasilState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(MetaResponse {
            meta: YggdrasilMeta {
                server_name: state
                    .config
                    .server_name
                    .unwrap_or("syncraft-yggdrasil".into()),
                implementation_name: "syncraft-yggdrasil".into(),
                implementation_version: "0.0.0".into(),
            },
            skin_domains: state.config.skin_domains,
            signature_publickey: state.public_pem,
        }),
    )
}

/// `POST /authserver/register`
///
/// Creates a new account with a character profile.
/// If an email config is present, also sends a verification email.
pub async fn register(
    State(state): State<YggdrasilState>,
    Json(body): Json<RegisterRequest>,
) -> impl IntoResponse {
    let (user, code) = match state
        .db
        .create_user(&body.username, &body.password, &body.character_name)
        .await
    {
        Ok(result) => result,
        Err(Error::Database(e)) if e.to_string().contains("UNIQUE") => {
            return error_response(
                StatusCode::CONFLICT,
                "UserAlreadyExists",
                "An account with this email address already exists.",
            )
            .into_response();
        }
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                "Failed to create account.",
            )
            .into_response();
        }
    };

    if let Some(mailer) = &state.config.email
        && mailer
            .send_email(
                &user.email,
                verify_email_string(format!("{}/verify?code={}", state.base_url, code)),
            )
            .await
            .is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "Failed to send verify email.",
        );
    }

    (
        StatusCode::CREATED,
        Json(RegisterResponse {
            id: user.id,
            email: user.email,
        }),
    )
        .into_response()
}

/// `GET /verify?code=xxx`
///
/// Verifies a user's email address using the code sent during registration.
pub async fn verify(
    State(state): State<YggdrasilState>,
    Query(query): Query<VerifyQuery>,
) -> impl IntoResponse {
    match state.db.verify_email(&query.code).await {
        Ok(Some(_user_id)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "message": "Email verified successfully."
            })),
        )
            .into_response(),
        Ok(None) => error_response(
            StatusCode::BAD_REQUEST,
            "InvalidVerificationCode",
            "The verification code is invalid or has already been used.",
        )
        .into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "Failed to verify email.",
        )
        .into_response(),
    }
}

/// `POST /authserver/authenticate`
///
/// Authenticates a user by email + password.
/// On success returns a fresh `accessToken` / `clientToken` pair and the user's profiles.
pub async fn authenticate(
    State(state): State<YggdrasilState>,
    Json(body): Json<AuthenticateRequest>,
) -> impl IntoResponse {
    // Look up user by email (the 'username' field holds the email address)
    let user = match state.db.find_user_by_email(&body.username).await {
        Ok(Some(user)) => user,
        _ => {
            return error_response(
                StatusCode::FORBIDDEN,
                "ForbiddenOperationException",
                "Invalid credentials. Invalid username or password.",
            );
        }
    };

    // Check email verification (only if email config is present)
    if state.config.email.is_some() && !user.email_verified {
        return error_response(
            StatusCode::FORBIDDEN,
            "ForbiddenOperationException",
            "Email not verified. Please check your inbox.",
        );
    }

    // Verify password
    if !matches!(
        verify_password(&body.password, &user.password_hash),
        Ok(true)
    ) {
        return error_response(
            StatusCode::FORBIDDEN,
            "ForbiddenOperationException",
            "Invalid credentials. Invalid username or password.",
        );
    };

    // Build the single profile (one-to-one: each user is their own character)
    let profile = Profile {
        id: user.id.clone(),
        name: user.character_name.clone(),
    };

    // Create new tokens
    let user_id = user.id.clone();
    if let Ok((access_token, client_token)) = state.db.create_tokens(&user_id).await {
        (
            StatusCode::OK,
            Json(AuthenticateResponse {
                access_token,
                client_token,
                available_profiles: vec![profile.clone()],
                selected_profile: Some(profile),
            }),
        )
            .into_response()
    } else {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "Failed to create authentication tokens.",
        )
    }
}

/// `POST /authserver/validate`
///
/// Checks whether an `accessToken` (and optionally `clientToken`) is valid.
/// Returns `204 No Content` on success, or a `403` error if the token is invalid.
pub async fn validate(
    State(state): State<YggdrasilState>,
    Json(body): Json<ValidateRequest>,
) -> impl IntoResponse {
    let token_info =
    // Look up the token
    if let Ok(Some(info)) = state.db.get_token(&body.access_token).await
    // Token must not be Invalid
    && !info.status.eq(&TokenStatus::Invalid) { info }
    else {
        return error_response(
            StatusCode::FORBIDDEN,
            "ForbiddenOperationException",
            "Invalid token."
        );
    };

    // If clientToken is provided, verify it matches
    if let Some(ref client_token) = body.client_token
        && token_info.client_token != *client_token
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "ForbiddenOperationException",
            "Invalid token.",
        );
    }

    StatusCode::NO_CONTENT.into_response()
}

/// `POST /authserver/invalidate`
///
/// Revokes a given `accessToken`. The `clientToken` is ignored.
/// Always returns `204 No Content` regardless of outcome.
pub async fn invalidate(
    State(state): State<YggdrasilState>,
    Json(body): Json<InvalidateRequest>,
) -> impl IntoResponse {
    // Best-effort deletion — ignore errors
    let _ = state.db.delete_token(&body.access_token).await;

    StatusCode::NO_CONTENT.into_response()
}

/// `POST /authserver/signout`
///
/// Revokes every token belonging to a user, identified by email + password.
/// Returns `204 No Content` on success.
pub async fn signout(
    State(state): State<YggdrasilState>,
    Json(body): Json<SignoutRequest>,
) -> impl IntoResponse {
    // Look up user by email
    let user = if let Ok(Some(user)) = state.db.find_user_by_email(&body.username).await {
        user
    } else {
        return error_response(
            StatusCode::FORBIDDEN,
            "ForbiddenOperationException",
            "Invalid credentials. Invalid username or password.",
        );
    };

    // Verify password
    if !matches!(
        verify_password(&body.password, &user.password_hash),
        Ok(true)
    ) {
        return error_response(
            StatusCode::FORBIDDEN,
            "ForbiddenOperationException",
            "Invalid credentials. Invalid username or password.",
        );
    };

    // Delete all tokens for this user — best-effort
    let _ = state.db.delete_user_tokens(&user.id).await;

    StatusCode::NO_CONTENT.into_response()
}

/// `POST /authserver/refresh`
///
/// Refreshes an `accessToken`. The token must not be `Invalid`.
/// The `clientToken` must match, and `selectedProfile` (if provided) must belong
/// to the token's owner.
pub async fn refresh(
    State(state): State<YggdrasilState>,
    Json(body): Json<RefreshRequest>,
) -> impl IntoResponse {
    let token_info =
    // Look up the token
    if let Ok(Some(info)) = state.db.get_token(&body.access_token).await
    // Status check
    && !info.status.eq(&TokenStatus::Invalid)
    // Verify clientToken matches
    && info.client_token == body.client_token { info }
    else {
        return error_response(
            StatusCode::FORBIDDEN,
            "ForbiddenOperationException",
            "Invalid token."
        );
    };

    // With 1-to-1 binding, if selectedProfile is provided, its id must match the user_id
    if let Some(ref profile) = body.selected_profile
        && profile.id != token_info.user_id
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "ForbiddenOperationException",
            "Invalid token.",
        );
    }

    // Create a new accessToken with the same clientToken
    if let Ok(access_token) = state.db.refresh_token(&token_info.client_token, &token_info.user_id).await
    // Look up the profile from the users table
    && let Ok(Some(profile)) = state.db.get_profile_for_user(&token_info.user_id).await
    {
        (
            StatusCode::OK,
            Json(RefreshResponse {
                access_token,
                client_token: token_info.client_token,
                selected_profile: Some(profile),
            }),
        )
            .into_response()
    } else {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "Failed to create new token.",
        )
    }
}

/// `POST /sessionserver/session/minecraft/join`
///
/// Called by the client to join a target server. The join request is cached for
/// 5 minutes so the server can verify it via `hasJoined`.
pub async fn join(
    State(state): State<YggdrasilState>,
    Json(body): Json<JoinRequest>,
) -> impl IntoResponse {
    let token_info =
    // Look up the token
    if let Ok(Some(info)) = state.db.get_token(&body.access_token).await
    // Status check
    && info.status.eq(&TokenStatus::Active) { info }
    else {
        return error_response(
            StatusCode::FORBIDDEN,
            "ForbiddenOperationException",
            "Invalid token."
        );
    };

    // With 1-to-1 binding, the selectedProfile's id must match the user_id
    if let Some(ref profile) = body.selected_profile
        && profile.id != token_info.user_id
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "IllegalArgumentException",
            "Access token already has a profile assigned.",
        );
    }

    if let Ok(Some(profile)) = state.db.get_profile_for_user(&token_info.user_id).await {
        {
            let mut guard = state.join_requests.lock().await;
            guard.insert(
                body.server_id,
                JoinRequestCache {
                    user_id: token_info.user_id,
                    character_name: profile.name,
                    timestamp: chrono::Utc::now(),
                },
            );
        }
        // Success
        StatusCode::NO_CONTENT.into_response()
    } else {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "Failed to get profile.",
        )
    }
}

/// `GET /sessionserver/session/minecraft/hasJoined`
///
/// Called by a game server to verify that a joining player has a valid session.
pub async fn has_joined(
    State(state): State<YggdrasilState>,
    Query(query): Query<HasJoinedQuery>,
) -> impl IntoResponse {
    let (profile, is_fake) =
    // Get cached join request
    if let Some(req) = {
        let mut guard = state.join_requests.lock().await;
        guard.remove(&query.server_id)
    }
    // Ensure the cached request hasn't expired (5-minute window)
    && Utc::now().signed_duration_since(req.timestamp) <= Duration::minutes(5)
    // Look up profile
    && let Some((profile, is_fake)) =
        if let Ok(Some(profile)) = state.db.get_profile_for_user(&req.user_id).await {
            // Real account: prefer the skin uploaded to this server.
            Some((profile, false))
        } else if state.config.identity_falsification {
            // make up fake identity
            Some((Profile {
                id: unsigned_uuid(),
                name: query.username.clone()
            }, true))
        } else { None }
    // Check if name matches
    && profile.name == req.character_name
    && profile.name == query.username { (profile, is_fake) }
    else {
        return StatusCode::NO_CONTENT.into_response();
    };

    (
        StatusCode::OK,
        Json(GameProfile::generate(state, profile, is_fake).await),
    )
        .into_response()
}

/// `GET /textures/{hash}`
///
/// Returns the raw skin PNG for the given texture hash.
pub async fn texture(
    State(state): State<YggdrasilState>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    if let Ok(Some(data)) = state.db.get_skin_data(&hash).await {
        ([(axum::http::header::CONTENT_TYPE, "image/png")], data).into_response()
    } else {
        (StatusCode::NOT_FOUND, "Not Found").into_response()
    }
}

/// `PUT /api/user/profile/{uuid}/{textureType}`
///
/// Sets a user's skin (or cape?) from a multipart upload.
pub async fn upload_texture(
    State(state): State<YggdrasilState>,
    Path((id, skin_type)): Path<(String, String)>,
    TypedHeader(auth): TypedHeader<Authorization<Bearer>>,
    multipart: Multipart,
) -> impl IntoResponse {
    // Only skin is supported for now.
    if skin_type != "skin" {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let access_token = auth.token();

    let token_info =
    // Look up token
    if let Ok(Some(info)) = state.db.get_token(access_token).await
    && info.status.eq(&TokenStatus::Active)
    // Check user identity
    && info.user_id == id { info }
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let upload = if let Ok(upload) = TextureUpload::from_multipart(multipart).await {
        upload
    } else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    if state
        .db
        .set_skin(&token_info.user_id, upload.data, upload.skin_type)
        .await
        .is_ok()
    {
        StatusCode::NO_CONTENT.into_response()
    } else {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "Failed to set the skin.",
        )
    }
}

/// `DELETE /api/user/profile/{uuid}/{textureType}`
///
/// Deletes a user's skin (or cape?).
pub async fn delete_skin(
    State(state): State<YggdrasilState>,
    Path((id, skin_type)): Path<(String, String)>,
    TypedHeader(auth): TypedHeader<Authorization<Bearer>>,
) -> impl IntoResponse {
    // Only skin is supported for now.
    if skin_type != "skin" {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let access_token = auth.token();

    let token_info =
    // Look up token
    if let Ok(Some(info)) = state.db.get_token(access_token).await
    && info.status.eq(&TokenStatus::Active)
    // Check user identity
    && info.user_id == id { info }
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    if state.db.remove_skin(&token_info.user_id).await.is_ok() {
        StatusCode::NO_CONTENT.into_response()
    } else {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "Failed to remove the skin.",
        )
    }
}

/// `GET /sessionserver/session/minecraft/profile/{uuid}`
///
/// Returns the game profile (with textures property) for the given user UUID.
/// The `unsigned` query parameter is accepted but ignored — signatures are always included.
pub async fn profile(
    State(state): State<YggdrasilState>,
    Path(id): Path<String>,
    Query(query): Query<ProfileQuery>,
) -> impl IntoResponse {
    #[allow(unused_variables)]
    let unsigned = query.unsigned.unwrap_or(false);

    if let Ok(Some(profile)) = state.db.get_profile_for_user(&id).await {
        (
            StatusCode::OK,
            Json(GameProfile::generate(state, profile, false).await),
        )
            .into_response()
    } else {
        StatusCode::NO_CONTENT.into_response()
    }
}
