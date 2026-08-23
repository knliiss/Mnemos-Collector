use anyhow::{Context, Result, bail};
#[cfg(not(target_os = "windows"))]
use keyring::{Entry, Error as KeyringError};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(target_os = "windows")]
mod windows;

const KEYRING_SERVICE: &str = "mnemos-collector";
const ACCESS_KEY_ACCOUNT: &str = "collector-access-key";
const PENDING_PROVISIONING_ACCOUNT: &str = "pending-provisioning";
const SECRET_LENGTH: usize = 43;

#[derive(Debug, Clone, Copy, Default)]
pub struct CredentialStore;

impl CredentialStore {
    pub fn load(self) -> Result<Option<String>> {
        #[cfg(target_os = "windows")]
        {
            let access_key = windows::load(&credential_target(ACCESS_KEY_ACCOUNT))?;

            if let Some(access_key) = access_key.as_deref() {
                validate_access_key(access_key)?;
            }

            return Ok(access_key);
        }

        #[cfg(not(target_os = "windows"))]
        {
            let entry = Entry::new(KEYRING_SERVICE, ACCESS_KEY_ACCOUNT)
                .context("failed to open the operating-system credential store")?;

            match entry.get_password() {
                Ok(access_key) => {
                    validate_access_key(&access_key)?;
                    Ok(Some(access_key))
                }
                Err(KeyringError::NoEntry) => Ok(None),
                Err(error) => Err(error).context("failed to read collector access key"),
            }
        }
    }

    pub fn save(self, access_key: &str) -> Result<()> {
        validate_access_key(access_key)?;

        #[cfg(target_os = "windows")]
        {
            return windows::save(
                &credential_target(ACCESS_KEY_ACCOUNT),
                ACCESS_KEY_ACCOUNT,
                access_key,
            )
            .context("failed to save collector access key");
        }

        #[cfg(not(target_os = "windows"))]
        {
            Entry::new(KEYRING_SERVICE, ACCESS_KEY_ACCOUNT)
                .context("failed to open the operating-system credential store")?
                .set_password(access_key)
                .context("failed to save collector access key")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingProvisioningSecret {
    pub activation_hash: String,
    pub secret: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PendingProvisioningStore;

impl PendingProvisioningStore {
    pub fn load(self) -> Result<Option<PendingProvisioningSecret>> {
        #[cfg(target_os = "windows")]
        let encoded = windows::load(&credential_target(PENDING_PROVISIONING_ACCOUNT))?;

        #[cfg(not(target_os = "windows"))]
        let encoded = {
            let entry = Entry::new(KEYRING_SERVICE, PENDING_PROVISIONING_ACCOUNT)
                .context("failed to open the operating-system credential store")?;

            match entry.get_password() {
                Ok(encoded) => Some(encoded),
                Err(KeyringError::NoEntry) => None,
                Err(error) => {
                    return Err(error).context("failed to read pending provisioning secret");
                }
            }
        };

        let Some(encoded) = encoded else {
            return Ok(None);
        };
        let pending: PendingProvisioningSecret =
            serde_json::from_str(&encoded).context("pending provisioning secret is corrupted")?;

        validate_secret(&pending.secret)?;

        if pending.activation_hash.len() != 64
            || !pending
                .activation_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            bail!("pending provisioning activation hash has an invalid format");
        }

        Ok(Some(pending))
    }

    pub fn save(self, pending: &PendingProvisioningSecret) -> Result<()> {
        validate_secret(&pending.secret)?;

        let encoded = serde_json::to_string(pending)
            .context("failed to encode pending provisioning secret")?;

        #[cfg(target_os = "windows")]
        {
            return windows::save(
                &credential_target(PENDING_PROVISIONING_ACCOUNT),
                PENDING_PROVISIONING_ACCOUNT,
                &encoded,
            )
            .context("failed to save pending provisioning secret");
        }

        #[cfg(not(target_os = "windows"))]
        {
            Entry::new(KEYRING_SERVICE, PENDING_PROVISIONING_ACCOUNT)
                .context("failed to open the operating-system credential store")?
                .set_password(&encoded)
                .context("failed to save pending provisioning secret")
        }
    }

    pub fn clear(self) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            return windows::delete(&credential_target(PENDING_PROVISIONING_ACCOUNT))
                .context("failed to clear pending provisioning secret");
        }

        #[cfg(not(target_os = "windows"))]
        {
            let entry = Entry::new(KEYRING_SERVICE, PENDING_PROVISIONING_ACCOUNT)
                .context("failed to open the operating-system credential store")?;

            match entry.delete_credential() {
                Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
                Err(error) => Err(error).context("failed to clear pending provisioning secret"),
            }
        }
    }
}

pub fn credential_id_from_access_key(access_key: &str) -> Result<Uuid> {
    let (credential_id, secret) = split_access_key(access_key)?;

    validate_secret(secret)?;

    Uuid::parse_str(credential_id).context("collector credential id is not a UUID")
}

pub fn validate_access_key(access_key: &str) -> Result<()> {
    credential_id_from_access_key(access_key)?;

    Ok(())
}

pub fn validate_secret(secret: &str) -> Result<()> {
    if secret.len() != SECRET_LENGTH
        || !secret
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        bail!("collector access key secret has an invalid format");
    }

    Ok(())
}

#[cfg(any(target_os = "windows", test))]
fn credential_target(account: &str) -> String {
    format!("{account}.{KEYRING_SERVICE}")
}

fn split_access_key(access_key: &str) -> Result<(&str, &str)> {
    access_key
        .split_once('.')
        .context("collector access key has an invalid format")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_credential_target_matches_keyring_native_mapping() {
        assert_eq!(
            credential_target(ACCESS_KEY_ACCOUNT),
            "collector-access-key.mnemos-collector"
        );
    }

    #[test]
    fn validates_server_access_key_format() {
        let key =
            "019c1129-ef54-7000-8000-000000000220.ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmno12";

        assert!(validate_access_key(key).is_ok());
    }

    #[test]
    fn extracts_credential_id_from_access_key() {
        let key =
            "019c1129-ef54-7000-8000-000000000220.ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmno12";

        let credential_id = credential_id_from_access_key(key).unwrap();

        assert_eq!(
            credential_id,
            Uuid::parse_str("019c1129-ef54-7000-8000-000000000220").unwrap()
        );
    }

    #[test]
    fn rejects_shared_or_malformed_secrets() {
        assert!(validate_access_key("shared-secret").is_err());
        assert!(
            validate_access_key("019c1129-ef54-7000-8000-000000000220.secret-with-invalid-length")
                .is_err()
        );
    }
}
