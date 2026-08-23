use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use uuid::Uuid;

const UPDATE_DIRECTORY: &str = "updates";
const HELPER_PREFIX: &str = "mnemos-collector-updater-";
const HEALTH_PREFIX: &str = "mnemos-collector-health-";

pub fn update_directory() -> Result<PathBuf> {
    let project_dirs = ProjectDirs::from("rest", "knalis", "Mnemos Collector")
        .context("operating system does not expose a local data directory")?;

    Ok(project_dirs.data_local_dir().join(UPDATE_DIRECTORY))
}

pub fn helper_path() -> Result<PathBuf> {
    let suffix = if cfg!(windows) { ".exe" } else { "" };

    Ok(update_directory()?.join(format!("{HELPER_PREFIX}{}{suffix}", Uuid::now_v7())))
}

pub fn health_file_path() -> Result<PathBuf> {
    Ok(update_directory()?.join(format!("{HEALTH_PREFIX}{}.ack", Uuid::now_v7())))
}

pub fn backup_path(current_executable: &Path, update_id: Uuid) -> Result<PathBuf> {
    let parent = current_executable
        .parent()
        .context("collector executable path has no parent directory")?;
    let file_name = current_executable
        .file_name()
        .context("collector executable path has no file name")?
        .to_string_lossy();

    Ok(parent.join(format!("{file_name}.rollback-{update_id}")))
}

pub fn require_safe_update_path(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!("collector update path must be absolute");
    }

    let expected_parent = update_directory()?;
    let parent = path
        .parent()
        .context("collector update path has no parent directory")?;

    if parent != expected_parent {
        bail!("collector update path is outside the collector update directory");
    }

    Ok(())
}

pub fn require_safe_helper_path(path: &Path) -> Result<()> {
    require_safe_update_path(path)?;

    let file_name = path
        .file_name()
        .context("collector helper path has no file name")?
        .to_string_lossy();

    if !file_name.starts_with(HELPER_PREFIX) {
        bail!("collector helper path has an invalid file name");
    }

    Ok(())
}

pub fn require_safe_health_path(path: &Path) -> Result<()> {
    require_safe_update_path(path)?;

    let file_name = path
        .file_name()
        .context("collector health path has no file name")?
        .to_string_lossy();

    if !file_name.starts_with(HEALTH_PREFIX) || !file_name.ends_with(".ack") {
        bail!("collector health path has an invalid file name");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_stays_next_to_running_binary_and_is_unique() {
        let current = if cfg!(windows) {
            PathBuf::from(r"C:\Mnemos\mnemos-collector.exe")
        } else {
            PathBuf::from("/opt/mnemos/mnemos-collector")
        };
        let first_id = Uuid::parse_str("019c1129-ef54-7000-8000-000000000301").unwrap();
        let second_id = Uuid::parse_str("019c1129-ef54-7000-8000-000000000302").unwrap();

        let first = backup_path(&current, first_id).unwrap();
        let second = backup_path(&current, second_id).unwrap();

        assert_eq!(first.parent(), current.parent());
        assert_ne!(first, second);
        assert!(first.to_string_lossy().contains(".rollback-"));
    }
}
