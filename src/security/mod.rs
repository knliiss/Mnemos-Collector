use anyhow::{Context, Result, bail};
use keyring::{Entry, Error as KeyringError};
use uuid::Uuid;

const KEYRING_SERVICE: &str = "mnemos-collector";
const KEYRING_ACCOUNT: &str = "collector-access-key";
const SECRET_LENGTH: usize = 43;

#[derive(Debug, Clone, Copy, Default)]
pub struct CredentialStore;

impl CredentialStore {
    pub fn load(self) -> Result<Option<String>> {
        let entry = Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
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

    pub fn save(self, access_key: &str) -> Result<()> {
        validate_access_key(access_key)?;

        Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
            .context("failed to open the operating-system credential store")?
            .set_password(access_key)
            .context("failed to save collector access key")
    }
}

pub fn validate_access_key(access_key: &str) -> Result<()> {
    let Some((credential_id, secret)) = access_key.split_once('.') else {
        bail!("collector access key has an invalid format");
    };

    Uuid::parse_str(credential_id).context("collector credential id is not a UUID")?;

    if secret.len() != SECRET_LENGTH
        || !secret
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        bail!("collector access key secret has an invalid format");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_server_access_key_format() {
        let key = "019c1129-ef54-7000-8000-000000000220.ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmno12";

        assert!(validate_access_key(key).is_ok());
    }

    #[test]
    fn rejects_shared_or_malformed_secrets() {
        assert!(validate_access_key("shared-secret").is_err());
        assert!(
            validate_access_key(
                "019c1129-ef54-7000-8000-000000000220.secret-with-invalid-length"
            )
            .is_err()
        );
    }
}
