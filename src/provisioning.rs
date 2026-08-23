use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::security::{
    CredentialStore, PendingProvisioningSecret, PendingProvisioningStore, validate_access_key,
    validate_secret,
};

const SECRET_BYTES: usize = 32;
const ACTIVATION_TOKEN_LENGTH: usize = 43;

#[derive(Debug, Clone)]
pub struct ProvisioningConfig {
    pub endpoint: String,
    pub request_timeout: Duration,
}

impl Default for ProvisioningConfig {
    fn default() -> Self {
        Self {
            endpoint: "https://api.knalis.rest/api/v1/realtime/collector-activations/redeem"
                .to_owned(),
            request_timeout: Duration::from_secs(10),
        }
    }
}

pub struct ProvisioningClient {
    http: Client,
    config: ProvisioningConfig,
    credential_store: CredentialStore,
    pending_store: PendingProvisioningStore,
}

impl ProvisioningClient {
    pub fn new() -> Result<Self> {
        Self::with_config(ProvisioningConfig::default())
    }

    pub fn with_config(config: ProvisioningConfig) -> Result<Self> {
        let http = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .context("failed to initialize collector provisioning HTTP client")?;

        Ok(Self {
            http,
            config,
            credential_store: CredentialStore,
            pending_store: PendingProvisioningStore,
        })
    }

    pub async fn provision(&self, activation_token: &str, device_name: &str) -> Result<Uuid> {
        validate_activation_token(activation_token)?;
        validate_device_name(device_name)?;

        let activation_hash = sha256_hex(activation_token.as_bytes());
        let secret = self.load_or_create_secret(&activation_hash)?;
        let secret_hash = sha256_hex(secret.as_bytes());
        let request = RedeemActivationRequest {
            token: activation_token,
            device_name,
            secret_hash: &secret_hash,
        };

        let response = self
            .http
            .post(&self.config.endpoint)
            .json(&request)
            .send()
            .await
            .context("failed to contact Mnemos collector provisioning endpoint")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let message = parse_server_error(&body)
                .unwrap_or_else(|| "provisioning request failed".to_owned());

            bail!("collector provisioning was rejected with {status}: {message}");
        }

        let redeemed: RedeemActivationResponse = response
            .json()
            .await
            .context("Mnemos returned an invalid collector provisioning response")?;
        let access_key = Zeroizing::new(format!("{}.{}", redeemed.credential_id, secret.as_str()));

        validate_access_key(access_key.as_str())?;
        self.credential_store
            .save(access_key.as_str())
            .context("collector credential was issued but could not be saved securely")?;
        self.pending_store.clear()?;

        Ok(redeemed.credential_id)
    }

    fn load_or_create_secret(&self, activation_hash: &str) -> Result<Zeroizing<String>> {
        if let Some(pending) = self.pending_store.load()?
            && pending.activation_hash == activation_hash
        {
            return Ok(Zeroizing::new(pending.secret));
        }

        let secret = generate_secret()?;
        let pending = PendingProvisioningSecret {
            activation_hash: activation_hash.to_owned(),
            secret: secret.to_string(),
        };

        self.pending_store
            .save(&pending)
            .context("failed to persist retry-safe collector provisioning state")?;

        Ok(secret)
    }
}

pub fn default_device_name() -> String {
    let raw = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Mnemos Collector".to_owned());
    let normalized: String = raw
        .chars()
        .filter(|character| {
            character.is_alphanumeric() || matches!(character, ' ' | '.' | '_' | '-')
        })
        .take(48)
        .collect();
    let normalized = normalized.trim();

    if normalized.is_empty() {
        "Mnemos Collector".to_owned()
    } else {
        normalized.to_owned()
    }
}

fn generate_secret() -> Result<Zeroizing<String>> {
    let mut bytes = Zeroizing::new([0_u8; SECRET_BYTES]);

    getrandom::fill(bytes.as_mut()).context("operating system CSPRNG is unavailable")?;

    let secret = Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_ref()));
    validate_secret(&secret)?;

    Ok(secret)
}

fn validate_activation_token(token: &str) -> Result<()> {
    if token.len() != ACTIVATION_TOKEN_LENGTH
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        bail!("collector activation token has an invalid format");
    }

    Ok(())
}

fn validate_device_name(device_name: &str) -> Result<()> {
    let trimmed = device_name.trim();

    if trimmed.is_empty() || trimmed.chars().count() > 48 {
        bail!("collector device name must contain between 1 and 48 characters");
    }

    if !trimmed
        .chars()
        .all(|character| character.is_alphanumeric() || matches!(character, ' ' | '.' | '_' | '-'))
    {
        bail!("collector device name contains unsupported characters");
    }

    Ok(())
}

fn sha256_hex(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn parse_server_error(body: &str) -> Option<String> {
    let error: ServerError = serde_json::from_str(body).ok()?;

    error.message.filter(|message| !message.trim().is_empty())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RedeemActivationRequest<'a> {
    token: &'a str,
    device_name: &'a str,
    secret_hash: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RedeemActivationResponse {
    credential_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct ServerError {
    message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_server_compatible_secret() {
        let secret = generate_secret().unwrap();

        assert_eq!(secret.len(), ACTIVATION_TOKEN_LENGTH);
        assert!(validate_secret(&secret).is_ok());
    }

    #[test]
    fn validates_activation_token_shape() {
        assert!(validate_activation_token("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopq").is_ok());
        assert!(validate_activation_token("short").is_err());
    }

    #[test]
    fn hashes_values_as_lowercase_sha256() {
        assert_eq!(
            sha256_hex(b"mnemos"),
            "606e9033fcb6ea658da54ddfdb93ae78d7ae4c51c49fa2f0503165f57020871c"
        );
    }

    #[test]
    fn normalizes_default_device_name_fallback() {
        let name = default_device_name();

        assert!(!name.is_empty());
        assert!(name.chars().count() <= 48);
    }
}
