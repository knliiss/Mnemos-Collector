mod coordinator;
pub(crate) mod integrity;
pub(crate) mod paths;
pub(crate) mod process;
mod release;
mod version;

pub use coordinator::{UpdateCoordinator, UpdateHandoffRequest};
pub use process::{
    ApplyUpdateCommand, UpdateHandoff, acknowledge_startup, cleanup_helper_when_possible,
};
pub use release::{ReleaseClient, UpdateCandidate, UpdateConfig, deterministic_rollout_delay};
pub use version::CollectorVersion;
