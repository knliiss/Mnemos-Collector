use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use sysinfo::{Pid, ProcessesToUpdate, System};
use uuid::Uuid;

use crate::update::integrity::{sha256_file, validate_sha256};
use crate::update::paths::{
    backup_path, health_file_path, helper_path, install_path, require_safe_health_path,
    require_safe_helper_path, require_safe_update_path, update_directory,
};

const APPLY_UPDATE_FLAG: &str = "--apply-update";
const CURRENT_EXECUTABLE_ARGUMENT: &str = "--current-executable";
const STAGED_EXECUTABLE_ARGUMENT: &str = "--staged-executable";
const BACKUP_EXECUTABLE_ARGUMENT: &str = "--backup-executable";
const EXPECTED_SHA256_ARGUMENT: &str = "--expected-sha256";
const PARENT_PID_ARGUMENT: &str = "--parent-pid";
pub(crate) const HEALTH_FILE_ARGUMENT: &str = "--health-file";
pub(crate) const HEALTH_TOKEN_ARGUMENT: &str = "--health-token";
pub(crate) const CLEANUP_HELPER_ARGUMENT: &str = "--cleanup-helper";

const REPLACEMENT_TIMEOUT: Duration = Duration::from_secs(15);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(15);
const HEALTH_STABILITY_WINDOW: Duration = Duration::from_secs(2);
const RETRY_INTERVAL: Duration = Duration::from_millis(100);
const HELPER_CLEANUP_ATTEMPTS: usize = 40;
const HELPER_CLEANUP_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyUpdateCommand {
    current_executable: PathBuf,
    staged_executable: PathBuf,
    backup_executable: PathBuf,
    health_file: PathBuf,
    health_token: Uuid,
    expected_sha256: String,
    parent_pid: u32,
}

impl ApplyUpdateCommand {
    pub fn parse_environment() -> Result<Option<Self>> {
        Self::parse(std::env::args().skip(1))
    }

    pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Option<Self>> {
        let arguments = arguments.into_iter().collect::<Vec<_>>();

        if arguments.first().map(String::as_str) != Some(APPLY_UPDATE_FLAG) {
            return Ok(None);
        }

        let mut current_executable = None;
        let mut staged_executable = None;
        let mut backup_executable = None;
        let mut health_file = None;
        let mut health_token = None;
        let mut expected_sha256 = None;
        let mut parent_pid = None;
        let mut index = 1;

        while index < arguments.len() {
            let argument_name = arguments[index].as_str();
            let value = arguments
                .get(index + 1)
                .with_context(|| format!("{argument_name} requires a value"))?
                .clone();

            match argument_name {
                CURRENT_EXECUTABLE_ARGUMENT => {
                    set_once(
                        &mut current_executable,
                        PathBuf::from(value),
                        CURRENT_EXECUTABLE_ARGUMENT,
                    )?;
                }
                STAGED_EXECUTABLE_ARGUMENT => {
                    set_once(
                        &mut staged_executable,
                        PathBuf::from(value),
                        STAGED_EXECUTABLE_ARGUMENT,
                    )?;
                }
                BACKUP_EXECUTABLE_ARGUMENT => {
                    set_once(
                        &mut backup_executable,
                        PathBuf::from(value),
                        BACKUP_EXECUTABLE_ARGUMENT,
                    )?;
                }
                HEALTH_FILE_ARGUMENT => {
                    set_once(&mut health_file, PathBuf::from(value), HEALTH_FILE_ARGUMENT)?;
                }
                HEALTH_TOKEN_ARGUMENT => {
                    let token = Uuid::parse_str(&value)
                        .context("collector update health token is not a UUID")?;

                    set_once(&mut health_token, token, HEALTH_TOKEN_ARGUMENT)?;
                }
                EXPECTED_SHA256_ARGUMENT => {
                    validate_sha256(&value)?;
                    set_once(&mut expected_sha256, value, EXPECTED_SHA256_ARGUMENT)?;
                }
                PARENT_PID_ARGUMENT => {
                    let pid = value
                        .parse::<u32>()
                        .context("collector update parent PID is invalid")?;

                    set_once(&mut parent_pid, pid, PARENT_PID_ARGUMENT)?;
                }
                _ => bail!("unsupported collector updater argument: {argument_name}"),
            }

            index += 2;
        }

        let command = Self {
            current_executable: require_argument(current_executable, CURRENT_EXECUTABLE_ARGUMENT)?,
            staged_executable: require_argument(staged_executable, STAGED_EXECUTABLE_ARGUMENT)?,
            backup_executable: require_argument(backup_executable, BACKUP_EXECUTABLE_ARGUMENT)?,
            health_file: require_argument(health_file, HEALTH_FILE_ARGUMENT)?,
            health_token: require_argument(health_token, HEALTH_TOKEN_ARGUMENT)?,
            expected_sha256: require_argument(expected_sha256, EXPECTED_SHA256_ARGUMENT)?,
            parent_pid: require_argument(parent_pid, PARENT_PID_ARGUMENT)?,
        };

        command.validate()?;

        Ok(Some(command))
    }

