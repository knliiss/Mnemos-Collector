use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::diagnostics;
use crate::protocol::{CollectorEvent, EventReport};

const SPOOL_FILE_NAME: &str = "pending-reports.json";
const DEFAULT_CAPACITY: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingReport {
    pub message_id: Uuid,
    pub observed_at: DateTime<Utc>,
    pub event: CollectorEvent,
}

impl PendingReport {
    pub fn new(event: CollectorEvent, observed_at: DateTime<Utc>) -> Self {
        Self {
            message_id: Uuid::now_v7(),
            observed_at,
            event,
        }
    }

    pub fn to_event_report(&self) -> EventReport {
        EventReport::with_message_id(self.message_id, self.event.clone(), self.observed_at)
    }
}

#[derive(Debug)]
pub struct ReportSpool {
    path: PathBuf,
    capacity: usize,
    reports: VecDeque<PendingReport>,
}

#[derive(Debug)]
struct LoadedSnapshot {
    reports: VecDeque<PendingReport>,
    recovered: bool,
    invalid_candidates: Vec<PathBuf>,
}

impl ReportSpool {
    pub async fn open_default() -> Result<Self> {
        let project_dirs = ProjectDirs::from("rest", "knalis", "Mnemos Collector")
            .context("operating system does not expose a local data directory")?;
        let path = project_dirs.data_local_dir().join(SPOOL_FILE_NAME);

        Self::open(path, DEFAULT_CAPACITY).await
    }

    pub async fn open(path: impl Into<PathBuf>, capacity: usize) -> Result<Self> {
        if capacity == 0 {
            bail!("report spool capacity must be positive");
        }

        let path = path.into();
        let loaded = load_snapshot(&path).await?;

        if loaded.reports.len() > capacity {
            bail!(
                "report spool contains {} entries but capacity is {}",
                loaded.reports.len(),
                capacity
            );
        }

        if !loaded.invalid_candidates.is_empty() {
            quarantine_corrupted_snapshots(&loaded.invalid_candidates).await?;
        }

        let spool = Self {
            path,
            capacity,
            reports: loaded.reports,
        };

        diagnostics::set_spool_recovered(loaded.recovered);
        spool.publish_state();

        if loaded.recovered {
            diagnostics::warn(
                "spool",
                "Report spool recovered from a backup or corrupted snapshot; unreadable files were quarantined for diagnostics",
            );
            spool.persist().await?;
        }

        Ok(spool)
    }

    pub fn len(&self) -> usize {
        self.reports.len()
    }

    pub fn is_empty(&self) -> bool {
        self.reports.is_empty()
    }

    pub fn front(&self) -> Option<&PendingReport> {
        self.reports.front()
    }

    pub async fn enqueue(&mut self, report: PendingReport) -> Result<()> {
        if self.reports.len() >= self.capacity {
            diagnostics::error(
                "spool",
                format!(
                    "Reliable report spool is full at {}/{}; refusing to drop an unacknowledged event",
                    self.reports.len(),
                    self.capacity
                ),
            );
            bail!("report spool is full; refusing to drop an unacknowledged event");
        }

        self.reports.push_back(report);

        if let Err(error) = self.persist().await {
            self.reports.pop_back();
            self.publish_state();
            return Err(error);
        }

        self.publish_state();

        Ok(())
    }

    pub async fn discard_before(&mut self, cutoff: DateTime<Utc>) -> Result<usize> {
        let previous = self.reports.clone();
        let previous_len = previous.len();

        self.reports.retain(|report| report.observed_at >= cutoff);

        let discarded = previous_len.saturating_sub(self.reports.len());

        if discarded == 0 {
            return Ok(0);
        }

        if let Err(error) = self.persist().await {
            self.reports = previous;
            self.publish_state();
            return Err(error);
        }

        self.publish_state();

        Ok(discarded)
    }

    pub async fn acknowledge(&mut self, message_id: Uuid) -> Result<()> {
        let Some(front) = self.reports.front() else {
            bail!("cannot acknowledge a report from an empty spool");
        };

        if front.message_id != message_id {
            bail!(
                "cannot acknowledge report {message_id}; next pending report is {}",
                front.message_id
            );
        }

        let acknowledged = self
            .reports
            .pop_front()
            .context("pending report disappeared before acknowledgement")?;

        if let Err(error) = self.persist().await {
            self.reports.push_front(acknowledged);
            self.publish_state();
            return Err(error);
        }

        self.publish_state();

        Ok(())
    }

