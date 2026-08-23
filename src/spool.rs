use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::protocol::CollectorEvent;

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
}

#[derive(Debug)]
pub struct ReportSpool {
    path: PathBuf,
    capacity: usize,
    reports: VecDeque<PendingReport>,
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
        let reports = load_snapshot(&path).await?;

        if reports.len() > capacity {
            bail!(
                "report spool contains {} entries but capacity is {}",
                reports.len(),
                capacity
            );
        }

        Ok(Self {
            path,
            capacity,
            reports,
        })
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
            bail!("report spool is full; refusing to drop an unacknowledged event");
        }

        self.reports.push_back(report);

        if let Err(error) = self.persist().await {
            self.reports.pop_back();
            return Err(error);
        }

        Ok(())
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
            return Err(error);
        }

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
}

async fn load_snapshot(path: &Path) -> Result<VecDeque<PendingReport>> {
    for candidate in [path.to_path_buf(), temporary_path(path), backup_path(path)] {
        if !fs::try_exists(&candidate).await? {
            continue;
        }

        let content = fs::read(&candidate)
            .await
            .with_context(|| format!("failed to read {}", candidate.display()))?;

        match serde_json::from_slice(&content) {
            Ok(reports) => return Ok(reports),
            Err(error) if candidate != backup_path(path) => {
                let _ = error;
            }
            Err(error) => {
                return Err(error).context("all available report spool snapshots are invalid");
            }
        }
    }

    Ok(VecDeque::new())
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
        PendingReport::new(
            CollectorEvent::Global {
                event_type: GlobalEventType::Moon,
            },
            Utc::now(),
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
    async fn refuses_to_silently_drop_reports_when_full() {
        let directory = std::env::temp_dir().join(format!("mnemos-spool-{}", Uuid::now_v7()));
        let path = directory.join("pending-reports.json");
        let mut spool = ReportSpool::open(&path, 1).await.unwrap();

        spool.enqueue(report()).await.unwrap();
        assert!(spool.enqueue(report()).await.is_err());
        assert_eq!(spool.len(), 1);

        let _ = fs::remove_dir_all(directory).await;
    }
}
