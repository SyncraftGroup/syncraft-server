use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::{Json, extract::{Query, State}, http::StatusCode, response::{IntoResponse, Response}};
use crate::yggdrasil::YggdrasilState;
use crate::yggdrasil::email::verify_email_string;
use crate::yggdrasil::models::{Profile, TokenStatus, YggdrasilError, requests::{AuthenticateRequest, RefreshRequest, RegisterRequest, VerifyQuery}, responses::{AuthenticateResponse, RefreshResponse, RegisterResponse}};
use crate::error::Error;

fn error_response(
    status: StatusCode,
    error: &str,
    error_message: &str,
) -> Response {
    (
        status,
        Json(YggdrasilError {
            error: error.into(),
            error_message: error_message.into(),
        }),
    ).into_response()
}

fn verify_password(password: &str, hash: &str) -> Result<bool, Error> {
    let parsed_hash = PasswordHash::new(hash).map_err(Error::PasswordHash)?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

/// POST /authserver/authenticate
///
/// Authenticates a user by email + password.
/// On success returns new accessToken / clientToken and available profiles.
pub async fn authenticate(
    State(state): State<YggdrasilState>,
    Json(body): Json<AuthenticateRequest>,
) -> impl IntoResponse {

    // Look up user by email (username does be email)
    let user = match state.db.find_user_by_email(&body.username).await {
        Ok(Some(user)) => user,
        _ => {
            return error_response(
                StatusCode::FORBIDDEN,
                "ForbiddenOperationException",
                "Invalid credentials. Invalid username or password."
            )
        }
    };

    // Check email verification (only if email config is present)
    if state.mailer.is_some() && !user.email_verified {
        return error_response(
            StatusCode::FORBIDDEN,
            "ForbiddenOperationException",
            "Email not verified. Please check your inbox."
        )
    }

    // Verify password
    if !matches!(
        verify_password(&body.password, &user.password_hash),
        Ok(true)
    ) {
        return error_response(
            StatusCode::FORBIDDEN,
            "ForbiddenOperationException",
            "Invalid credentials. Invalid username or password."
        )
    };

    // Build the single profile (1-to-1: each user is their own character)
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
                selected_profile: Some(profile)
            })
        ).into_response()
    } else {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "Failed to create authentication tokens.",
        )
    }
}

/// POST /authserver/register
///
/// Creates a new account with a default character profile.
/// If email config is present, sends a verification email.
pub async fn register(
    State(state): State<YggdrasilState>,
    Json(body): Json<RegisterRequest>,
) -> impl IntoResponse {
    let (user, code) = match state.db.create_user(
        &body.username,
        &body.password,
        &body.character_name
    ).await {
        Ok(result) => result,
        Err(Error::Database(e)) if e.to_string().contains("UNIQUE") => {
            return error_response(
                StatusCode::CONFLICT,
                "UserAlreadyExists",
                "An account with this email address already exists.",
            )
            .into_response()
        }
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                "Failed to create account.",
            )
            .into_response()
        }
    };

    if let Some(mailer) = &state.mailer
        && mailer.send_email(
        &user.email,
        verify_email_string(format!("{}/verify?code={}", state.base_url, code))
        ).await.is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "Failed to send verify email."
        );
    }

    (
        StatusCode::CREATED,
        Json(RegisterResponse {
            id: user.id,
            email: user.email
        })
    ).into_response()
}

/// POST /authserver/refresh
///
/// Refreshes an accessToken. The token must not be Invalid.
/// The clientToken must match,
/// and selectedProfile (if provided) must belong to the token's user.
pub async fn refresh(
    State(state): State<YggdrasilState>,
    Json(body): Json<RefreshRequest>,
) -> impl IntoResponse {
    // Look up the token
    
    let token_info =
    if let Ok(Some(info)) = state.db.get_token(&body.access_token).await
    // Status check
    && !info.status.eq(&TokenStatus::Invalid)
    // Verify clientToken matches
    && info.client_token == body.client_token { info } else {
        return error_response(
            StatusCode::FORBIDDEN,
            "ForbiddenOperationException",
            "Invalid token."
        );
    };

    // With 1-to-1 binding, if selectedProfile is provided, its id must match the user_id
    if let Some(ref profile) = body.selected_profile 
    && profile.id != token_info.user_id {
        return error_response(
            StatusCode::FORBIDDEN,
            "ForbiddenOperationException",
            "Invalid token."
        );
    }

    // Create a new accessToken with the same clientToken
    if let Ok(access_token) = state.db.refresh_token(&token_info.client_token, &token_info.user_id).await
    // Look up the profile from the users table
    && let Ok(Some(profile)) = state.db.get_profile_for_user(&token_info.user_id).await {
        (
            StatusCode::OK,
            Json(RefreshResponse {
                access_token,
                client_token: token_info.client_token,
                selected_profile: Some(profile)
            })
        ).into_response()
    } else {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "Failed to create new token."
        )
    }
}

/// GET /verify?code=xxx
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
        ).into_response(),
        Ok(None) => error_response(
            StatusCode::BAD_REQUEST,
            "InvalidVerificationCode",
            "The verification code is invalid or has already been used.",
        ).into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "Failed to verify email.",
        ).into_response(),
    }
}
