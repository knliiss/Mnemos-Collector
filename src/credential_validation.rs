use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::header::AUTHORIZATION;
use reqwest::{Client, StatusCode};

use crate::security::validate_access_key;

const DEFAULT_ENDPOINT: &str =
    "https://api.knalis.rest/api/v1/realtime/collector-credentials/validate";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialValidationStatus {
    Active,
    Rejected,
}

pub struct CredentialValidationClient {
    http: Client,
    endpoint: String,
}

impl CredentialValidationClient {
    pub fn new() -> Result<Self> {
        Self::with_endpoint(DEFAULT_ENDPOINT)
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Result<Self> {
        let http = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .context("failed to initialize collector credential validation client")?;

        Ok(Self {
            http,
            endpoint: endpoint.into(),
        })
    }

    pub async fn validate(&self, access_key: &str) -> Result<CredentialValidationStatus> {
        validate_access_key(access_key)?;

        let response = self
            .http
            .get(&self.endpoint)
            .header(AUTHORIZATION, format!("Collector {access_key}"))
            .send()
            .await
            .context("failed to validate collector credential with Mnemos")?;

        classify_status(response.status())
    }
}

fn classify_status(status: StatusCode) -> Result<CredentialValidationStatus> {
    match status {
        StatusCode::NO_CONTENT => Ok(CredentialValidationStatus::Active),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            Ok(CredentialValidationStatus::Rejected)
        }
        status => bail!("collector credential validation returned unexpected HTTP status {status}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_no_content_as_active_credential() {
        assert_eq!(
            classify_status(StatusCode::NO_CONTENT).unwrap(),
            CredentialValidationStatus::Active,
        );
    }

    #[test]
    fn treats_authentication_failures_as_rejected_credentials() {
        assert_eq!(
            classify_status(StatusCode::UNAUTHORIZED).unwrap(),
            CredentialValidationStatus::Rejected,
        );
        assert_eq!(
            classify_status(StatusCode::FORBIDDEN).unwrap(),
            CredentialValidationStatus::Rejected,
        );
    }

    #[test]
    fn does_not_revoke_local_credentials_for_transient_server_failures() {
        assert!(classify_status(StatusCode::BAD_GATEWAY).is_err());
        assert!(classify_status(StatusCode::SERVICE_UNAVAILABLE).is_err());
    }
}
