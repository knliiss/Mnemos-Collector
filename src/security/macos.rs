use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use keyring::{Entry, Error as KeyringError};
use uuid::Uuid;

use crate::diagnostics;

use super::credential_id_from_access_key;

const CREDENTIAL_DIRECTORY: &str = "credentials";
const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
const SECURITY_TOOL: &str = "/usr/bin/security";

pub(super) fn load_access_key(service: &str, account: &str) -> Result<Option<String>> {
    let path = credential_path(account)?;
    let protected = read_protected_file(&path)?;
    let system = load_system_value(service, account)?;
    let selected = select_preferred_access_key(protected.as_deref(), system.as_deref())?;

    let Some(selected) = selected else {
        return Ok(None);
    };

    if protected.as_deref() != Some(selected.as_str()) {
        persist_protected_file(&path, &selected)?;
    }

    if system.as_deref() != Some(selected.as_str()) {
        mirror_to_keyring(service, account, &selected);
    }

    if protected.is_some() && system.is_some() && protected.as_deref() != system.as_deref() {
        let selected_id = credential_id_from_access_key(&selected)?;

        diagnostics::warn(
            "security",
            format!(
                "Reconciled divergent macOS Collector credentials; selected newest credential {selected_id}"
            ),
        );
    }

    Ok(Some(selected))
}

pub(super) fn load(service: &str, account: &str) -> Result<Option<String>> {
    let path = credential_path(account)?;

    if let Some(value) = read_protected_file(&path)? {
        return Ok(Some(value));
    }

    let Some(value) = load_system_value(service, account)? else {
        return Ok(None);
    };

    persist_protected_file(&path, &value)?;
    diagnostics::info(
        "security",
        "Recovered macOS Collector secret into upgrade-safe local storage",
    );

    Ok(Some(value))
}

pub(super) fn save(service: &str, account: &str, value: &str) -> Result<()> {
    let path = credential_path(account)?;

    persist_protected_file(&path, value)?;
    mirror_to_keyring(service, account, value);

    Ok(())
}

pub(super) fn delete(service: &str, account: &str) -> Result<()> {
    let path = credential_path(account)?;

    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("failed to remove {}", path.display()));
        }
    }

    match Entry::new(service, account) {
        Ok(entry) => match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => {}
            Err(error) => diagnostics::warn(
                "security",
                format!("Failed to remove compatibility Keychain entry: {error}"),
            ),
        },
        Err(error) => diagnostics::warn(
            "security",
            format!("Failed to open compatibility Keychain entry for removal: {error}"),
        ),
    }

    Ok(())
}

fn load_system_value(service: &str, account: &str) -> Result<Option<String>> {
    match load_from_keyring(service, account) {
        Ok(Some(value)) => return Ok(Some(value)),
        Ok(None) => {}
        Err(error) => {
            diagnostics::warn(
                "security",
                format!("Direct macOS Keychain lookup failed; trying system migration: {error:#}"),
            );
        }
    }

    load_with_security_tool(service, account)
}

fn select_preferred_access_key(
    protected: Option<&str>,
    system: Option<&str>,
) -> Result<Option<String>> {
    match (protected, system) {
        (None, None) => Ok(None),
        (Some(value), None) | (None, Some(value)) => {
            credential_id_from_access_key(value)?;
            Ok(Some(value.to_owned()))
        }
        (Some(protected), Some(system)) if protected == system => {
            credential_id_from_access_key(protected)?;
            Ok(Some(protected.to_owned()))
        }
        (Some(protected), Some(system)) => {
            let protected_id = credential_id_from_access_key(protected)?;
            let system_id = credential_id_from_access_key(system)?;

            if system_id > protected_id {
                Ok(Some(system.to_owned()))
            } else {
                Ok(Some(protected.to_owned()))
            }
        }
    }
}

fn credential_path(account: &str) -> Result<PathBuf> {
    let project_dirs = ProjectDirs::from("rest", "knalis", "Mnemos Collector")
        .context("macOS does not expose the Collector application data directory")?;

    Ok(project_dirs
        .data_local_dir()
        .join(CREDENTIAL_DIRECTORY)
        .join(account))
}

fn load_from_keyring(service: &str, account: &str) -> Result<Option<String>> {
    let entry = Entry::new(service, account).context("failed to open the macOS Keychain entry")?;

    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(error).context("failed to read the macOS Keychain entry"),
    }
}

