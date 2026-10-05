//! The host's list of authorised devices.
//!
//! Pairing adds a device; connections are accepted only from keys on the list
//! and not revoked. The list is a small JSON file, written owner-only. Keys are
//! compared in constant time. A revoked device stays listed (so its history
//! is visible) but is refused.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use thiserror::Error;

use crate::identity::{write_private_file, DeviceId};

/// Why a registry operation failed.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// The file could not be read or written.
    #[error("cannot access the device list: {0}")]
    Io(#[from] std::io::Error),
    /// The file is damaged.
    #[error("the device list is damaged")]
    Corrupt,
    /// No device matches.
    #[error("no such device")]
    NotFound,
    /// The selector matches several devices.
    #[error("that matches more than one device; use more of the id")]
    Ambiguous,
}

/// One paired device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    /// Hex of the device's static public key.
    key: String,
    /// The name it gave itself.
    pub name: String,
    /// Seconds since the epoch when it was paired.
    pub first_seen_unix: u64,
    /// Seconds since the epoch of its last connection.
    pub last_seen_unix: u64,
    /// Whether the owner revoked it.
    pub revoked: bool,
}

impl DeviceRecord {
    /// The device's static public key.
    pub fn public_key(&self) -> [u8; 32] {
        from_hex(&self.key).unwrap_or([0; 32])
    }

