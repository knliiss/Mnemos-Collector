use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use uuid::Uuid;

use crate::update::CollectorVersion;

const INSTALL_DIRECTORY: &str = "bin";
const EXECUTABLE_NAME: &str = "mnemos-collector";
const VERSION_EXTENSION: &str = "version";
const REPLACEMENT_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const REPLACEMENT_RETRY_INTERVAL: Duration = Duration::from_millis(100);

pub struct Installation;

impl Installation {
    pub fn install_and_launch(activation_token: &str, device_name: Option<&str>) -> Result<()> {
        let target = installation_path()?;

        ensure_installed(&target, false)?;
        launch_installed(&target, Some(activation_token), device_name)
    }

    pub fn migrate_existing_and_launch() -> Result<()> {
        let target = installation_path()?;

        ensure_installed(&target, true)?;
        launch_installed(&target, None, None)
    }

    pub fn is_current_installation() -> Result<bool> {
        let target = installation_path()?;

        if !target.exists() {
            return Ok(false);
        }

        validate_existing_installation(&target)?;

        let current = std::env::current_exe()
            .context("failed to locate running collector executable")?
            .canonicalize()
            .context("failed to resolve running collector executable")?;
        let installed = target
            .canonicalize()
            .context("failed to resolve collector installation")?;

        Ok(current == installed)
    }

