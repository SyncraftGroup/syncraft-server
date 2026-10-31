use base64::Engine;
use rand_core::OsRng;
use rsa::{
    RsaPrivateKey,
    pkcs1v15::SigningKey,
    pkcs8::{DecodePrivateKey, EncodePrivateKey},
    signature::{SignatureEncoding, Signer},
};
use sha1::Sha1;
use tokio::fs;

use crate::{error::Error, log_info, paths::SyncraftPaths};

/// Load the RSA private key from disk, or generate a new 4096-bit key if none exists.
pub async fn private_key(paths: &SyncraftPaths) -> Result<RsaPrivateKey, Error> {
    let key_file = paths.yggdrasil().private_key();
    if key_file.exists()
        && let Ok(pem) = fs::read_to_string(&key_file).await
        && let Ok(key) = RsaPrivateKey::from_pkcs8_pem(&pem)
    {
        return Ok(key);
    }

    log_info!("Generating RSA keys, this may take a while...");
    let private_key = RsaPrivateKey::new(&mut OsRng, 4096)?;
    let private_pem = private_key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)?;
    fs::write(key_file.clone(), private_pem.as_bytes()).await?;
    restrict_key_file(&key_file)?;

    Ok(private_key)
}

/// The private key signs every profile we hand out; keep it owner-readable only.
fn restrict_key_file(path: &std::path::Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Sign raw data with the given RSA-SHA-1 signing key, returning a Base64-encoded signature.
pub fn sign(signing_key: &SigningKey<Sha1>, data: &[u8]) -> String {
    let signature = signing_key.sign(data);
    base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
}
