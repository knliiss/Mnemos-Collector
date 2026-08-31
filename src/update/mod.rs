mod coordinator;
pub(crate) mod integrity;
pub(crate) mod paths;
pub(crate) mod process;
mod release;
mod version;

use std::path::Path;

use anyhow::Result;

pub use coordinator::{UpdateCoordinator, UpdateHandoffRequest};
pub use process::{ApplyUpdateCommand, acknowledge_startup, cleanup_helper_when_possible};
pub use release::{ReleaseClient, UpdateCandidate, UpdateConfig, deterministic_rollout_delay};
pub use version::CollectorVersion;

pub struct UpdateHandoff;

impl UpdateHandoff {
    pub fn start(staged_executable: &Path, expected_sha256: &str) -> Result<()> {
        process::UpdateHandoff::start(staged_executable, expected_sha256)?;
        schedule_portable_parent_exit();

        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
fn schedule_portable_parent_exit() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(250));
        std::process::exit(0);
    });
}

#[cfg(target_os = "windows")]
fn schedule_portable_parent_exit() {}