    pub fn run(self) -> Result<()> {
        self.validate()?;

        let helper =
            std::env::current_exe().context("failed to locate collector updater helper")?;
        require_safe_helper_path(&helper)?;

        wait_for_parent_exit(self.parent_pid, REPLACEMENT_TIMEOUT)?;
        remove_if_exists(&self.health_file)?;

        let install_executable = install_path(&self.current_executable, self.health_token)?;

        if let Err(error) = self.prepare_install_executable(&install_executable) {
            restart_current(&self.current_executable, &helper)?;
            return Err(error)
                .context("failed to prepare collector update beside the installed binary");
        }

        if let Err(error) = move_current_to_backup(
            &self.current_executable,
            &self.backup_executable,
            REPLACEMENT_TIMEOUT,
        ) {
            let _ = remove_if_exists(&install_executable);
            restart_current(&self.current_executable, &helper)?;
            return Err(error);
        }

        if let Err(error) = fs::rename(&install_executable, &self.current_executable) {
            let rollback_result = fs::rename(&self.backup_executable, &self.current_executable);

            match rollback_result {
                Ok(()) => {
                    restart_current(&self.current_executable, &helper)?;
                    return Err(error).context("failed to install prepared collector executable");
                }
                Err(rollback_error) => {
                    return Err(anyhow!(
                        "failed to install prepared collector executable: {error}; rollback also failed: {rollback_error}"
                    ));
                }
            }
        }

        let _ = remove_if_exists(&self.staged_executable);

        let mut child = match launch_collector(
            &self.current_executable,
            Some((&self.health_file, self.health_token)),
            &helper,
        ) {
            Ok(child) => child,
            Err(error) => {
                restore_backup(&self.current_executable, &self.backup_executable)?;
                restart_current(&self.current_executable, &helper)?;

                return Err(error).context("updated collector could not be started");
            }
        };

        let health_error = self.verify_updated_process(&mut child);

        if health_error.is_none() {
            let _ = remove_if_exists(&self.health_file);
            let _ = remove_if_exists(&self.backup_executable);
            return Ok(());
        }

        terminate_child(&mut child)?;
        remove_if_exists(&self.health_file)?;
        restore_backup(&self.current_executable, &self.backup_executable)?;
        restart_current(&self.current_executable, &helper)?;

        Err(health_error
            .expect("unhealthy collector did not produce an update error")
            .context("updated collector failed health verification; rolled back to the previous executable"))
    }

    fn prepare_install_executable(&self, install_executable: &Path) -> Result<()> {
        let staged_hash = sha256_file(&self.staged_executable)?;

        if staged_hash != self.expected_sha256 {
            bail!("staged collector update SHA-256 changed after download verification");
        }

        remove_if_exists(install_executable)?;

        if let Err(error) = fs::copy(&self.staged_executable, install_executable) {
            let _ = remove_if_exists(install_executable);
            return Err(error)
                .context("failed to copy collector update beside installed executable");
        }

        preserve_executable_permissions(&self.staged_executable, install_executable)?;
        sync_file(install_executable)?;

        let installed_hash = sha256_file(install_executable)?;

        if installed_hash != self.expected_sha256 {
            remove_if_exists(install_executable)?;
            bail!("prepared collector update SHA-256 does not match signed release metadata");
        }

        Ok(())
    }

