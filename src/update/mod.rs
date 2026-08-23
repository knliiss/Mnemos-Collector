pub mod release;
pub mod version;

pub use release::{ReleaseClient, UpdateCandidate, UpdateConfig, deterministic_rollout_delay};
pub use version::CollectorVersion;
