use argon2::{
    Argon2, PasswordHasher,
    password_hash::SaltString,
};
use chrono::{DateTime, Utc};
use rand_core::OsRng;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{error::Error, paths::SyncraftPaths, yggdrasil::models::{Profile, TokenInfo, TokenStatus, User}};

#[derive(Debug, Clone)]
pub struct DBManager {
    pub pool: SqlitePool,
}

impl DBManager {
    pub async fn init(paths: &SyncraftPaths) -> Result<Self, Error> {
        let db_path = paths.yggdrasil().db_path();
        let db_url = format!("sqlite:{}", db_path.display());
        let pool = SqlitePool::connect(&db_url).await?;
        Self::init_db(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn init_db(pool: &SqlitePool) -> Result<(), Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS users (
                id                  TEXT PRIMARY KEY,
                email               TEXT UNIQUE NOT NULL,
                password_hash       TEXT NOT NULL,
                email_verified      INTEGER NOT NULL DEFAULT 0,
                verification_code   TEXT,
                character_name      TEXT NOT NULL,
                created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            )"
        ).execute(pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS access_tokens (
                access_token    TEXT PRIMARY KEY,
                client_token    TEXT NOT NULL,
                user_id         TEXT NOT NULL,
                created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                FOREIGN KEY (user_id) REFERENCES users(id)
            )"
        ).execute(pool).await?;

        Ok(())
    }

    /// Create a new user using info provided with register API.
    /// Returns `(User, verify_code)` where `verify_code` as unsigned UUID strings.
    pub async fn create_user(
        &self,
        email: &str,
        password: &str,
        character_name: &str,
    ) -> Result<(User, String), Error> {
        let user_id = unsigned_uuid();
        let password_hash = hash_password(password)?;
        let verification_code = unsigned_uuid();

        // Delete any unverified existing user with the same email
        let existing: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM users WHERE email = ?1 AND email_verified = 0"
        )
        .bind(email)
        .fetch_optional(&self.pool).await?;

        if let Some((existing_id,)) = existing {
            sqlx::query("DELETE FROM users WHERE id = ?1")
                .bind(&existing_id)
                .execute(&self.pool).await?;
        }

        sqlx::query(
            "INSERT INTO users (id, email, password_hash, email_verified, verification_code, character_name)
             VALUES (?1, ?2, ?3, 0, ?4, ?5)"
        )
        .bind(&user_id)
        .bind(email)
        .bind(&password_hash)
        .bind(&verification_code)
        .bind(character_name)
        .execute(&self.pool).await?;

        let user = User {
            id: user_id,
            email: email.to_string(),
            password_hash,
            email_verified: false,
            character_name: character_name.into(),
        };

        Ok((user, verification_code))
    }

    pub async fn find_user_by_email(&self, email: &str) -> Result<Option<User>, Error> {
        let user = sqlx::query_as::<_, User>(
            "SELECT id, email, password_hash, email_verified, character_name FROM users WHERE email = ?1"
        )
        .bind(email)
        .fetch_optional(&self.pool).await?;
        Ok(user)
    }

    /// Create a new access token / client token pair for a user.
    /// Old tokens are kept — validity is determined by `created_at` at lookup time.
    /// Returns `(access_token, client_token)` as unsigned UUID strings.
    pub async fn create_tokens(&self, user_id: &str) -> Result<(String, String), Error> {
        let access_token = unsigned_uuid();
        let client_token = unsigned_uuid();
        sqlx::query(
            "INSERT INTO access_tokens (access_token, client_token, user_id)
             VALUES (?1, ?2, ?3)"
        )
        .bind(&access_token)
        .bind(&client_token)
        .bind(user_id)
        .execute(&self.pool).await?;
        Ok((access_token, client_token))
    }

    /// Create a new `access_token` for the `client_token`.
    pub async fn refresh_token(&self, client_token: &str, user_id: &str) -> Result<String, Error> {
        let access_token = unsigned_uuid();
        sqlx::query(
            "INSERT INTO access_tokens (access_token, client_token, user_id) VALUES (?1, ?2, ?3)"
        )
        .bind(&access_token)
        .bind(client_token)
        .bind(user_id)
        .execute(&self.pool).await?;
        Ok(access_token)
    }

    /// Get the [`TokenInfo`] by the access_token provided.
    pub async fn get_token(&self, access_token: &str) -> Result<Option<TokenInfo>, Error> {
        let row: Option<(String, String, String)> = sqlx::query_as(
            "SELECT client_token, user_id, created_at FROM access_tokens WHERE access_token = ?1"
        )
        .bind(access_token)
        .fetch_optional(&self.pool).await?;

        match row {
            Some((client_token, user_id, created_at_str)) => {
                let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                    .map_err(|e| sqlx::Error::Protocol(e.to_string()))?
                    .with_timezone(&Utc);
                Ok(Some(TokenInfo {
                    client_token,
                    user_id,
                    status: TokenStatus::from_created_at(created_at),
                }))
            }
            None => Ok(None),
        }
    }

    /// Look up the profile (id + character_name) for a user by user id.
    pub async fn get_profile_for_user(&self, user_id: &str) -> Result<Option<Profile>, Error> {
        sqlx::query_as::<_, Profile>(
            "SELECT id, character_name AS name FROM users WHERE id = ?1"
        )
        .bind(user_id)
        .fetch_optional(&self.pool).await
        .map_err(Error::Database)
    }

    /// Verify an email address using the verification code.
    pub async fn verify_email(&self, code: &str) -> Result<Option<String>, Error> {
        let user_id: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM users WHERE verification_code = ?1 AND email_verified = 0"
        )
        .bind(code)
        .fetch_optional(&self.pool).await?;

        if let Some((ref uid,)) = user_id {
            sqlx::query(
                "UPDATE users SET email_verified = 1, verification_code = NULL WHERE id = ?1"
            )
            .bind(uid)
            .execute(&self.pool).await?;
        }

        Ok(user_id.map(|(uid,)| uid))
    }
}

/// Generate an unsigned UUID (UUID v4 with dashes stripped).
pub fn unsigned_uuid() -> String {
    Uuid::new_v4().to_string().replace('-', "")
}

/// Hash a plaintext password with argon2.
fn hash_password(password: &str) -> Result<String, Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(Error::PasswordHash)?;
    Ok(hash.to_string())
}
