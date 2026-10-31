//! Optional Ed25519 signing of the manifest.
//!
//! Signing is not a replacement for TLS. It lets clients detect a manifest that
//! was modified in transit (caching proxy, hostile network) when the server only
//! speaks HTTP, and makes an unexpected key change loud instead of silent.
//!
//! The key lives in `syncraft/data/manifest_key.hex` with `0600` permissions and
//! is generated on first use.

use ed25519_dalek::{Signer, SigningKey};
use rand_core::OsRng;
use syncraft_protocol::{Manifest, ManifestSignature, manifest_signing_payload};

use crate::{error::Error, paths::SyncraftPaths};

const KEY_FILE: &str = "manifest_key.hex";

fn key_path(paths: &SyncraftPaths) -> std::path::PathBuf {
    paths.data().join(KEY_FILE)
}

/// Load the server signing key, generating it on first use.
pub fn signing_key(paths: &SyncraftPaths) -> Result<SigningKey, Error> {
    let path = key_path(paths);

    if let Ok(content) = std::fs::read_to_string(&path) {
        let bytes = hex::decode(content.trim())
            .map_err(|_| Error::Server("invalid manifest signing key".into()))?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::Server("invalid manifest signing key length".into()))?;
        return Ok(SigningKey::from_bytes(&bytes));
    }

    let key = SigningKey::generate(&mut OsRng);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, hex::encode(key.to_bytes()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(key)
}

/// Attach a signature to `manifest`, or clear it when signing is disabled.
pub fn sign_if_enabled(
    paths: &SyncraftPaths,
    manifest: &mut Manifest,
    enabled: bool,
) -> Result<(), Error> {
    if !enabled {
        manifest.signature = None;
        return Ok(());
    }

    let key = signing_key(paths)?;
    let signature = key.sign(&manifest_signing_payload(&manifest.version_hash));

    manifest.signature = Some(ManifestSignature {
        algorithm: "ed25519".to_string(),
        key: hex::encode(key.verifying_key().as_bytes()),
        value: hex::encode(signature.to_bytes()),
    });
    Ok(())
}

/// Short, human-comparable fingerprint of a public key.
pub fn fingerprint(public_key_hex: &str) -> String {
    let hash = blake3::hash(public_key_hex.as_bytes());
    hash.to_hex().to_string()[..16].to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    use syncraft_protocol::{Loader, MANIFEST_SCHEMA, VersionInfo};

    use super::*;

    fn temp_paths(name: &str) -> SyncraftPaths {
        let root = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&root);
        let paths = SyncraftPaths::new(root);
        paths.create_dirs().unwrap();
        paths
    }

    fn unsigned_manifest() -> Manifest {
        Manifest {
            schema: MANIFEST_SCHEMA,
            version_hash: "a".repeat(64),
            commit: "deadbeef".into(),
            version: VersionInfo {
                name: "test".into(),
                id: "test".into(),
                vanilla_version: "1.21.1".into(),
                loader: Loader::Vanilla,
            },
            download_concurrency: None,
            signature: None,
            files: BTreeMap::new(),
        }
    }

    #[test]
    fn signature_verifies_and_key_is_persisted() {
        let paths = temp_paths("syncraft_test_manifest_signing");
        let mut manifest = unsigned_manifest();

        sign_if_enabled(&paths, &mut manifest, true).unwrap();
        let signature = manifest.signature.clone().unwrap();
        assert_eq!(signature.algorithm, "ed25519");

        // The key file is reused on the next call.
        let mut again = unsigned_manifest();
        sign_if_enabled(&paths, &mut again, true).unwrap();
        assert_eq!(again.signature.unwrap().key, signature.key);

        let key: [u8; 32] = hex::decode(&signature.key).unwrap().try_into().unwrap();
        let verifying = VerifyingKey::from_bytes(&key).unwrap();
        let sig =
            Signature::from_bytes(&hex::decode(&signature.value).unwrap().try_into().unwrap());
        verifying
            .verify(&manifest_signing_payload(&manifest.version_hash), &sig)
            .expect("signature must verify");

        // A different version hash must not verify.
        assert!(
            verifying
                .verify(&manifest_signing_payload(&"b".repeat(64)), &sig)
                .is_err()
        );

        let _ = std::fs::remove_dir_all(paths.root());
    }

    #[test]
    fn signing_can_be_disabled() {
        let paths = temp_paths("syncraft_test_manifest_signing_off");
        let mut manifest = unsigned_manifest();
        sign_if_enabled(&paths, &mut manifest, true).unwrap();
        sign_if_enabled(&paths, &mut manifest, false).unwrap();
        assert!(manifest.signature.is_none());
        let _ = std::fs::remove_dir_all(paths.root());
    }
}
