use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::signal;
use tokio::time::{MissedTickBehavior, interval};

use crate::cristalix::{CristalixProcessDetector, LogTailer, discover_latest_log};
use crate::parser::{EventDeduplicator, GameMode, LogParser};
use crate::protocol::CollectorEvent;
use crate::realtime::{RealtimeClient, RealtimeConfig};
use crate::security::credential_id_from_access_key;
use crate::spool::{PendingReport, ReportSpool};
use crate::update::{UpdateCoordinator, UpdateHandoff};

const MAX_REPORTS_PER_TICK: usize = 16;

#[derive(Debug, Clone)]
pub struct CollectorApplicationConfig {
    pub process_check_interval: Duration,
    pub log_poll_interval: Duration,
    pub reconnect_initial_delay: Duration,
    pub reconnect_max_delay: Duration,
}

impl Default for CollectorApplicationConfig {
    fn default() -> Self {
        Self {
            process_check_interval: Duration::from_secs(2),
            log_poll_interval: Duration::from_millis(250),
            reconnect_initial_delay: Duration::from_secs(1),
            reconnect_max_delay: Duration::from_secs(30),
        }
    }
}

pub struct CollectorApplication {
    config: CollectorApplicationConfig,
    access_key: String,
    realtime_config: RealtimeConfig,
    process_detector: CristalixProcessDetector,
    process_running: bool,
    process_log_candidates: Vec<PathBuf>,
    next_process_check: Instant,
    parser: LogParser,
    deduplicator: EventDeduplicator,
    tailer: Option<LogTailer>,
    cached_log_path: Option<PathBuf>,
    spool: ReportSpool,
    realtime: Option<RealtimeClient>,
    reconnect_delay: Duration,
    next_reconnect_at: Instant,
    update_coordinator: Option<UpdateCoordinator>,
}

impl CollectorApplication {
    pub async fn new(access_key: String) -> Result<Self> {
        Self::with_config(access_key, CollectorApplicationConfig::default()).await
    }

    pub async fn with_config(
        access_key: String,
        config: CollectorApplicationConfig,
    ) -> Result<Self> {
        let spool = ReportSpool::open_default()
            .await
            .context("failed to open reliable report spool")?;
        let collector_id = credential_id_from_access_key(&access_key)?;
        let update_coordinator = UpdateCoordinator::from_build(collector_id)
            .context("failed to initialize collector update coordinator")?;
        let now = Instant::now();

        Ok(Self {
            reconnect_delay: config.reconnect_initial_delay,
            config,
            access_key,
            realtime_config: RealtimeConfig::default(),
            process_detector: CristalixProcessDetector::default(),
            process_running: false,
            process_log_candidates: Vec::new(),
            next_process_check: now,
            parser: LogParser::default(),
            deduplicator: EventDeduplicator::new(Duration::from_secs(2)),
            tailer: None,
            cached_log_path: None,
            spool,
            realtime: None,
            next_reconnect_at: now,
            update_coordinator,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        let mut ticker = interval(self.config.log_poll_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if self.tick().await? {
                        return Ok(());
                    }
                }
                result = signal::ctrl_c() => {
                    result.context("failed to listen for shutdown signal")?;
                    self.shutdown().await;
                    return Ok(());
                }
            }
        }
    }

    async fn tick(&mut self) -> Result<bool> {
        self.refresh_process_state_if_due().await?;
        self.ensure_realtime_connection().await;

        if !self.process_running {
            self.pause_connection().await;
            return Ok(self.try_apply_update().await);
        }

        self.ensure_log_tailer().await?;
        self.read_log().await?;

        if self.parser.mode() == GameMode::MasterSword {
            self.observe_connection().await;
            self.deliver_pending_reports().await?;
        } else {
            self.pause_connection().await;
        }

        Ok(self.try_apply_update().await)
    }

    async fn refresh_process_state_if_due(&mut self) -> Result<()> {
        let now = Instant::now();

        if now < self.next_process_check {
            return Ok(());
        }

        self.next_process_check = now + self.config.process_check_interval;

        let snapshot = self.process_detector.inspect();
        let candidates_changed = snapshot.latest_log_candidates != self.process_log_candidates;

        self.process_log_candidates = snapshot.latest_log_candidates;

        let tailer_points_to_running_process = self.tailer.as_ref().is_none_or(|tailer| {
            self.process_log_candidates.is_empty()
                || self
                    .process_log_candidates
                    .iter()
                    .any(|candidate| candidate == tailer.path())
        });

        if candidates_changed && !tailer_points_to_running_process {
            self.parser = LogParser::default();
            self.tailer = None;
            self.cached_log_path = None;
            self.pause_connection().await;
        }

        if snapshot.running == self.process_running {
            return Ok(());
        }

        self.process_running = snapshot.running;

        if snapshot.running {
            self.parser = LogParser::default();
            self.tailer = None;
            return Ok(());
        }

        let pending_events = self.parser.flush();

        self.enqueue_events(pending_events).await?;
        self.parser = LogParser::default();
        self.tailer = None;
        self.pause_connection().await;

        Ok(())
    }

