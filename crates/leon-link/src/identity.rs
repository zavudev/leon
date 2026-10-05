//! Long-term identity: the keys an installation is known by.
//!
//! Two keys are generated once and kept in the data directory:
//!
//! * an X25519 static key, used by the Noise handshakes; a [`DeviceId`] is a
//!   fingerprint of its public half;
//! * an Ed25519 signing key, with which a host proves to a relay that it owns
//!   its [`HostId`] (a hash of the verifying key).
//!
//! The file is created with mode `0600` on Unix (on Windows it inherits the
//! user profile's access control) and replaced atomically. The secrets are
//! zeroized when dropped and are never printed: `Debug` shows fingerprints
//! only.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use leon_wire::{base32, HostId};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use thiserror::Error;
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"LEONKEY1";
/// The name of the key file inside the identity directory.
pub const KEY_FILE: &str = "identity.key";

/// Why an identity could not be loaded or saved.
#[derive(Debug, Error)]
pub enum IdentityError {
    /// The file system refused.
    #[error("cannot access the identity file: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not a Leon identity.
    #[error("the identity file is damaged")]
    Corrupt,
}

/// A short, public fingerprint of a device's key, shown to people so they can
/// compare it: grouped base32.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId([u8; 16]);

impl DeviceId {
    /// The fingerprint of an X25519 public key.
    pub fn of(public: &[u8; 32]) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"leon-device-id-v1");
        hash.update(public);
        let digest = hash.finalize();
        let mut id = [0u8; 16];
        id.copy_from_slice(&digest[..16]);
        Self(id)
    }

    /// The raw bytes.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// The id from its bytes.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The first twelve symbols in four groups: what a screen shows.
    pub fn short(&self) -> String {
        let text = base32(&self.0);
        format!(
            "{}-{}-{}-{}",
            &text[0..3],
            &text[3..6],
            &text[6..9],
            &text[9..12]
        )
    }

    /// Parses a full id or any prefix of at least four symbols; the matching
    /// is done by the caller.
    pub fn full(&self) -> String {
        base32(&self.0)
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.short())
    }
}

/// An installation's keys.
pub struct Identity {
    static_secret: Zeroizing<[u8; 32]>,
    static_public: [u8; 32],
    signing: SigningKey,
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("device_id", &self.device_id())
            .field("host_id", &self.host_id())
            .finish_non_exhaustive()
    }
}

impl Identity {
    /// Generates fresh keys.
    pub fn generate() -> Self {
        let mut secret = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(&mut *secret);
        let mut seed = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(&mut *seed);
        Self::from_secrets(secret, &seed)
    }

    fn from_secrets(secret: Zeroizing<[u8; 32]>, seed: &[u8; 32]) -> Self {
        let public = x25519_public(&secret);
        Self {
            static_secret: secret,
            static_public: public,
            signing: SigningKey::from_bytes(seed),
        }
    }

    /// Loads the identity in `dir`, creating and saving a new one when there
    /// is none. The directory is created if needed.
    pub fn load_or_create(dir: &Path) -> Result<Self, IdentityError> {
        let path = dir.join(KEY_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let identity = Self::decode(&bytes)?;
                tighten(&path);
                Ok(identity)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(dir)?;
                let identity = Self::generate();
                identity.save(&path)?;
                Ok(identity)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn decode(bytes: &[u8]) -> Result<Self, IdentityError> {
        if bytes.len() != 8 + 64 || &bytes[..8] != MAGIC {
            return Err(IdentityError::Corrupt);
        }
        let mut secret = Zeroizing::new([0u8; 32]);
        secret.copy_from_slice(&bytes[8..40]);
        let mut seed = Zeroizing::new([0u8; 32]);
        seed.copy_from_slice(&bytes[40..72]);
        Ok(Self::from_secrets(secret, &seed))
    }

    fn save(&self, path: &Path) -> Result<(), IdentityError> {
        let mut bytes = Zeroizing::new(Vec::with_capacity(72));
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&*self.static_secret);
        bytes.extend_from_slice(&self.signing.to_bytes());
        write_private(path, &bytes)?;
        Ok(())
    }

    /// The X25519 secret, for the Noise handshakes of this crate.
    pub(crate) fn static_secret(&self) -> &[u8; 32] {
        &self.static_secret
    }

    /// The X25519 public key.
    pub fn static_public(&self) -> [u8; 32] {
        self.static_public
    }

    /// The fingerprint of this device.
    pub fn device_id(&self) -> DeviceId {
        DeviceId::of(&self.static_public)
    }

    /// The Ed25519 verifying key.
    pub fn verifying_key(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    /// The id this installation has as a host.
    pub fn host_id(&self) -> HostId {
        HostId::from_verifying_key(&self.verifying_key())
    }

    /// Signs `message` with the Ed25519 key (relay registration).
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.signing.sign(message).to_bytes().to_vec()
    }
}

/// Whether `signature` is a valid Ed25519 signature of `message` by the key.
pub fn verify(verifying_key: &[u8; 32], message: &[u8], signature: &[u8]) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(verifying_key) else {
        return false;
    };
    let Ok(signature) = <[u8; 64]>::try_from(signature) else {
        return false;
    };
    key.verify_strict(message, &ed25519_dalek::Signature::from_bytes(&signature))
        .is_ok()
}

