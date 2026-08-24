use std::fs::{File, OpenOptions};

use anyhow::{Context, Result};
use directories::ProjectDirs;

pub struct InstanceGuard {
    _file: File,
}

impl InstanceGuard {
    pub fn acquire() -> Result<Option<Self>> {
        let project_dirs = ProjectDirs::from("rest", "knalis", "Mnemos Collector")
            .context("operating system does not expose a local data directory")?;
        let state_directory = project_dirs.data_local_dir();

        std::fs::create_dir_all(state_directory).with_context(|| {
            format!(
                "failed to create Collector state directory {}",
                state_directory.display()
            )
        })?;

        let lock_path = state_directory.join("collector.instance.lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open {}", lock_path.display()))?;

        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => {
                Err(error).with_context(|| format!("failed to lock {}", lock_path.display()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;

    #[test]
    fn exclusive_file_lock_rejects_a_second_handle() {
        let path = std::env::temp_dir().join(format!(
            "mnemos-collector-instance-test-{}",
            uuid::Uuid::now_v7()
        ));
        let first = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();

        first.try_lock().unwrap();

        assert!(matches!(
            second.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));

        drop(first);

        second.try_lock().unwrap();
        drop(second);
        let _ = std::fs::remove_file(path);
    }
}
