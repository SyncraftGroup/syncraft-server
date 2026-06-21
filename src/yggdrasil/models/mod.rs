use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

pub mod requests;
pub mod responses;

/// Internal user row from the database.
/// Each user has exactly one character — profile fields live directly on the user row.
#[derive(Debug, sqlx::FromRow)]
pub struct User {
    pub id: String,
    pub email: String,
    pub password_hash: String,
    pub email_verified: bool,
    pub character_name: String,
}

/// The status of a token determined purely by its age.
#[derive(Debug, PartialEq)]
pub enum TokenStatus {
    /// Within 48 hours — valid for MC login and refresh
    Active,
    /// Between 48h and 7d — valid only for refresh
    Inactive,
    /// Beyond 7d — invalid, treated as non-existent
    Invalid,
}

impl TokenStatus {
    pub fn from_created_at(created_at: DateTime<Utc>) -> Self {
        let now = Utc::now();
        let duration = now.signed_duration_since(created_at);

        if duration < Duration::hours(48) {
            TokenStatus::Active
        } else if duration < Duration::days(7) {
            TokenStatus::Inactive
        } else {
            TokenStatus::Invalid
        }
    }
}

/// Result of looking up a token, including status and owning user.
#[derive(Debug)]
pub struct TokenInfo {
    pub client_token: String,
    pub user_id: String,
    pub status: TokenStatus,
}

/// Yggdrasil error response body (returned with appropriate HTTP status)
#[derive(Debug, Serialize)]
pub struct YggdrasilError {
    pub error: String,
    #[serde(rename = "errorMessage")]
    pub error_message: String,
}

/// A user's character profile.
#[derive(Debug, Serialize, Deserialize, Clone, sqlx::FromRow)]
pub struct Profile {
    pub id: String,
    pub name: String
}