use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use keyring::{Entry, Error as KeyringError};
use uuid::Uuid;

use crate::diagnostics;

const CREDENTIAL_DIRECTORY: &str = "credentials";
const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
const SECURITY_TOOL: &str = "/usr/bin/security";

pub(super) fn load(service: &str, account: &str) -> Result<Option<String>> {
    let path = credential_path(account)?;

    if let Some(value) = read_protected_file(&path)? {
        return Ok(Some(value));
    }

    match load_from_keyring(service, account) {
        Ok(Some(value)) => {
            persist_protected_file(&path, &value)?;
            diagnostics::info(
                "security",
                "Migrated macOS Collector credential into upgrade-safe local storage",
            );

            return Ok(Some(value));
        }
        Ok(None) => {}
        Err(error) => {
            diagnostics::warn(
                "security",
                format!("Direct macOS Keychain lookup failed; trying system migration: {error:#}"),
            );
        }
    }

    let Some(value) = load_with_security_tool(service, account)? else {
        return Ok(None);
    };

    persist_protected_file(&path, &value)?;
    diagnostics::info(
        "security",
        "Recovered legacy macOS Collector credential through the system Keychain tool",
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
        .args([
            "find-generic-password",
            "-s",
            service,
            "-a",
            account,
            "-w",
        ])
        .output()
        .context("failed to start the macOS Keychain migration helper")?;

    if output.status.success() {
        let value = String::from_utf8(output.stdout)
            .context("macOS Keychain migration returned non-UTF-8 credential data")?;
        let value = value.trim_end_matches(['\r', '\n']).to_owned();

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

    let value = fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    if value.is_empty() {
        bail!("Collector credential file is empty");
    }

    Ok(Some(value))
}

fn persist_protected_file(path: &Path, value: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("Collector credential path has no parent directory")?;

    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create {}", parent.display()))?;
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
        File::open(parent)
            .with_context(|| format!("failed to open {} for flushing", parent.display()))?
            .sync_all()
            .with_context(|| format!("failed to flush {}", parent.display()))?;

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
        let directory = std::env::temp_dir().join(format!("mnemos-macos-secret-{}", Uuid::now_v7()));
        let path = directory.join("credential");

        persist_protected_file(&path, "secret-value").unwrap();

        let loaded = read_protected_file(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;

        assert_eq!(loaded.as_deref(), Some("secret-value"));
        assert_eq!(mode, FILE_MODE);

        let _ = fs::remove_dir_all(directory);
    }
}
