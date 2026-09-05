use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::catalog::{
    LocalizationLanguageSnapshot, LocalizationSnapshot, SaoLocalizationStore,
};
use crate::diagnostics;

const MANIFEST_URL: &str = "https://webdata.c7x.dev/client/lang.json";
const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const RETRY_INTERVAL: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const USER_AGENT: &str = concat!("Mnemos-Collector/", env!("CARGO_PKG_VERSION"));

static REFRESH_STARTED: AtomicBool = AtomicBool::new(false);

pub(super) fn load_cached_snapshot() -> Option<LocalizationSnapshot> {
    let path = cache_file_path().ok()?;
    let content = std::fs::read_to_string(&path).ok()?;

    match serde_json::from_str::<LocalizationSnapshot>(&content) {
        Ok(snapshot) if !snapshot.languages.is_empty() => Some(snapshot),
        Ok(_) => None,
        Err(error) => {
            diagnostics::warn(
                "localization",
                format!(
                    "Ignoring invalid cached SAO localization catalog at {}: {error}",
                    path.display()
                ),
            );
            None
        }
    }
}

pub(super) fn ensure_refresh_started(store: SaoLocalizationStore) {
    if REFRESH_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }

    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        REFRESH_STARTED.store(false, Ordering::Release);
        return;
    };

    runtime.spawn(async move {
        refresh_loop(store).await;
    });
}

async fn refresh_loop(store: SaoLocalizationStore) {
    let client = match Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            diagnostics::warn(
                "localization",
                format!("Failed to initialize SAO localization updater: {error}"),
            );
            return;
        }
    };

    loop {
        let delay = match refresh_once(&client, &store).await {
            Ok(updated) => {
                if updated {
                    diagnostics::info(
                        "localization",
                        "SAO localization catalog updated from Cristalix webdata",
                    );
                }

                REFRESH_INTERVAL
            }
            Err(error) => {
                diagnostics::warn(
                    "localization",
                    format!(
                        "SAO localization refresh failed; keeping last-known-good catalog and retrying: {error:#}"
                    ),
                );
                RETRY_INTERVAL
            }
        };

        tokio::time::sleep(delay).await;
    }
}

async fn refresh_once(client: &Client, store: &SaoLocalizationStore) -> Result<bool> {
    let manifest = client
        .get(MANIFEST_URL)
        .send()
        .await
        .context("failed to download SAO localization manifest")?
        .error_for_status()
        .context("SAO localization manifest returned an error status")?
        .json::<LocalizationManifest>()
        .await
        .context("failed to decode SAO localization manifest")?;

    let current = store.snapshot();
    let mut languages = HashMap::new();
    let mut changed = false;
    let mut refresh_failed = false;

    for language in manifest.langs {
        let Some(pie) = language.pies.into_iter().find(|pie| pie.pie_key == "sao") else {
            continue;
        };
        let existing = current.languages.get(&language.lang);

        if let Some(existing) = existing
            && existing.complete
            && existing.hash == pie.hash
        {
            languages.insert(language.lang, existing.clone());
            continue;
        }

        match download_language_pack(client, &pie).await {
            Ok(properties) => {
                languages.insert(
                    language.lang,
                    LocalizationLanguageSnapshot {
                        hash: pie.hash,
                        complete: true,
                        properties,
                    },
                );
                changed = true;
            }
            Err(error) => {
                refresh_failed = true;
                diagnostics::warn(
                    "localization",
                    format!(
                        "Failed to refresh SAO localization {}: {error:#}",
                        language.lang
                    ),
                );

                if let Some(existing) = existing {
                    languages.insert(language.lang, existing.clone());
                }
            }
        }
    }

    if languages.is_empty() {
        bail!("manifest does not contain any usable SAO localization packs");
    }

    let catalog_changed = changed || current.languages.len() != languages.len();

    if catalog_changed {
        let snapshot = LocalizationSnapshot { languages };
        persist_snapshot(&snapshot).await?;
        store.replace(snapshot)?;
    }

    if refresh_failed {
        bail!("one or more SAO localization packs could not be refreshed");
    }

    Ok(catalog_changed)
}

async fn download_language_pack(
    client: &Client,
    pie: &LocalizationPie,
) -> Result<HashMap<String, String>> {
    let bytes = client
        .get(&pie.url)
        .send()
        .await
        .with_context(|| format!("failed to download SAO localization pack {}", pie.hash))?
        .error_for_status()
        .with_context(|| format!("SAO localization pack {} returned an error status", pie.hash))?
        .bytes()
        .await
        .with_context(|| format!("failed to read SAO localization pack {}", pie.hash))?;

    let actual_hash = sha256_hex(&bytes);

    if actual_hash != pie.hash {
        bail!(
            "SAO localization pack hash mismatch: expected {}, got {}",
            pie.hash,
            actual_hash
        );
    }

    let pack = serde_json::from_slice::<LocalizationPack>(&bytes)
        .with_context(|| format!("failed to decode SAO localization pack {}", pie.hash))?;

    if pack.properties.is_empty() {
        bail!("SAO localization pack {} is empty", pie.hash);
    }

    Ok(pack.properties)
}

async fn persist_snapshot(snapshot: &LocalizationSnapshot) -> Result<()> {
    let path = cache_file_path()?;
    let directory = path
        .parent()
        .context("SAO localization cache path does not have a parent directory")?;
    tokio::fs::create_dir_all(directory)
        .await
        .context("failed to create SAO localization cache directory")?;

    let bytes = serde_json::to_vec(snapshot).context("failed to encode SAO localization cache")?;
    let temporary = path.with_extension("json.tmp");

    tokio::fs::write(&temporary, bytes)
        .await
        .context("failed to write temporary SAO localization cache")?;

    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        tokio::fs::remove_file(&path)
            .await
            .context("failed to replace previous SAO localization cache")?;
    }

    tokio::fs::rename(&temporary, &path)
        .await
        .context("failed to commit SAO localization cache")?;

    Ok(())
}

fn cache_file_path() -> Result<PathBuf> {
    let project_dirs = ProjectDirs::from("rest", "knalis", "Mnemos Collector")
        .context("operating system does not expose a Collector cache directory")?;

    Ok(project_dirs
        .cache_dir()
        .join("sao-localization")
        .join("catalog.json"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Deserialize)]
struct LocalizationManifest {
    langs: Vec<LocalizationManifestLanguage>,
}

#[derive(Debug, Deserialize)]
struct LocalizationManifestLanguage {
    lang: String,
    pies: Vec<LocalizationPie>,
}

#[derive(Debug, Deserialize)]
struct LocalizationPie {
    #[serde(rename = "pieKey")]
    pie_key: String,
    hash: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct LocalizationPack {
    properties: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    #[test]
    fn sha256_is_rendered_as_lowercase_hex() {
        assert_eq!(
            sha256_hex(b"mnemos"),
            "606e9033fcb6ea658da54ddfdb93ae78d7ae4c51c49fa2f0503165f57020871c",
        );
    }
}
