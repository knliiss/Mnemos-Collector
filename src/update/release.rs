use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use directories::ProjectDirs;
use reqwest::Client;
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::update::version::CollectorVersion;

const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/knliiss/Mnemos-Collector/releases/latest/download/manifest.json";
const UPDATE_DIRECTORY: &str = "updates";
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_ROLLOUT_WINDOW: Duration = Duration::from_secs(30 * 60);
const SIGNED_PAYLOAD_DOMAIN: &str = "mnemos-collector-update-v1";

#[derive(Debug, Clone)]
pub struct UpdateConfig {
    pub manifest_url: String,
    pub public_key: Vec<u8>,
    pub request_timeout: Duration,
    pub max_artifact_bytes: usize,
    pub rollout_window: Duration,
}

impl UpdateConfig {
    pub fn from_build() -> Result<Option<Self>> {
        let Some(encoded_public_key) = option_env!("MNEMOS_COLLECTOR_UPDATE_PUBLIC_KEY") else {
            return Ok(None);
        };
        let public_key = decode_public_key(encoded_public_key)?;

        Ok(Some(Self {
            manifest_url: DEFAULT_MANIFEST_URL.to_owned(),
            public_key,
            request_timeout: Duration::from_secs(15),
            max_artifact_bytes: DEFAULT_MAX_ARTIFACT_BYTES,
            rollout_window: DEFAULT_ROLLOUT_WINDOW,
        }))
    }
}

#[derive(Debug, Clone)]
pub struct UpdateCandidate {
    pub version: CollectorVersion,
    pub platform: String,
    pub artifact: UpdateArtifact,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateArtifact {
    pub url: String,
    pub sha256: String,
    pub signature: String,
}

#[derive(Debug, Deserialize)]
struct UpdateManifest {
    version: String,
    artifacts: HashMap<String, UpdateArtifact>,
}

pub struct ReleaseClient {
    http: Client,
    config: UpdateConfig,
}

impl ReleaseClient {
    pub fn new(config: UpdateConfig) -> Result<Self> {
        validate_public_key(&config.public_key)?;

        if config.max_artifact_bytes == 0 {
            bail!("collector update artifact size limit must be positive");
        }

        if config.rollout_window.is_zero() {
            bail!("collector update rollout window must be positive");
        }

        let http = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .context("failed to initialize collector update HTTP client")?;

        Ok(Self { http, config })
    }

    pub async fn check(&self, current_version: CollectorVersion) -> Result<Option<UpdateCandidate>> {
        let response = self
            .http
            .get(&self.config.manifest_url)
            .send()
            .await
            .context("failed to fetch collector update manifest")?
            .error_for_status()
            .context("collector update manifest endpoint returned an error")?;
        let bytes = response
            .bytes()
            .await
            .context("failed to read collector update manifest")?;

        if bytes.len() > MAX_MANIFEST_BYTES {
            bail!("collector update manifest exceeds the maximum allowed size");
        }

        let manifest: UpdateManifest =
            serde_json::from_slice(&bytes).context("collector update manifest is invalid")?;

        select_candidate(
            manifest,
            current_version,
            platform_key(),
            &self.config.public_key,
        )
    }

    pub async fn stage(&self, candidate: &UpdateCandidate) -> Result<PathBuf> {
        let response = self
            .http
            .get(&candidate.artifact.url)
            .send()
            .await
            .context("failed to download collector update artifact")?
            .error_for_status()
            .context("collector update artifact endpoint returned an error")?;

        if response
            .content_length()
            .is_some_and(|length| length > self.config.max_artifact_bytes as u64)
        {
            bail!("collector update artifact exceeds the maximum allowed size");
        }

        let bytes = response
            .bytes()
            .await
            .context("failed to read collector update artifact")?;

        if bytes.len() > self.config.max_artifact_bytes {
            bail!("collector update artifact exceeds the maximum allowed size");
        }

        let actual_hash = sha256_hex(&bytes);

        if actual_hash != candidate.artifact.sha256 {
            bail!("collector update artifact SHA-256 does not match the signed manifest entry");
        }

        let update_directory = update_directory()?;
        fs::create_dir_all(&update_directory)
            .await
            .context("failed to create collector update staging directory")?;

        let staged_path = update_directory.join(staged_file_name(candidate.version));
        let partial_path = staged_path.with_extension(partial_extension());

        remove_if_exists(&partial_path).await?;
        remove_if_exists(&staged_path).await?;

        let mut file = File::create(&partial_path)
            .await
            .context("failed to create collector update staging file")?;

        file.write_all(&bytes)
            .await
            .context("failed to write collector update staging file")?;
        file.sync_all()
            .await
            .context("failed to flush collector update staging file")?;
        drop(file);

        preserve_executable_permissions(&partial_path).await?;

        fs::rename(&partial_path, &staged_path)
            .await
            .context("failed to finalize collector update staging file")?;

        Ok(staged_path)
    }

