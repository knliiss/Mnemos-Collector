use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use directories::ProjectDirs;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const CURRENT_EXECUTABLE_ARGUMENT: &str = "--current-executable";
const STAGED_EXECUTABLE_ARGUMENT: &str = "--staged-executable";
const BACKUP_EXECUTABLE_ARGUMENT: &str = "--backup-executable";
const EXPECTED_SHA256_ARGUMENT: &str = "--expected-sha256";
const PARENT_PID_ARGUMENT: &str = "--parent-pid";
const PARENT_START_TIME_ARGUMENT: &str = "--parent-start-time";
const HEALTH_FILE_ARGUMENT: &str = "--health-file";
const HEALTH_TOKEN_ARGUMENT: &str = "--health-token";
const CLEANUP_RETRY_ATTEMPTS: usize = 40;
const CLEANUP_RETRY_DELAY: Duration = Duration::from_millis(50);

#[test]
fn unhealthy_update_restores_previous_executable() {
    let update_id = Uuid::now_v7();
    let health_token = Uuid::now_v7();
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let root = std::env::temp_dir().join(format!("mnemos-collector-rollback-{update_id}"));
    let update_directory = collector_update_directory();

    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&update_directory).unwrap();

    let helper = update_directory.join(format!("mnemos-collector-updater-{update_id}{suffix}"));
    let staged = update_directory.join(format!("rollback-test-staged-{update_id}{suffix}"));
    let health_file = update_directory.join(format!("mnemos-collector-health-{update_id}.ack"));
    let current = root.join(format!("mnemos-collector-stable{suffix}"));
    let backup = root.join(format!(
        "{}.rollback-{health_token}",
        current.file_name().unwrap().to_string_lossy()
    ));

    prepare_stable_executable(&current);
    prepare_unhealthy_executable(&staged);
    copy_executable(Path::new(env!("CARGO_BIN_EXE_mnemos-collector")), &helper);

    let stable_hash = sha256_file(&current);
    let staged_hash = sha256_file(&staged);

    assert_ne!(stable_hash, staged_hash);

    let status = Command::new(&helper)
        .arg("--apply-update")
        .arg(CURRENT_EXECUTABLE_ARGUMENT)
        .arg(&current)
        .arg(STAGED_EXECUTABLE_ARGUMENT)
        .arg(&staged)
        .arg(BACKUP_EXECUTABLE_ARGUMENT)
        .arg(&backup)
        .arg(HEALTH_FILE_ARGUMENT)
        .arg(&health_file)
        .arg(HEALTH_TOKEN_ARGUMENT)
        .arg(health_token.to_string())
        .arg(EXPECTED_SHA256_ARGUMENT)
        .arg(&staged_hash)
        .arg(PARENT_PID_ARGUMENT)
        .arg((u32::MAX - 1).to_string())
        .arg(PARENT_START_TIME_ARGUMENT)
        .arg("1")
        .status()
        .unwrap();

    assert!(!status.success());
    assert!(current.is_file());
    assert_eq!(sha256_file(&current), stable_hash);
    assert_ne!(sha256_file(&current), staged_hash);

    cleanup(&helper);
    cleanup(&staged);
    cleanup(&health_file);
    cleanup(&backup);
    cleanup(&current);
    let _ = fs::remove_dir(&root);
}

fn collector_update_directory() -> PathBuf {
    ProjectDirs::from("rest", "knalis", "Mnemos Collector")
        .unwrap()
        .data_local_dir()
        .join("updates")
}

#[cfg(unix)]
fn prepare_stable_executable(path: &Path) {
    write_script(path, "#!/bin/sh\nexit 0\n");
}

#[cfg(unix)]
fn prepare_unhealthy_executable(path: &Path) {
    write_script(path, "#!/bin/sh\nexit 42\n");
}

#[cfg(unix)]
fn write_script(path: &Path, content: &str) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, content).unwrap();

    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[cfg(windows)]
fn prepare_stable_executable(path: &Path) {
    let source = windows_system_executable("where.exe");
    copy_executable(&source, path);
}

#[cfg(windows)]
fn prepare_unhealthy_executable(path: &Path) {
    let source = windows_system_executable("whoami.exe");
    copy_executable(&source, path);
}

#[cfg(windows)]
fn windows_system_executable(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("WINDIR").unwrap())
        .join("System32")
        .join(name)
}

fn copy_executable(source: &Path, target: &Path) {
    fs::copy(source, target).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(target).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(target, permissions).unwrap();
    }
}

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path).unwrap();
    let digest = Sha256::digest(bytes);

    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn cleanup(path: &Path) {
    for attempt in 0..CLEANUP_RETRY_ATTEMPTS {
        match fs::remove_file(path) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                if attempt + 1 == CLEANUP_RETRY_ATTEMPTS {
                    panic!("failed to clean up {}: {error}", path.display());
                }

                thread::sleep(CLEANUP_RETRY_DELAY);
            }
            Err(error) => panic!("failed to clean up {}: {error}", path.display()),
        }
    }
}