    /// The device's fingerprint.
    pub fn device_id(&self) -> DeviceId {
        DeviceId::of(&self.public_key())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    devices: Vec<DeviceRecord>,
}

/// The authorised devices.
#[derive(Debug, Default)]
pub struct DeviceRegistry {
    path: Option<PathBuf>,
    devices: Vec<DeviceRecord>,
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(text.as_bytes().chunks(2)) {
        *slot = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

impl DeviceRegistry {
    /// A registry that is not saved anywhere.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Opens the registry at `path`; a missing file is an empty registry.
    pub fn open(path: &Path) -> Result<Self, RegistryError> {
        let devices = match std::fs::read(path) {
            Ok(bytes) => {
                let file: File =
                    serde_json::from_slice(&bytes).map_err(|_| RegistryError::Corrupt)?;
                if file.devices.iter().any(|d| from_hex(&d.key).is_none()) {
                    return Err(RegistryError::Corrupt);
                }
                file.devices
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path: Some(path.to_path_buf()),
            devices,
        })
    }

    fn save(&self) -> Result<(), RegistryError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = serde_json::to_vec_pretty(&File {
            devices: self.devices.clone(),
        })
        .map_err(|_| RegistryError::Corrupt)?;
        write_private_file(path, &text)?;
        Ok(())
    }

    /// All devices, oldest first.
    pub fn devices(&self) -> &[DeviceRecord] {
        &self.devices
    }

    fn position(&self, key: &[u8; 32]) -> Option<usize> {
        // Every entry is compared, in constant time, so the time taken does
        // not depend on where (or whether) the key is on the list.
        let mut found = None;
        for (index, record) in self.devices.iter().enumerate() {
            if bool::from(record.public_key().ct_eq(key)) {
                found = Some(index);
            }
        }
        found
    }

    /// Whether `key` belongs to a device that is paired and not revoked.
    pub fn is_authorised(&self, key: &[u8; 32]) -> bool {
        self.position(key)
            .is_some_and(|index| !self.devices[index].revoked)
    }

    /// Adds a device, or re-authorises one that was revoked and paired again.
    pub fn authorise(
        &mut self,
        key: [u8; 32],
        name: &str,
        now_unix: u64,
    ) -> Result<(), RegistryError> {
        match self.position(&key) {
            Some(index) => {
                let record = &mut self.devices[index];
                record.revoked = false;
                record.name = name.to_owned();
                record.last_seen_unix = now_unix;
            }
            None => self.devices.push(DeviceRecord {
                key: to_hex(&key),
                name: name.to_owned(),
                first_seen_unix: now_unix,
                last_seen_unix: now_unix,
                revoked: false,
            }),
        }
        self.save()
    }

    /// Records a connection from `key`.
    pub fn touch(&mut self, key: &[u8; 32], now_unix: u64) -> Result<(), RegistryError> {
        if let Some(index) = self.position(key) {
            self.devices[index].last_seen_unix = now_unix;
            self.save()?;
        }
        Ok(())
    }

    /// Revokes the device that `selector` names: its id (any prefix of at
    /// least four symbols, dashes ignored) or its exact name.
    pub fn revoke(&mut self, selector: &str) -> Result<DeviceRecord, RegistryError> {
        let wanted: String = selector
            .chars()
            .filter(|c| *c != '-' && !c.is_whitespace())
            .collect::<String>()
            .to_ascii_uppercase();
        let matches: Vec<usize> = self
            .devices
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                (wanted.len() >= 4 && d.device_id().full().starts_with(&wanted))
                    || d.name == selector
            })
            .map(|(i, _)| i)
            .collect();
        match matches.as_slice() {
            [] => Err(RegistryError::NotFound),
            [index] => {
                self.devices[*index].revoked = true;
                self.save()?;
                Ok(self.devices[*index].clone())
            }
            _ => Err(RegistryError::Ambiguous),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paired_device_is_authorised_until_revoked() {
        let mut registry = DeviceRegistry::in_memory();
        registry.authorise([1; 32], "laptop", 100).unwrap();
        assert!(registry.is_authorised(&[1; 32]));
        assert!(!registry.is_authorised(&[2; 32]));
        let record = registry.revoke("laptop").unwrap();
        assert!(record.revoked);
        assert!(!registry.is_authorised(&[1; 32]));
        assert_eq!(registry.devices().len(), 1, "it stays listed");
    }

    #[test]
    fn pairing_a_revoked_device_again_authorises_it() {
        let mut registry = DeviceRegistry::in_memory();
        registry.authorise([1; 32], "laptop", 100).unwrap();
        registry.revoke("laptop").unwrap();
        registry.authorise([1; 32], "laptop", 200).unwrap();
        assert!(registry.is_authorised(&[1; 32]));
        assert_eq!(registry.devices()[0].first_seen_unix, 100);
    }

    #[test]
    fn a_device_is_revoked_by_id_prefix_dashes_and_case_ignored() {
        let mut registry = DeviceRegistry::in_memory();
        registry.authorise([1; 32], "a", 1).unwrap();
        registry.authorise([2; 32], "b", 1).unwrap();
        let short = DeviceId::of(&[2; 32]).short().to_ascii_lowercase();
        assert_eq!(registry.revoke(&short).unwrap().name, "b");
        assert!(registry.is_authorised(&[1; 32]));
        assert!(matches!(
            registry.revoke("nope"),
            Err(RegistryError::NotFound)
        ));
        assert!(matches!(
            registry.revoke("AB"),
            Err(RegistryError::NotFound)
        ));
    }

    #[test]
    fn two_devices_with_one_name_need_the_id() {
        let mut registry = DeviceRegistry::in_memory();
        registry.authorise([1; 32], "same", 1).unwrap();
        registry.authorise([2; 32], "same", 1).unwrap();
        assert!(matches!(
            registry.revoke("same"),
            Err(RegistryError::Ambiguous)
        ));
    }

    #[test]
    fn the_list_survives_a_restart_and_the_file_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        {
            let mut registry = DeviceRegistry::open(&path).unwrap();
            registry.authorise([5; 32], "phone", 10).unwrap();
            registry.touch(&[5; 32], 99).unwrap();
        }
        let registry = DeviceRegistry::open(&path).unwrap();
        assert!(registry.is_authorised(&[5; 32]));
        assert_eq!(registry.devices()[0].last_seen_unix, 99);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn a_damaged_file_is_an_error_not_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(matches!(
            DeviceRegistry::open(&path),
            Err(RegistryError::Corrupt)
        ));
        std::fs::write(&path, br#"{"devices":[{"key":"zz","name":"x","first_seen_unix":0,"last_seen_unix":0,"revoked":false}]}"#).unwrap();
        assert!(matches!(
            DeviceRegistry::open(&path),
            Err(RegistryError::Corrupt)
        ));
    }
}