    pub fn rollout_delay(
        &self,
        collector_id: Uuid,
        version: CollectorVersion,
    ) -> Duration {
        deterministic_rollout_delay(
            collector_id,
            version,
            self.config.rollout_window,
        )
    }
}

fn select_candidate(
    manifest: UpdateManifest,
    current_version: CollectorVersion,
    platform: String,
    public_key: &[u8],
) -> Result<Option<UpdateCandidate>> {
    let version = CollectorVersion::from_str(&manifest.version)
        .context("collector update manifest version is invalid")?;

    if version <= current_version {
        return Ok(None);
    }

    let artifact = manifest
        .artifacts
        .get(&platform)
        .cloned()
        .with_context(|| format!("collector update manifest has no artifact for {platform}"))?;

    validate_sha256(&artifact.sha256)?;
    verify_signature(
        public_key,
        version,
        &platform,
        &artifact.sha256,
        &artifact.signature,
    )?;

    Ok(Some(UpdateCandidate {
        version,
        platform,
        artifact,
    }))
}

pub fn deterministic_rollout_delay(
    collector_id: Uuid,
    version: CollectorVersion,
    window: Duration,
) -> Duration {
    if window.is_zero() {
        return Duration::ZERO;
    }

    let seed = format!("{collector_id}:{version}");
    let digest = Sha256::digest(seed.as_bytes());
    let mut prefix = [0_u8; 8];

    prefix.copy_from_slice(&digest[..8]);

    let window_millis = window.as_millis();
    let offset_millis = u128::from(u64::from_be_bytes(prefix)) % window_millis;

    Duration::from_millis(offset_millis as u64)
}

fn verify_signature(
    public_key: &[u8],
    version: CollectorVersion,
    platform: &str,
    sha256: &str,
    encoded_signature: &str,
) -> Result<()> {
    validate_public_key(public_key)?;
    validate_sha256(sha256)?;

    let signature = STANDARD
        .decode(encoded_signature)
        .context("collector update signature is not valid base64")?;
    let payload = signed_payload(version, platform, sha256);

    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(payload.as_bytes(), &signature)
        .map_err(|_| anyhow!("collector update manifest signature verification failed"))
}

fn signed_payload(version: CollectorVersion, platform: &str, sha256: &str) -> String {
    format!("{SIGNED_PAYLOAD_DOMAIN}\n{version}\n{platform}\n{sha256}\n")
}

fn decode_public_key(encoded: &str) -> Result<Vec<u8>> {
    let public_key = STANDARD
        .decode(encoded)
        .context("collector update public key is not valid base64")?;

    validate_public_key(&public_key)?;

    Ok(public_key)
}

fn validate_public_key(public_key: &[u8]) -> Result<()> {
    if public_key.len() != 32 {
        bail!("collector update Ed25519 public key must be exactly 32 bytes");
    }

    Ok(())
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        bail!("collector update SHA-256 must be 64 lowercase hexadecimal characters");
    }

    Ok(())
}

fn sha256_hex(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn platform_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn update_directory() -> Result<PathBuf> {
    let project_dirs = ProjectDirs::from("rest", "knalis", "Mnemos Collector")
        .context("operating system does not expose a local data directory")?;

    Ok(project_dirs.data_local_dir().join(UPDATE_DIRECTORY))
}

fn staged_file_name(version: CollectorVersion) -> String {
    if cfg!(windows) {
        format!("mnemos-collector-{version}.staged.exe")
    } else {
        format!("mnemos-collector-{version}.staged")
    }
}

fn partial_extension() -> &'static str {
    if cfg!(windows) { "part.exe" } else { "part" }
}

async fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("failed to remove stale collector update staging file"),
    }
}

#[cfg(unix)]
async fn preserve_executable_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let current_executable =
        std::env::current_exe().context("failed to locate the running collector executable")?;
    let mode = std::fs::metadata(&current_executable)
        .context("failed to read collector executable permissions")?
        .permissions()
        .mode();

    fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .await
        .context("failed to preserve collector executable permissions")
}

#[cfg(not(unix))]
async fn preserve_executable_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PUBLIC_KEY: &str = "A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg=";
    const TEST_SIGNATURE: &str =
        "vzGIQpq6CybNm5M9wZ0Us6zA82mwxGObNm2yx1kNAzgmyk+xAfPdnBY4eVg8bp9ue/O4SB4WFgqZvqh4CIhrBg==";
    const TEST_SHA256: &str =
        "b2e64f429b4ede4f8873ae95cc9e31fcc21905da23acd0d42bb47c48db1e0acb";

    #[test]
    fn verifies_signed_release_metadata() {
        let public_key = decode_public_key(TEST_PUBLIC_KEY).unwrap();

        verify_signature(
            &public_key,
            CollectorVersion::new(0, 2, 0),
            "windows-x86_64",
            TEST_SHA256,
            TEST_SIGNATURE,
        )
        .unwrap();
    }

    #[test]
    fn rejects_metadata_changed_after_signing() {
        let public_key = decode_public_key(TEST_PUBLIC_KEY).unwrap();
        let result = verify_signature(
            &public_key,
            CollectorVersion::new(0, 2, 1),
            "windows-x86_64",
            TEST_SHA256,
            TEST_SIGNATURE,
        );

        assert!(result.is_err());
    }

    #[test]
    fn verifies_test_artifact_hash_vector() {
        assert_eq!(sha256_hex(b"collector-binary"), TEST_SHA256);
    }

    #[test]
    fn rollout_delay_is_stable_and_bounded() {
        let collector_id = Uuid::parse_str("019c1129-ef54-7000-8000-000000000220").unwrap();
        let version = CollectorVersion::new(0, 2, 0);
        let window = Duration::from_secs(30 * 60);

        let first = deterministic_rollout_delay(collector_id, version, window);
        let second = deterministic_rollout_delay(collector_id, version, window);

        assert_eq!(first, second);
        assert!(first < window);
    }

    #[test]
    fn ignores_release_that_is_not_newer() {
        let public_key = decode_public_key(TEST_PUBLIC_KEY).unwrap();
        let manifest = UpdateManifest {
            version: "0.1.0".to_owned(),
            artifacts: HashMap::new(),
        };

        let candidate = select_candidate(
            manifest,
            CollectorVersion::new(0, 1, 0),
            "windows-x86_64".to_owned(),
            &public_key,
        )
        .unwrap();

        assert!(candidate.is_none());
    }
}