    async fn persist(&self) -> Result<()> {
        let parent = self
            .path
            .parent()
            .context("report spool path has no parent directory")?;
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;

        let encoded = serde_json::to_vec(&self.reports).context("failed to encode report spool")?;
        let temporary_path = temporary_path(&self.path);
        let backup_path = backup_path(&self.path);
        let mut temporary = File::create(&temporary_path)
            .await
            .with_context(|| format!("failed to create {}", temporary_path.display()))?;

        temporary
            .write_all(&encoded)
            .await
            .context("failed to write report spool snapshot")?;
        temporary
            .sync_all()
            .await
            .context("failed to flush report spool snapshot")?;
        drop(temporary);

        if fs::try_exists(&self.path).await? {
            fs::copy(&self.path, &backup_path)
                .await
                .with_context(|| format!("failed to back up {}", self.path.display()))?;

            #[cfg(windows)]
            fs::remove_file(&self.path)
                .await
                .with_context(|| format!("failed to replace {}", self.path.display()))?;
        }

        fs::rename(&temporary_path, &self.path)
            .await
            .with_context(|| format!("failed to install {}", self.path.display()))?;

        Ok(())
    }

    fn publish_state(&self) {
        diagnostics::set_spool_state(
            self.reports.len(),
            self.capacity,
            self.reports.front().map(|report| report.observed_at),
        );
    }
}

async fn load_snapshot(path: &Path) -> Result<LoadedSnapshot> {
    let candidates = [path.to_path_buf(), temporary_path(path), backup_path(path)];
    let mut found_snapshot = false;
    let mut invalid_candidates = Vec::new();

    for (index, candidate) in candidates.into_iter().enumerate() {
        if !fs::try_exists(&candidate).await? {
            continue;
        }

        found_snapshot = true;

        let content = fs::read(&candidate)
            .await
            .with_context(|| format!("failed to read {}", candidate.display()))?;

        match serde_json::from_slice(&content) {
            Ok(reports) => {
                return Ok(LoadedSnapshot {
                    reports,
                    recovered: index != 0 || !invalid_candidates.is_empty(),
                    invalid_candidates,
                });
            }
            Err(_) => invalid_candidates.push(candidate),
        }
    }

    if !found_snapshot {
        return Ok(LoadedSnapshot {
            reports: VecDeque::new(),
            recovered: false,
            invalid_candidates,
        });
    }

    Ok(LoadedSnapshot {
        reports: VecDeque::new(),
        recovered: true,
        invalid_candidates,
    })
}

async fn quarantine_corrupted_snapshots(candidates: &[PathBuf]) -> Result<()> {
    for candidate in candidates {
        if !fs::try_exists(candidate).await? {
            continue;
        }

        let quarantine = corrupted_snapshot_path(candidate);

        fs::rename(candidate, &quarantine).await.with_context(|| {
            format!(
                "failed to quarantine corrupted report spool {} as {}",
                candidate.display(),
                quarantine.display()
            )
        })?;
    }

    Ok(())
}

fn corrupted_snapshot_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("pending-reports.json");
    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ");

    path.with_file_name(format!(
        "{file_name}.corrupt-{timestamp}-{}",
        Uuid::now_v7()
    ))
}

