use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Git(#[from] git2::Error),
    #[error("{0}")]
    UI(String),
    #[error("{0}")]
    TomlSerialize(#[from] toml::ser::Error),
    #[error("{0}")]
    JsonSerialize(#[from] serde_json::Error),
    #[error("{0}")]
    Database(#[from] sqlx::Error),
    #[error("{0}")]
    PasswordHash(argon2::password_hash::Error),
}