    fn verify_updated_process(&self, child: &mut Child) -> Option<anyhow::Error> {
        match wait_for_health(child, &self.health_file, self.health_token, HEALTH_TIMEOUT) {
            Ok(true) => {}
            Ok(false) => return Some(anyhow!("updated collector did not acknowledge startup")),
            Err(error) => return Some(error.context("collector startup health check failed")),
        }

        thread::sleep(HEALTH_STABILITY_WINDOW);

        match child.try_wait() {
            Ok(None) => None,
            Ok(Some(status)) => Some(anyhow!(
                "updated collector exited during the startup stability window with status {status}"
            )),
            Err(error) => Some(error.into()),
        }
    }

    fn validate(&self) -> Result<()> {
        for path in [
            &self.current_executable,
            &self.staged_executable,
            &self.backup_executable,
            &self.health_file,
        ] {
            if !path.is_absolute() {
                bail!("collector updater paths must be absolute");
            }
        }

        require_safe_update_path(&self.staged_executable)?;
        require_safe_health_path(&self.health_file)?;
        validate_sha256(&self.expected_sha256)?;

        if self.parent_pid == 0 || self.parent_pid == std::process::id() {
            bail!("collector update parent PID is invalid");
        }

        let expected_backup = backup_path(&self.current_executable, self.health_token)?;

        if self.backup_executable != expected_backup {
            bail!("collector rollback path does not match the running executable");
        }

        if self.current_executable == self.staged_executable
            || self.current_executable == self.backup_executable
            || self.staged_executable == self.backup_executable
        {
            bail!("collector update paths must refer to different files");
        }

        Ok(())
    }
}

pub struct UpdateHandoff;

impl UpdateHandoff {
    pub fn start(staged_executable: &Path, expected_sha256: &str) -> Result<()> {
        require_safe_update_path(staged_executable)?;
        validate_sha256(expected_sha256)?;

        if !staged_executable.is_file() {
            bail!("staged collector update executable does not exist");
        }

        if sha256_file(staged_executable)? != expected_sha256 {
            bail!("staged collector update SHA-256 changed before updater handoff");
        }

        let current_executable =
            std::env::current_exe().context("failed to locate the running collector executable")?;
        let current_executable = current_executable
            .canonicalize()
            .context("failed to resolve the running collector executable")?;
        let staged_executable = staged_executable
            .canonicalize()
            .context("failed to resolve the staged collector executable")?;
        let health_token = Uuid::now_v7();
        let backup_executable = backup_path(&current_executable, health_token)?;
        let helper = helper_path()?;
        let health_file = health_file_path()?;
        let parent_pid = std::process::id();

        fs::create_dir_all(update_directory()?)
            .context("failed to create collector update directory")?;
        fs::copy(&current_executable, &helper)
            .context("failed to create collector updater helper executable")?;
        sync_file(&helper)?;
        preserve_executable_permissions(&current_executable, &helper)?;

        let spawn_result = Command::new(&helper)
            .arg(APPLY_UPDATE_FLAG)
            .arg(CURRENT_EXECUTABLE_ARGUMENT)
            .arg(&current_executable)
            .arg(STAGED_EXECUTABLE_ARGUMENT)
            .arg(&staged_executable)
            .arg(BACKUP_EXECUTABLE_ARGUMENT)
            .arg(&backup_executable)
            .arg(HEALTH_FILE_ARGUMENT)
            .arg(&health_file)
            .arg(HEALTH_TOKEN_ARGUMENT)
            .arg(health_token.to_string())
            .arg(EXPECTED_SHA256_ARGUMENT)
            .arg(expected_sha256)
            .arg(PARENT_PID_ARGUMENT)
            .arg(parent_pid.to_string())
            .spawn();

        match spawn_result {
            Ok(_) => Ok(()),
            Err(error) => {
                remove_if_exists(&helper)?;
                Err(error).context("failed to launch collector updater helper")
            }
        }
    }
}

