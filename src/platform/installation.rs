use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use uuid::Uuid;

const INSTALL_DIRECTORY: &str = "bin";
const EXECUTABLE_NAME: &str = "mnemos-collector";

pub struct Installation;

impl Installation {
    pub fn install_and_launch(activation_token: &str, device_name: Option<&str>) -> Result<()> {
        let target = installation_path()?;

        ensure_installed(&target)?;
        launch_installed(&target, activation_token, device_name)
    }
}

fn installation_path() -> Result<PathBuf> {
    let project_dirs = ProjectDirs::from("rest", "knalis", "Mnemos Collector")
        .context("operating system does not expose a local data directory")?;
    let executable_name = if cfg!(windows) {
        format!("{EXECUTABLE_NAME}.exe")
    } else {
        EXECUTABLE_NAME.to_owned()
    };

    Ok(project_dirs
        .data_local_dir()
        .join(INSTALL_DIRECTORY)
        .join(executable_name))
}

fn ensure_installed(target: &Path) -> Result<()> {
    let current = std::env::current_exe()
        .context("failed to locate collector installer executable")?
        .canonicalize()
        .context("failed to resolve collector installer executable")?;

    if target.exists() {
        validate_existing_installation(target)?;

        let installed = target
            .canonicalize()
            .context("failed to resolve existing collector installation")?;

        if installed == current {
            return Ok(());
        }

        return Ok(());
    }

    let parent = target
        .parent()
        .context("collector installation path has no parent directory")?;

    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;

    let temporary = temporary_installation_path(target)?;

    remove_if_exists(&temporary)?;

    if let Err(error) = fs::copy(&current, &temporary) {
        let _ = remove_if_exists(&temporary);
        return Err(error).context("failed to copy collector into its installation directory");
    }

    preserve_executable_permissions(&current, &temporary)?;
    sync_file(&temporary)?;

    if let Err(error) = fs::rename(&temporary, target) {
        let _ = remove_if_exists(&temporary);
        return Err(error).context("failed to finalize collector installation");
    }

    Ok(())
}

fn validate_existing_installation(target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(target)
        .with_context(|| format!("failed to inspect {}", target.display()))?;

    if metadata.file_type().is_symlink() {
        bail!("collector installation path must not be a symbolic link");
    }

    if !metadata.is_file() {
        bail!("collector installation path is not a regular file");
    }

    Ok(())
}

fn temporary_installation_path(target: &Path) -> Result<PathBuf> {
    let parent = target
        .parent()
        .context("collector installation path has no parent directory")?;
    let file_name = target
        .file_name()
        .context("collector installation path has no file name")?
        .to_string_lossy();

    Ok(parent.join(format!("{file_name}.install-{}", Uuid::now_v7())))
}

fn launch_installed(
    target: &Path,
    activation_token: &str,
    device_name: Option<&str>,
) -> Result<()> {
    let mut command = Command::new(target);

    command.arg("--activation-token").arg(activation_token);

    if let Some(device_name) = device_name {
        command.arg("--device-name").arg(device_name);
    }

    command.spawn().with_context(|| {
        format!(
            "failed to launch installed collector at {}",
            target.display()
        )
    })?;

    Ok(())
}

fn sync_file(path: &Path) -> Result<()> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open {} for flushing", path.display()))?
        .sync_all()
        .with_context(|| format!("failed to flush {}", path.display()))
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

#[cfg(unix)]
fn preserve_executable_permissions(source: &Path, target: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(source)
        .with_context(|| format!("failed to read {} permissions", source.display()))?
        .permissions()
        .mode();

    fs::set_permissions(target, fs::Permissions::from_mode(mode))
        .with_context(|| format!("failed to set {} permissions", target.display()))
}

#[cfg(not(unix))]
fn preserve_executable_permissions(_source: &Path, _target: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_path_uses_stable_collector_name() {
        let path = installation_path().unwrap();
        let file_name = path.file_name().unwrap().to_string_lossy();

        if cfg!(windows) {
            assert_eq!(file_name, "mnemos-collector.exe");
        } else {
            assert_eq!(file_name, "mnemos-collector");
        }
    }

    #[test]
    fn temporary_installation_path_is_unique_and_adjacent() {
        let target = installation_path().unwrap();
        let first = temporary_installation_path(&target).unwrap();
        let second = temporary_installation_path(&target).unwrap();

        assert_eq!(first.parent(), target.parent());
        assert_eq!(second.parent(), target.parent());
        assert_ne!(first, second);
        assert!(first.to_string_lossy().contains(".install-"));
    }
}