    async fn ensure_log_tailer(&mut self) -> Result<()> {
        if self.tailer.is_some() {
            return Ok(());
        }

        let Some(path) = discover_latest_log(
            self.cached_log_path.as_deref(),
            &self.process_log_candidates,
        ) else {
            return Ok(());
        };

        match LogTailer::open_from_end(&path).await {
            Ok(tailer) => {
                self.cached_log_path = Some(path);
                self.tailer = Some(tailer);
            }
            Err(error) => {
                self.cached_log_path = None;
                return Err(error).context("failed to start latest.log tailing");
            }
        }

        Ok(())
    }

    async fn read_log(&mut self) -> Result<()> {
        let read_result = {
            let Some(tailer) = self.tailer.as_mut() else {
                return Ok(());
            };
            let generation = tailer.generation();

            tailer
                .read_new_lines()
                .await
                .map(|lines| (lines, tailer.generation() != generation))
        };

        let (lines, source_reset) = match read_result {
            Ok(result) => result,
            Err(_) => {
                self.parser = LogParser::default();
                self.tailer = None;
                self.cached_log_path = None;
                self.pause_connection().await;
                return Ok(());
            }
        };

        if source_reset {
            self.parser = LogParser::default();
            self.pause_connection().await;
            return Ok(());
        }

        for line in lines {
            let previous_mode = self.parser.mode();
            let events = self.parser.consume_line(&line);
            let current_mode = self.parser.mode();

            self.enqueue_events(events).await?;

            if previous_mode == GameMode::MasterSword && current_mode != GameMode::MasterSword {
                self.pause_connection().await;
            }
        }

        Ok(())
    }

    async fn enqueue_events(&mut self, events: Vec<CollectorEvent>) -> Result<()> {
        for event in events {
            if !self.deduplicator.accept(&event, Instant::now()) {
                continue;
            }

            self.spool
                .enqueue(PendingReport::new(event, Utc::now()))
                .await
                .context("failed to persist observed event before delivery")?;
        }

        Ok(())
    }

    async fn ensure_realtime_connection(&mut self) {
        if self
            .realtime
            .as_ref()
            .is_some_and(RealtimeClient::is_connected)
        {
            return;
        }

        if self.realtime.is_some() {
            self.realtime = None;
            self.schedule_reconnect();
        }

        if Instant::now() < self.next_reconnect_at {
            return;
        }

        match RealtimeClient::connect(&self.realtime_config, &self.access_key).await {
            Ok(client) => {
                self.realtime = Some(client);
                self.reconnect_delay = self.config.reconnect_initial_delay;
                self.next_reconnect_at = Instant::now();
            }
            Err(_) => self.schedule_reconnect(),
        }
    }

    async fn observe_connection(&mut self) {
        let Some(client) = self.realtime.as_mut() else {
            return;
        };

        if client.observe().await.is_err() {
            self.realtime = None;
            self.schedule_reconnect();
        }
    }

    async fn deliver_pending_reports(&mut self) -> Result<()> {
        for _ in 0..MAX_REPORTS_PER_TICK {
            let Some(pending) = self.spool.front().cloned() else {
                return Ok(());
            };
            let Some(client) = self.realtime.as_mut() else {
                return Ok(());
            };
            let report = pending.to_event_report();

            if client.report(&report).await.is_err() {
                self.realtime = None;
                self.schedule_reconnect();
                return Ok(());
            }

            self.spool
                .acknowledge(pending.message_id)
                .await
                .context("failed to persist report acknowledgement")?;
        }

        Ok(())
    }

    async fn pause_connection(&mut self) {
        let Some(client) = self.realtime.as_mut() else {
            return;
        };

        if client.pause().await.is_err() {
            self.realtime = None;
            self.schedule_reconnect();
        }
    }

    async fn try_apply_update(&mut self) -> bool {
        let Some(mut coordinator) = self.update_coordinator.take() else {
            return false;
        };
        let had_pending_update = coordinator.has_pending_update();
        let poll_result = coordinator
            .poll(self.realtime.as_mut(), self.spool.is_empty())
            .await;

        let request = match poll_result {
            Ok(Some(request)) => request,
            Ok(None) => {
                self.update_coordinator = Some(coordinator);
                return false;
            }
            Err(_) => {
                coordinator.defer_after_error();
                self.update_coordinator = Some(coordinator);

                if had_pending_update {
                    self.realtime = None;
                    self.schedule_reconnect();
                }

                return false;
            }
        };

        let handoff_result =
            UpdateHandoff::start(request.staged_executable(), request.expected_sha256());

        if handoff_result.is_err() {
            coordinator.restore_handoff(request);
            self.update_coordinator = Some(coordinator);
            return false;
        }

        self.realtime = None;

        true
    }

    fn schedule_reconnect(&mut self) {
        let now = Instant::now();

        self.next_reconnect_at = now + self.reconnect_delay;
        self.reconnect_delay = self
            .reconnect_delay
            .saturating_mul(2)
            .min(self.config.reconnect_max_delay);
    }

    async fn shutdown(&mut self) {
        let pending_events = self.parser.flush();
        let _ = self.enqueue_events(pending_events).await;

        let Some(mut client) = self.realtime.take() else {
            return;
        };

        let _ = client.pause().await;
        let _ = client.close().await;
    }
}