    pub fn record_current_version() -> Result<()> {
        let target = installation_path()?;

        if !target.exists() {
            return Ok(());
        }

        let current = std::env::current_exe()
            .context("failed to locate running collector executable")?
            .canonicalize()
            .context("failed to resolve running collector executable")?;
        let installed = target
            .canonicalize()
            .context("failed to resolve collector installation")?;

        if current != installed {
            return Ok(());
        }

        write_version_marker(&target, current_build_version()?)
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

fn ensure_installed(target: &Path, stop_existing_instance: bool) -> Result<()> {
    let current = std::env::current_exe()
        .context("failed to locate collector installer executable")?
        .canonicalize()
        .context("failed to resolve collector installer executable")?;
    let current_version = current_build_version()?;

    if target.exists() {
        validate_existing_installation(target)?;

        if installed_version_allows_reuse(read_version_marker(target)?, current_version) {
            return Ok(());
        }
    }

    install_current_executable(&current, target, current_version, stop_existing_instance)
}

fn installed_version_allows_reuse(
    installed_version: Option<CollectorVersion>,
    current_version: CollectorVersion,
) -> bool {
    installed_version.is_some_and(|installed_version| installed_version >= current_version)
}

fn install_current_executable(
    current: &Path,
    target: &Path,
    current_version: CollectorVersion,
    stop_existing_instance: bool,
) -> Result<()> {
    let parent = target
        .parent()
        .context("collector installation path has no parent directory")?;

    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;

    let temporary = temporary_installation_path(target)?;

    remove_if_exists(&temporary)?;

    if let Err(error) = fs::copy(current, &temporary) {
        let _ = remove_if_exists(&temporary);
        return Err(error).context("failed to copy collector into its installation directory");
    }

    preserve_executable_permissions(current, &temporary)?;
    sync_file(&temporary)?;

    let replacement_result = if target.exists() {
        replace_existing_installation(&temporary, target, stop_existing_instance)
    } else {
        fs::rename(&temporary, target).context("failed to finalize collector installation")
    };

    if let Err(error) = replacement_result {
        let _ = remove_if_exists(&temporary);
        return Err(error);
    }

    write_version_marker(target, current_version)?;

    Ok(())
}

fn replace_existing_installation(
    temporary: &Path,
    target: &Path,
    stop_existing_instance: bool,
) -> Result<()> {
    let backup = replacement_backup_path(target)?;

    remove_if_exists(&backup)?;

    if stop_existing_instance {
        request_existing_collector_shutdown();
    }

    move_existing_to_backup(target, &backup, stop_existing_instance)?;

    if let Err(error) = fs::rename(temporary, target) {
        let _ = fs::rename(&backup, target);
        return Err(error).context("failed to activate replacement collector installation");
    }

    let _ = remove_if_exists(&backup);

    Ok(())
}

fn move_existing_to_backup(target: &Path, backup: &Path, retry_locked_file: bool) -> Result<()> {
    let deadline = Instant::now() + REPLACEMENT_WAIT_TIMEOUT;

    loop {
        match fs::rename(target, backup) {
            Ok(()) => return Ok(()),
            Err(_) if retry_locked_file && Instant::now() < deadline => {
                std::thread::sleep(REPLACEMENT_RETRY_INTERVAL);
            }
            Err(error) => {
                return Err(error)
                    .context("failed to move the previous collector installation out of the way");
            }
        }
    }
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

fn current_build_version() -> Result<CollectorVersion> {
    CollectorVersion::from_str(env!("CARGO_PKG_VERSION"))
        .context("collector build version is invalid")
}

fn version_marker_path(target: &Path) -> PathBuf {
    target.with_extension(VERSION_EXTENSION)
}

fn read_version_marker(target: &Path) -> Result<Option<CollectorVersion>> {
    let marker = version_marker_path(target);
    let content = match fs::read_to_string(&marker) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", marker.display()));
        }
    };

    match CollectorVersion::from_str(content.trim()) {
        Ok(version) => Ok(Some(version)),
        Err(_) => Ok(None),
    }
}

fn write_version_marker(target: &Path, version: CollectorVersion) -> Result<()> {
    let marker = version_marker_path(target);
    let content = format!("{version}\n");

    fs::write(&marker, content).with_context(|| format!("failed to write {}", marker.display()))?;
    sync_file(&marker)
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

fn replacement_backup_path(target: &Path) -> Result<PathBuf> {
    let parent = target
        .parent()
        .context("collector installation path has no parent directory")?;
    let file_name = target
        .file_name()
        .context("collector installation path has no file name")?
        .to_string_lossy();

    Ok(parent.join(format!("{file_name}.previous-{}", Uuid::now_v7())))
}

fn launch_installed(
    target: &Path,
    activation_token: Option<&str>,
    device_name: Option<&str>,
) -> Result<()> {
    let mut command = Command::new(target);

    if let Some(activation_token) = activation_token {
        command.arg("--activation-token").arg(activation_token);
    }

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

#[cfg(target_os = "windows")]
fn request_existing_collector_shutdown() {
    use std::ptr::null;

    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_APP};

    const WINDOW_CLASS_NAME: &str = "MnemosCollectorShell";
    const WM_COLLECTOR_STOPPED: u32 = WM_APP + 3;

    let class_name = WINDOW_CLASS_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let hwnd = unsafe { FindWindowW(class_name.as_ptr(), null()) };

    if hwnd.is_null() {
        return;
    }

    unsafe {
        PostMessageW(hwnd, WM_COLLECTOR_STOPPED, 0, 0);
    }
}

#[cfg(not(target_os = "windows"))]
fn request_existing_collector_shutdown() {}

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
    fn version_marker_is_adjacent_to_stable_executable() {
        let target = installation_path().unwrap();
        let marker = version_marker_path(&target);

        assert_eq!(marker.parent(), target.parent());
        assert_eq!(
            marker.extension().and_then(|value| value.to_str()),
            Some("version")
        );
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

    #[test]
    fn legacy_install_without_version_marker_requires_replacement() {
        let current = CollectorVersion::new(0, 1, 6);

        assert!(!installed_version_allows_reuse(None, current));
    }

    #[test]
    fn known_newer_installation_is_never_downgraded() {
        let current = CollectorVersion::new(0, 1, 6);
        let installed = CollectorVersion::new(0, 2, 0);

        assert!(installed_version_allows_reuse(Some(installed), current));
    }

    #[test]
    fn legacy_installation_is_replaced_and_versioned() {
        let directory = test_directory("legacy-replacement");
        let source = directory.join("new-collector.exe");
        let target = directory.join("mnemos-collector.exe");
        let version = CollectorVersion::new(0, 1, 6);

        fs::create_dir_all(&directory).unwrap();
        fs::write(&source, b"new-collector").unwrap();
        fs::write(&target, b"old-collector").unwrap();

        install_current_executable(&source, &target, version, false).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new-collector");
        assert_eq!(read_version_marker(&target).unwrap(), Some(version));

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn failed_replacement_activation_restores_previous_executable() {
        let directory = test_directory("replacement-rollback");
        let target = directory.join("mnemos-collector.exe");
        let missing_replacement = directory.join("missing-replacement.exe");

        fs::create_dir_all(&directory).unwrap();
        fs::write(&target, b"previous-version").unwrap();

        let result = replace_existing_installation(&missing_replacement, &target, false);

        assert!(result.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"previous-version");

        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(windows)]
    #[test]
    fn running_executable_replacement_waits_for_file_unlock() {
        if std::env::var_os("MNEMOS_REPLACEMENT_LOCK_HELPER").is_some() {
            return;
        }

        let directory = test_directory("running-replacement");
        let target = directory.join("mnemos-collector-test.exe");
        let backup = directory.join("mnemos-collector-test.previous.exe");
        let test_executable = std::env::current_exe().unwrap();

        fs::create_dir_all(&directory).unwrap();
        fs::copy(test_executable, &target).unwrap();

        let mut child = Command::new(&target)
            .arg("replacement_lock_helper")
            .arg("--nocapture")
            .env("MNEMOS_REPLACEMENT_LOCK_HELPER", "1")
            .spawn()
            .unwrap();

        std::thread::sleep(Duration::from_millis(150));

        move_existing_to_backup(&target, &backup, true).unwrap();

        assert!(backup.exists());
        assert!(!target.exists());
        assert!(child.wait().unwrap().success());

        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(windows)]
    #[test]
    fn replacement_lock_helper() {
        if std::env::var_os("MNEMOS_REPLACEMENT_LOCK_HELPER").is_none() {
            return;
        }

        std::thread::sleep(Duration::from_millis(600));
    }

    fn test_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mnemos-install-{name}-{}", Uuid::now_v7()))
    }
}