/// The X25519 public key for `secret` (the clamped scalar times the base
/// point), the same function the Noise handshakes use.
fn x25519_public(secret: &[u8; 32]) -> [u8; 32] {
    let secret = x25519_dalek::StaticSecret::from(*secret);
    x25519_dalek::PublicKey::from(&secret).to_bytes()
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut temp: PathBuf = path.to_path_buf();
    temp.set_extension("tmp");
    let _ = std::fs::remove_file(&temp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temp, path)
}

/// Writes `bytes` to `path` owner-only, replacing it atomically.
pub(crate) fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_private(path, bytes)
}

#[cfg(unix)]
fn tighten(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.permissions().mode() & 0o077 != 0 {
            tracing::warn!("the identity file was readable by others; restricting it");
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(not(unix))]
fn tighten(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_identity_is_saved_and_loaded_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let first = Identity::load_or_create(dir.path()).unwrap();
        let second = Identity::load_or_create(dir.path()).unwrap();
        assert_eq!(first.static_public(), second.static_public());
        assert_eq!(first.host_id(), second.host_id());
        assert_eq!(first.device_id(), second.device_id());
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        Identity::load_or_create(dir.path()).unwrap();
        let mode = std::fs::metadata(dir.path().join(KEY_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_key_file_that_others_can_read_is_restricted_on_load() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        Identity::load_or_create(dir.path()).unwrap();
        let path = dir.path().join(KEY_FILE);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        Identity::load_or_create(dir.path()).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_damaged_key_file_is_refused_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(KEY_FILE), b"nope").unwrap();
        assert!(matches!(
            Identity::load_or_create(dir.path()),
            Err(IdentityError::Corrupt)
        ));
        assert_eq!(std::fs::read(dir.path().join(KEY_FILE)).unwrap(), b"nope");
    }

    #[test]
    fn debug_output_shows_fingerprints_and_no_key_material() {
        let identity = Identity::generate();
        let text = format!("{identity:?}");
        assert!(text.contains("DeviceId"));
        assert!(!text.contains("static_secret"));
        assert!(!text.contains("signing"));
    }

    #[test]
    fn two_identities_differ_and_the_host_id_follows_the_signing_key() {
        let (a, b) = (Identity::generate(), Identity::generate());
        assert_ne!(a.device_id(), b.device_id());
        assert_eq!(a.host_id(), HostId::from_verifying_key(&a.verifying_key()));
    }

    #[test]
    fn a_signature_verifies_only_for_its_message_and_key() {
        let identity = Identity::generate();
        let signature = identity.sign(b"nonce");
        assert!(verify(&identity.verifying_key(), b"nonce", &signature));
        assert!(!verify(&identity.verifying_key(), b"other", &signature));
        assert!(!verify(
            &Identity::generate().verifying_key(),
            b"nonce",
            &signature
        ));
        assert!(!verify(
            &identity.verifying_key(),
            b"nonce",
            &signature[..10]
        ));
    }
}