fn load_with_security_tool(service: &str, account: &str) -> Result<Option<String>> {
    let output = Command::new(SECURITY_TOOL)
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .output()
        .context("failed to start the macOS Keychain migration helper")?;

    if output.status.success() {
        let value = String::from_utf8(output.stdout)
            .context("macOS Keychain migration returned non-UTF-8 credential data")?;
        let value = value.trim_end_matches(&['\r', '\n'][..]).to_owned();

        if value.is_empty() {
            bail!("macOS Keychain migration returned an empty credential");
        }

        return Ok(Some(value));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let missing = stderr.contains("could not be found")
        || stderr.contains("The specified item could not be found")
        || stderr.contains("SecKeychainSearchCopyNext: The specified item could not be found");

    if !missing {
        diagnostics::warn(
            "security",
            format!(
                "macOS Keychain migration helper could not recover the credential: {}",
                stderr.trim()
            ),
        );
    }

    Ok(None)
}

fn mirror_to_keyring(service: &str, account: &str, value: &str) {
    let entry = match Entry::new(service, account) {
        Ok(entry) => entry,
        Err(error) => {
            diagnostics::warn(
                "security",
                format!("Failed to open compatibility macOS Keychain entry: {error}"),
            );
            return;
        }
    };

    if let Err(error) = entry.set_password(value) {
        diagnostics::warn(
            "security",
            format!("Failed to mirror Collector credential into macOS Keychain: {error}"),
        );
        return;
    }

    match entry.get_password() {
        Ok(stored) if stored == value => {}
        Ok(_) => diagnostics::warn(
            "security",
            "macOS Keychain compatibility write verification returned different data",
        ),
        Err(error) => diagnostics::warn(
            "security",
            format!("macOS Keychain compatibility write could not be verified: {error}"),
        ),
    }
}

fn read_protected_file(path: &Path) -> Result<Option<String>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };

    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("Collector credential path is not a regular file");
    }

    if metadata.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(FILE_MODE))
            .with_context(|| format!("failed to secure {}", path.display()))?;
    }

    let value =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;

    if value.is_empty() {
        bail!("Collector credential file is empty");
    }

    Ok(Some(value))
}

fn persist_protected_file(path: &Path, value: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("Collector credential path has no parent directory")?;

    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    fs::set_permissions(parent, fs::Permissions::from_mode(DIRECTORY_MODE))
        .with_context(|| format!("failed to secure {}", parent.display()))?;

    let temporary = parent.join(format!(".credential-{}.tmp", Uuid::now_v7()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(&temporary)
        .with_context(|| format!("failed to create {}", temporary.display()))?;

    let write_result = (|| -> Result<()> {
        file.write_all(value.as_bytes())
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to flush {}", temporary.display()))?;
        drop(file);

        fs::rename(&temporary, path)
            .with_context(|| format!("failed to activate {}", path.display()))?;
        fs::set_permissions(path, fs::Permissions::from_mode(FILE_MODE))
            .with_context(|| format!("failed to secure {}", path.display()))?;

        let persisted = fs::read_to_string(path)
            .with_context(|| format!("failed to verify {}", path.display()))?;

        if persisted != value {
            bail!("Collector credential persistence verification failed");
        }

        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }

    write_result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_file_round_trip_uses_private_permissions() {
        let directory =
            std::env::temp_dir().join(format!("mnemos-macos-secret-{}", Uuid::now_v7()));
        let path = directory.join("credential");

        persist_protected_file(&path, "secret-value").unwrap();

        let loaded = read_protected_file(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;

        assert_eq!(loaded.as_deref(), Some("secret-value"));
        assert_eq!(mode, FILE_MODE);

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn newer_system_access_key_wins_over_stale_protected_copy() {
        let protected = format!(
            "{}.{}",
            "019c1129-ef54-7000-8000-000000000100",
            "a".repeat(43),
        );
        let system = format!(
            "{}.{}",
            "019c1129-ef54-7000-8000-000000000200",
            "b".repeat(43),
        );

        let selected =
            select_preferred_access_key(Some(protected.as_str()), Some(system.as_str())).unwrap();

        assert_eq!(selected.as_deref(), Some(system.as_str()));
    }

    #[test]
    fn newer_protected_access_key_is_not_replaced_by_stale_keychain_copy() {
        let protected = format!(
            "{}.{}",
            "019c1129-ef54-7000-8000-000000000300",
            "c".repeat(43),
        );
        let system = format!(
            "{}.{}",
            "019c1129-ef54-7000-8000-000000000200",
            "d".repeat(43),
        );

        let selected =
            select_preferred_access_key(Some(protected.as_str()), Some(system.as_str())).unwrap();

        assert_eq!(selected.as_deref(), Some(protected.as_str()));
    }
}
