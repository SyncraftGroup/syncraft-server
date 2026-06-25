use argon2::{
    Argon2, PasswordHasher,
    password_hash::SaltString,
};
use chrono::{DateTime, Utc};
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{error::Error, paths::SyncraftPaths, yggdrasil::models::{Profile, YggdrasilSkinType, TokenInfo, TokenStatus, User}};

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
                skin_hash           TEXT,
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

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS skins (
                skin_hash       TEXT PRIMARY KEY,
                skin_data       BLOB,
                skin_type       TEXT NOT NULL DEFAULT 'default',
                updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            )"
        ).execute(pool).await?;

        Ok(())
    }

    /// Create a new user via the register API.
    /// Returns `(User, verification_code)` where both IDs are unsigned UUID strings.
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
            character_name: character_name.into()
        };

        Ok((user, verification_code))
    }

    pub async fn find_user_by_email(&self, email: &str) -> Result<Option<User>, Error> {
        let user = sqlx::query_as::<_, User>(
            "SELECT id, email, password_hash, email_verified, character_name, skin_hash
            FROM users WHERE email = ?1"
        )
        .bind(email)
        .fetch_optional(&self.pool).await?;
        Ok(user)
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

    /// Issue a new access-token / client-token pair for a user.
    /// Old tokens are retained — validity is determined from `created_at` at lookup time.
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

    /// Issue a fresh `access_token` for an existing `client_token` (token refresh).
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

    /// Look up [TokenInfo] by access token.
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

    /// Look up the character profile (id + name) for a user by their user id.
    pub async fn get_profile_for_user(&self, user_id: &str) -> Result<Option<Profile>, Error> {
        sqlx::query_as::<_, Profile>(
            "SELECT id, character_name AS name FROM users WHERE id = ?1"
        )
        .bind(user_id)
        .fetch_optional(&self.pool).await
        .map_err(Error::Database)
    }

    /// Set a user's skin (upserts the skin blob and links it to the user).
    pub async fn set_skin(&self, user_id: &str, skin_data: Vec<u8>, skin_type: YggdrasilSkinType) -> Result<(), Error> {
        
        let skin_hash = hash_skin_data(&skin_data);
        
        sqlx::query(
            "INSERT OR IGNORE INTO skins (skin_hash, skin_data, skin_type, updated_at)
            VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))"
        )
        .bind(&skin_hash)
        .bind(&skin_data)
        .bind(&skin_type.to_string())
        .execute(&self.pool).await?;

        sqlx::query(
            "UPDATE users SET skin_hash = ?1 WHERE id = ?2"
        )
        .bind(&skin_hash)
        .bind(user_id)
        .execute(&self.pool).await?;

        Ok(())
    }

    // /// Get a user's skin by user_id.
    // pub async fn get_skin_by_user_id(&self, user_id: &str) -> Result<Option<(Vec<u8>, SyncraftSkinType)>, Error> {
    //     let result: Option<(Vec<u8>, String)> = sqlx::query_as(
    //         "SELECT s.skin_data, s.skin_type
    //         FROM skins s
    //         INNER JOIN users u ON u.skin_hash = s.skin_hash
    //         WHERE u.id = ?1"
    //     )
    //     .bind(user_id)
    //     .fetch_optional(&self.pool).await?;

    //     match result {
    //         Some((data, skin_type)) => {
    //             Ok(Some((data, SyncraftSkinType::from(skin_type))))
    //         }
    //         None => Ok(None)
    //     }
    // }

    /// Get raw skin image data by skin hash.
    pub async fn get_skin_data(&self, skin_hash: &str) -> Result<Option<Vec<u8>>, Error> {
        let result: Option<(Vec<u8>,)> = sqlx::query_as(
            "SELECT skin_data FROM skins WHERE skin_hash = ?1"
        )
        .bind(skin_hash)
        .fetch_optional(&self.pool).await?;

        Ok(result.map(|(data,)| data))
    }

    /// Get a user's skin metadata (hash, type), if any.
    pub async fn get_user_skin_meta(&self, user_id: &str) -> Result<Option<(String, YggdrasilSkinType)>, Error> {
        let result: Option<(String, String)> = sqlx::query_as(
            "SELECT s.skin_hash, s.skin_type
            FROM skins s
            INNER JOIN users u ON u.skin_hash = s.skin_hash
            WHERE u.id = ?1"
        )
        .bind(user_id)
        .fetch_optional(&self.pool).await?;

        match result {
            Some((hash, skin_type)) => {
                Ok(Some((hash, YggdrasilSkinType::from(skin_type))))
            }
            None => Ok(None),
        }
    }

    /// Unlink and remove a user's skin by user id.
    pub async fn remove_skin(&self, user_id: &str) -> Result<(), Error> {
        sqlx::query("UPDATE users SET skin_hash = NULL WHERE id = ?1")
            .bind(user_id)
            .execute(&self.pool).await?;

        self.gc_skins().await?;
        Ok(())
    }

    /// Delete a single access token (used by `POST /authserver/invalidate`).
    pub async fn delete_token(&self, access_token: &str) -> Result<(), Error> {
        sqlx::query("DELETE FROM access_tokens WHERE access_token = ?1")
            .bind(access_token)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete all tokens belonging to a user (used by `POST /authserver/signout`).
    pub async fn delete_user_tokens(&self, user_id: &str) -> Result<(), Error> {
        sqlx::query("DELETE FROM access_tokens WHERE user_id = ?1")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete orphaned skins that are no longer referenced by any user.
    pub async fn gc_skins(&self) -> Result<(), Error> {
        sqlx::query(
            "DELETE FROM skins
             WHERE skin_hash NOT IN (
                 SELECT DISTINCT skin_hash FROM users WHERE skin_hash IS NOT NULL
             )"
        )
        .execute(&self.pool).await?;

        Ok(())
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

/// Hash the raw skin data with SHA-256, returning a hex-encoded digest.
fn hash_skin_data(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    hex::encode(digest)
}