fn temporary_path(path: &Path) -> PathBuf {
    path.with_extension("json.tmp")
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("json.bak")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GlobalEventType;

    fn report() -> PendingReport {
        report_at(Utc::now())
    }

    fn report_at(observed_at: DateTime<Utc>) -> PendingReport {
        PendingReport::new(
            CollectorEvent::Global {
                event_type: GlobalEventType::Moon,
            },
            observed_at,
        )
    }

    #[tokio::test]
    async fn persists_reports_until_the_matching_acknowledgement() {
        let directory = std::env::temp_dir().join(format!("mnemos-spool-{}", Uuid::now_v7()));
        let path = directory.join("pending-reports.json");
        let mut spool = ReportSpool::open(&path, 8).await.unwrap();
        let pending = report();
        let message_id = pending.message_id;

        spool.enqueue(pending).await.unwrap();
        drop(spool);

        let mut reopened = ReportSpool::open(&path, 8).await.unwrap();
        assert_eq!(reopened.len(), 1);
        assert_eq!(reopened.front().unwrap().message_id, message_id);

        reopened.acknowledge(message_id).await.unwrap();
        drop(reopened);

        let empty = ReportSpool::open(&path, 8).await.unwrap();
        assert!(empty.is_empty());

        let _ = fs::remove_dir_all(directory).await;
    }

    #[tokio::test]
    async fn discards_expired_reports_and_persists_the_remaining_queue() {
        let directory = std::env::temp_dir().join(format!("mnemos-spool-{}", Uuid::now_v7()));
        let path = directory.join("pending-reports.json");
        let mut spool = ReportSpool::open(&path, 8).await.unwrap();
        let now = Utc::now();
        let expired = report_at(now - chrono::Duration::hours(2));
        let retained = report_at(now - chrono::Duration::minutes(5));
        let retained_id = retained.message_id;

        spool.enqueue(expired).await.unwrap();
        spool.enqueue(retained).await.unwrap();

        let discarded = spool
            .discard_before(now - chrono::Duration::minutes(30))
            .await
            .unwrap();

        assert_eq!(discarded, 1);
        assert_eq!(spool.len(), 1);
        assert_eq!(spool.front().unwrap().message_id, retained_id);
        drop(spool);

        let reopened = ReportSpool::open(&path, 8).await.unwrap();

        assert_eq!(reopened.len(), 1);
        assert_eq!(reopened.front().unwrap().message_id, retained_id);

        let _ = fs::remove_dir_all(directory).await;
    }

    #[tokio::test]
    async fn refuses_to_silently_drop_reports_when_full() {
        let directory = std::env::temp_dir().join(format!("mnemos-spool-{}", Uuid::now_v7()));
        let path = directory.join("pending-reports.json");
        let mut spool = ReportSpool::open(&path, 1).await.unwrap();

        spool.enqueue(report()).await.unwrap();
        assert!(spool.enqueue(report()).await.is_err());
        assert_eq!(spool.len(), 1);

        let _ = fs::remove_dir_all(directory).await;
    }

    #[tokio::test]
    async fn recovers_from_a_valid_backup_when_primary_is_corrupted() {
        let directory = std::env::temp_dir().join(format!("mnemos-spool-{}", Uuid::now_v7()));
        let path = directory.join("pending-reports.json");
        let backup = backup_path(&path);
        let pending = report();
        let encoded = serde_json::to_vec(&VecDeque::from([pending.clone()])).unwrap();

        fs::create_dir_all(&directory).await.unwrap();
        fs::write(&path, b"not-json").await.unwrap();
        fs::write(&backup, encoded).await.unwrap();

        let spool = ReportSpool::open(&path, 8).await.unwrap();

        assert_eq!(spool.len(), 1);
        assert_eq!(spool.front(), Some(&pending));
        assert!(path.exists());
        assert!(has_quarantined_snapshot(&directory).await);

        let _ = fs::remove_dir_all(directory).await;
    }

    #[tokio::test]
    async fn quarantines_all_corrupted_snapshots_and_starts_fresh() {
        let directory = std::env::temp_dir().join(format!("mnemos-spool-{}", Uuid::now_v7()));
        let path = directory.join("pending-reports.json");

        fs::create_dir_all(&directory).await.unwrap();
        fs::write(&path, b"not-json").await.unwrap();
        fs::write(temporary_path(&path), b"also-not-json")
            .await
            .unwrap();
        fs::write(backup_path(&path), b"still-not-json")
            .await
            .unwrap();

        let spool = ReportSpool::open(&path, 8).await.unwrap();

        assert!(spool.is_empty());
        assert!(path.exists());
        assert!(has_quarantined_snapshot(&directory).await);

        let _ = fs::remove_dir_all(directory).await;
    }

    async fn has_quarantined_snapshot(directory: &Path) -> bool {
        let mut entries = fs::read_dir(directory).await.unwrap();

        while let Some(entry) = entries.next_entry().await.unwrap() {
            if entry.file_name().to_string_lossy().contains(".corrupt-") {
                return true;
            }
        }

        false
    }
}