pub fn acknowledge_startup(health_file: &Path, health_token: Uuid) -> Result<()> {
    require_safe_health_path(health_file)?;

    let temporary = health_file.with_extension("ack.part");

    remove_if_exists(&temporary)?;
    remove_if_exists(health_file)?;

    let mut file = File::create(&temporary).context("failed to create collector health marker")?;

    file.write_all(health_token.to_string().as_bytes())
        .context("failed to write collector health marker")?;
    file.sync_all()
        .context("failed to flush collector health marker")?;
    drop(file);

    fs::rename(&temporary, health_file).context("failed to finalize collector health marker")
}

pub async fn cleanup_helper_when_possible(helper: PathBuf) {
    if require_safe_helper_path(&helper).is_err() {
        return;
    }

    for _ in 0..HELPER_CLEANUP_ATTEMPTS {
        match tokio::fs::remove_file(&helper).await {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => tokio::time::sleep(HELPER_CLEANUP_INTERVAL).await,
        }
    }
}

fn restart_current(executable: &Path, helper: &Path) -> Result<()> {
    launch_collector(executable, None, helper)
        .map(|_| ())
        .context("failed to restart previous collector executable")
}

fn launch_collector(
    executable: &Path,
    health: Option<(&Path, Uuid)>,
    helper: &Path,
) -> Result<Child> {
    let mut command = Command::new(executable);

    if let Some((health_file, health_token)) = health {
        command
            .arg(HEALTH_FILE_ARGUMENT)
            .arg(health_file)
            .arg(HEALTH_TOKEN_ARGUMENT)
            .arg(health_token.to_string());
    }

    command.arg(CLEANUP_HELPER_ARGUMENT).arg(helper);

    command
        .spawn()
        .with_context(|| format!("failed to launch {}", executable.display()))
}

fn wait_for_parent_exit(parent_pid: u32, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let pid = Pid::from_u32(parent_pid);
    let mut system = System::new();

    while Instant::now() < deadline {
        system.refresh_processes(ProcessesToUpdate::All, true);

        if system.process(pid).is_none() {
            return Ok(());
        }

        thread::sleep(RETRY_INTERVAL);
    }

    bail!("timed out waiting for previous collector process to exit")
}

fn wait_for_health(
    child: &mut Child,
    health_file: &Path,
    expected_token: Uuid,
    timeout: Duration,
) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    let expected_token = expected_token.to_string();

    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(false);
        }

        match fs::read_to_string(health_file) {
            Ok(value) if value.trim() == expected_token => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("failed to read collector health marker"),
        }

        thread::sleep(RETRY_INTERVAL);
    }

    Ok(false)
}

fn move_current_to_backup(current: &Path, backup: &Path, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut last_error = None;

    while Instant::now() < deadline {
        match fs::rename(current, backup) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(error)
                    .context("running collector executable disappeared during update");
            }
            Err(error) => {
                last_error = Some(error);
                thread::sleep(RETRY_INTERVAL);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| std::io::Error::other("collector replacement timed out")))
        .context("timed out waiting to replace the running collector executable")
}

fn restore_backup(current: &Path, backup: &Path) -> Result<()> {
    remove_if_exists(current)?;

    fs::rename(backup, current).context("failed to restore previous collector executable")
}

fn terminate_child(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_none() {
        child
            .kill()
            .context("failed to terminate unhealthy updated collector")?;
    }

    child
        .wait()
        .context("failed to wait for unhealthy updated collector")?;

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

fn set_once<T>(target: &mut Option<T>, value: T, argument_name: &str) -> Result<()> {
    if target.is_some() {
        bail!("{argument_name} may only be specified once");
    }

    *target = Some(value);

    Ok(())
}

fn require_argument<T>(value: Option<T>, argument_name: &str) -> Result<T> {
    value.with_context(|| format!("{argument_name} is required for collector updater mode"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_launch_is_not_interpreted_as_updater_mode() {
        let command = ApplyUpdateCommand::parse(["--activation-token".to_owned()]).unwrap();

        assert!(command.is_none());
    }

    #[test]
    fn updater_mode_requires_all_arguments() {
        let result = ApplyUpdateCommand::parse([APPLY_UPDATE_FLAG.to_owned()]);

        assert!(result.is_err());
    }
}
