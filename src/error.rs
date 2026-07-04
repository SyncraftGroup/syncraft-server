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
    #[error("{0}")]
    Sign(String),
    #[error("{0}")]
    SkinUpload(String),
    #[error("Daemon error: {0}")]
    Daemon(String)
}

impl From<axum_extra::extract::multipart::MultipartError> for Error {
    fn from(value: axum_extra::extract::multipart::MultipartError) -> Self {
        Self::SkinUpload(value.to_string())
    }
}

impl From<rsa::pkcs8::spki::Error> for Error {
    fn from(value: rsa::pkcs8::spki::Error) -> Self {
        Self::Sign(value.to_string())
    }
}

impl From<rsa::pkcs8::Error> for Error {
    fn from(value: rsa::pkcs8::Error) -> Self {
        Self::Sign(value.to_string())
    }
}

impl From<rsa::Error> for Error {
    fn from(value: rsa::Error) -> Self {
        Self::Sign(value.to_string())
    }
}