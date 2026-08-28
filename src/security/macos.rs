use std::fs::{self, OpenOptions, Permissions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use directories::BaseDirs;
use uuid::Uuid;

const APPLICATION_DIRECTORY: &str = "Mnemos Collector";
const CREDENTIALS_DIRECTORY: &str = "credentials";
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;

pub fn load(account: &str) -> Result<Option<String>> {
    let path = credential_path(account)?;

    load_from_path(&path)
}

pub fn save(account: &str, value: &str) -> Result<()> {
    let path = credential_path(account)?;

    save_to_path(&path, value)
}

pub fn delete(account: &str) -> Result<()> {
    let path = credential_path(account)?;

    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!("failed to delete macOS collector credential {}", path.display())
        }),
    }
}

fn credential_path(account: &str) -> Result<PathBuf> {
    validate_account_name(account)?;

    let base_directories = BaseDirs::new().context("macOS user data directory is unavailable")?;

    Ok(base_directories
        .data_dir()
        .join(APPLICATION_DIRECTORY)
        .join(CREDENTIALS_DIRECTORY)
        .join(format!("{account}.secret")))
}

fn validate_account_name(account: &str) -> Result<()> {
    if account.is_empty()
        || !account
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("macOS collector credential account name is invalid");
    }

    Ok(())
}

fn load_from_path(path: &Path) -> Result<Option<String>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to inspect macOS collector credential {}", path.display())
            });
        }
    };

    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "macOS collector credential path is not a regular file: {}",
            path.display()
        );
    }

    fs::set_permissions(path, Permissions::from_mode(PRIVATE_FILE_MODE)).with_context(|| {
        format!(
            "failed to enforce private permissions on macOS collector credential {}",
            path.display()
        )
    })?;

    fs::read_to_string(path)
        .map(Some)
        .with_context(|| format!("failed to read macOS collector credential {}", path.display()))
}

fn save_to_path(path: &Path, value: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("macOS collector credential path has no parent directory")?;

    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create macOS collector credential directory {}",
            parent.display()
        )
    })?;
    fs::set_permissions(parent, Permissions::from_mode(PRIVATE_DIRECTORY_MODE)).with_context(
        || {
            format!(
                "failed to secure macOS collector credential directory {}",
                parent.display()
            )
        },
    )?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("macOS collector credential filename is invalid")?;
    let temporary_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::now_v7()));

    let write_result = write_temporary_file(&temporary_path, value).and_then(|()| {
        fs::rename(&temporary_path, path).with_context(|| {
            format!(
                "failed to atomically replace macOS collector credential {}",
                path.display()
            )
        })
    });

    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary_path);

        return Err(error);
    }

    fs::set_permissions(path, Permissions::from_mode(PRIVATE_FILE_MODE)).with_context(|| {
        format!(
            "failed to enforce private permissions on macOS collector credential {}",
            path.display()
        )
    })
}

fn write_temporary_file(path: &Path, value: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE_MODE)
        .open(path)
        .with_context(|| {
            format!(
                "failed to create temporary macOS collector credential {}",
                path.display()
            )
        })?;

    file.write_all(value.as_bytes()).with_context(|| {
        format!(
            "failed to write temporary macOS collector credential {}",
            path.display()
        )
    })?;
    file.sync_all().with_context(|| {
        format!(
            "failed to flush temporary macOS collector credential {}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_account_names() {
        assert!(validate_account_name("collector-access-key").is_ok());
        assert!(validate_account_name("pending_provisioning").is_ok());
        assert!(validate_account_name("../credential").is_err());
        assert!(validate_account_name("").is_err());
    }

    #[test]
    fn private_file_store_persists_and_replaces_credentials() {
        let root = std::env::temp_dir().join(format!("mnemos-credential-{}", Uuid::now_v7()));
        let path = root.join("collector-access-key.secret");

        save_to_path(&path, "first").unwrap();
        assert_eq!(load_from_path(&path).unwrap().as_deref(), Some("first"));

        save_to_path(&path, "second").unwrap();
        assert_eq!(load_from_path(&path).unwrap().as_deref(), Some("second"));

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, PRIVATE_FILE_MODE);

        fs::remove_dir_all(root).unwrap();
    }
}
