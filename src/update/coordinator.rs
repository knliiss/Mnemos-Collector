use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::diagnostics;
use crate::realtime::RealtimeClient;
use crate::update::release::{ReleaseClient, UpdateCandidate, UpdateConfig};
use crate::update::version::CollectorVersion;

const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(15 * 60);
const UPDATE_ERROR_RETRY_DELAY: Duration = Duration::from_secs(5 * 60);
const MINIMUM_SLOT_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug)]
struct PendingUpdate {
    candidate: UpdateCandidate,
    staged_executable: std::path::PathBuf,
    ready_at: Instant,
}

#[derive(Debug)]
pub struct UpdateHandoffRequest {
    pending: PendingUpdate,
}

impl UpdateHandoffRequest {
    pub fn version(&self) -> CollectorVersion {
        self.pending.candidate.version
    }

    pub fn staged_executable(&self) -> &Path {
        &self.pending.staged_executable
    }

    pub fn expected_sha256(&self) -> &str {
        &self.pending.candidate.artifact.sha256
    }
}

pub struct UpdateCoordinator {
    release_client: ReleaseClient,
    collector_id: Uuid,
    current_version: CollectorVersion,
    next_check_at: Instant,
    pending: Option<PendingUpdate>,
    manual_install_requested: bool,
}

impl UpdateCoordinator {
    pub fn from_build(collector_id: Uuid) -> Result<Option<Self>> {
        let Some(config) = UpdateConfig::from_build()? else {
            return Ok(None);
        };
        let release_client = ReleaseClient::new(config)?;
        let current_version = CollectorVersion::from_str(env!("CARGO_PKG_VERSION"))
            .context("running collector version is invalid")?;

        Ok(Some(Self {
            release_client,
            collector_id,
            current_version,
            next_check_at: Instant::now(),
            pending: None,
            manual_install_requested: false,
        }))
    }

    pub fn has_pending_update(&self) -> bool {
        self.pending.is_some()
    }

    pub fn request_immediate_install(&mut self) {
        self.manual_install_requested = true;
        self.next_check_at = Instant::now();

        if let Some(pending) = self.pending.as_mut() {
            pending.ready_at = Instant::now();
        }
    }

    pub async fn poll(
        &mut self,
        realtime: Option<&mut RealtimeClient>,
        delivery_idle: bool,
    ) -> Result<Option<UpdateHandoffRequest>> {
        if diagnostics::take_update_install_request() {
            self.request_immediate_install();
        }

        self.refresh_candidate_if_due().await?;

        let manual_install = self.manual_install_requested;

        if !delivery_idle && !manual_install {
            return Ok(None);
        }

        let Some(pending) = self.pending.as_ref() else {
            if manual_install {
                self.manual_install_requested = false;
                diagnostics::set_update_installing(false);
            }

            return Ok(None);
        };

        if !manual_install && Instant::now() < pending.ready_at {
            return Ok(None);
        }

        if let Some(realtime) = realtime {
            let version = pending.candidate.version;
            let decision = realtime.request_update_slot(&version.to_string()).await?;

            if !decision.granted {
                let retry_delay = decision
                    .retry_after
                    .unwrap_or(UPDATE_ERROR_RETRY_DELAY)
                    .max(MINIMUM_SLOT_RETRY_DELAY);

                self.pending
                    .as_mut()
                    .expect("pending collector update disappeared")
                    .ready_at = Instant::now() + retry_delay;

                return Ok(None);
            }

            realtime.pause().await?;
        } else if !manual_install {
            return Ok(None);
        }

        let pending = self
            .pending
            .take()
            .expect("granted collector update disappeared");

        self.manual_install_requested = false;
        diagnostics::set_available_update_version(None);
        diagnostics::set_update_installing(true);

        Ok(Some(UpdateHandoffRequest { pending }))
    }

    pub fn restore_handoff(&mut self, mut request: UpdateHandoffRequest) {
        request.pending.ready_at = Instant::now() + UPDATE_ERROR_RETRY_DELAY;
        diagnostics::set_available_update_version(Some(request.version().to_string()));
        diagnostics::set_update_installing(false);
        self.pending = Some(request.pending);
    }

    pub fn defer_after_error(&mut self) {
        let retry_at = Instant::now() + UPDATE_ERROR_RETRY_DELAY;

        diagnostics::set_update_installing(false);
        self.manual_install_requested = false;

        if let Some(pending) = self.pending.as_mut() {
            pending.ready_at = retry_at;
        } else {
            self.next_check_at = retry_at;
        }
    }

    async fn refresh_candidate_if_due(&mut self) -> Result<()> {
        if self.pending.is_some() || Instant::now() < self.next_check_at {
            return Ok(());
        }

        self.next_check_at = Instant::now() + UPDATE_CHECK_INTERVAL;

        let Some(candidate) = self.release_client.check(self.current_version).await? else {
            diagnostics::set_available_update_version(None);
            diagnostics::set_update_installing(false);
            self.manual_install_requested = false;
            return Ok(());
        };
        let staged_executable = self.release_client.stage(&candidate).await?;
        let rollout_delay = if self.manual_install_requested {
            Duration::ZERO
        } else {
            self.release_client
                .rollout_delay(self.collector_id, candidate.version)
        };

        diagnostics::set_available_update_version(Some(candidate.version.to_string()));

        self.pending = Some(PendingUpdate {
            candidate,
            staged_executable,
            ready_at: Instant::now() + rollout_delay,
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_check_interval_is_shorter_than_rollout_window() {
        assert!(UPDATE_CHECK_INTERVAL <= Duration::from_secs(30 * 60));
    }
}
