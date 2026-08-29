use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use chrono::{DateTime, Local, Utc};
use directories::ProjectDirs;

const MAX_VISIBLE_LOG_LINES: usize = 600;
const MAX_LOG_FILE_BYTES: u64 = 5 * 1024 * 1024;
const RETAINED_LOG_FILES: usize = 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InstallationMode {
    Stable,
    External,
    #[default]
    Unknown,
}

impl InstallationMode {
    fn label(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::External => "external",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    pub cristalix_running: bool,
    pub game_mode: String,
    pub realtime_connected: bool,
    pub observing: bool,
    pub log_path: Option<PathBuf>,
    pub last_error: Option<String>,
    pub available_update_version: Option<String>,
    pub required_update_version: Option<String>,
    pub update_installing: bool,
    pub update_waiting_for_slot: bool,
    pub update_retry_after_seconds: Option<u64>,
    pub installation_mode: InstallationMode,
    pub spool_pending: usize,
    pub spool_capacity: usize,
    pub oldest_pending_at: Option<DateTime<Utc>>,
    pub last_log_activity_at: Option<DateTime<Utc>>,
    pub last_realtime_message_at: Option<DateTime<Utc>>,
    pub collector_protocol_version: Option<u16>,
    pub server_protocol_version: Option<u16>,
    pub spool_recovered: bool,
}

struct Diagnostics {
    lines: Mutex<VecDeque<String>>,
    file: Mutex<Option<RotatingLogWriter>>,
    debug_enabled: AtomicBool,
    update_install_requested: AtomicBool,
    runtime: RwLock<RuntimeSnapshot>,
    log_file_path: Option<PathBuf>,
}

struct RotatingLogWriter {
    path: PathBuf,
    file: Option<File>,
    bytes_written: u64,
    max_bytes: u64,
    retained_files: usize,
}

static DIAGNOSTICS: OnceLock<Diagnostics> = OnceLock::new();

pub fn initialize() {
    let diagnostics = DIAGNOSTICS.get_or_init(Diagnostics::new);

    diagnostics.write(
        "INFO",
        "startup",
        format!("Mnemos Collector {} started", env!("CARGO_PKG_VERSION")),
    );
}

pub fn install_panic_hook() {
    let previous = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |panic_info| {
        error("panic", format!("Unhandled panic: {panic_info}"));
        previous(panic_info);
    }));
}

pub fn debug_enabled() -> bool {
    diagnostics().debug_enabled.load(Ordering::Relaxed)
}

pub fn set_debug_enabled(enabled: bool) {
    diagnostics()
        .debug_enabled
        .store(enabled, Ordering::Relaxed);

    info(
        "diagnostics",
        if enabled {
            "Detailed diagnostic hints enabled"
        } else {
            "Detailed diagnostic hints disabled"
        },
    );
}

pub fn debug(category: &str, message: impl Into<String>) {
    let diagnostics = diagnostics();

    if diagnostics.debug_enabled.load(Ordering::Relaxed) {
        diagnostics.write("DEBUG", category, message.into());
    }
}

pub fn info(category: &str, message: impl Into<String>) {
    diagnostics().write("INFO", category, message.into());
}

pub fn warn(category: &str, message: impl Into<String>) {
    diagnostics().write("WARN", category, message.into());
}

pub fn error(category: &str, message: impl Into<String>) {
    let message = message.into();

    diagnostics().write("ERROR", category, message.clone());
    update_runtime(|runtime| runtime.last_error = Some(message));
}

pub fn recent_text() -> String {
    let journal = diagnostics()
        .lines
        .lock()
        .expect("diagnostics log mutex poisoned")
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join("\r\n");
    let report = diagnostic_report();

    if journal.is_empty() {
        report
    } else {
        format!("{report}\r\n\r\n=== JOURNAL ===\r\n{journal}")
    }
}

pub fn diagnostic_report() -> String {
    let runtime = runtime_snapshot();
    let game_mode = if runtime.game_mode.trim().is_empty() {
        "Unknown"
    } else {
        runtime.game_mode.as_str()
    };
    let log_source = runtime
        .log_path
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "not found".to_owned());
    let update_state = if runtime.update_installing {
        "installing".to_owned()
    } else if runtime.update_waiting_for_slot {
        match runtime.update_retry_after_seconds {
            Some(seconds) => format!("waiting for update slot; retry in {seconds}s"),
            None => "waiting for update slot".to_owned(),
        }
    } else if let Some(required) = runtime.required_update_version.as_deref() {
        format!("required >= {required}")
    } else if let Some(available) = runtime.available_update_version.as_deref() {
        format!("available {available}")
    } else {
        "current".to_owned()
    };
    let protocol = match (
        runtime.collector_protocol_version,
        runtime.server_protocol_version,
    ) {
        (Some(collector), Some(server)) => format!("collector={collector}, server={server}"),
        (Some(collector), None) => format!("collector={collector}, server=unknown"),
        _ => "unknown".to_owned(),
    };
    let oldest_pending = age_label(runtime.oldest_pending_at);
    let last_log_activity = age_label(runtime.last_log_activity_at);
    let last_realtime_message = age_label(runtime.last_realtime_message_at);
    let last_error = runtime.last_error.as_deref().unwrap_or("none");

    [
        "=== MNEMOS COLLECTOR DIAGNOSTICS ===".to_owned(),
        format!("Version: {}", env!("CARGO_PKG_VERSION")),
        format!(
            "Platform: {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
        format!("Installation: {}", runtime.installation_mode.label()),
        format!(
            "Cristalix: {}",
            if runtime.cristalix_running {
                "running"
            } else {
                "waiting"
            }
        ),
        format!("Game mode: {game_mode}"),
        format!("Log source: {log_source}"),
        format!("Last log activity: {last_log_activity}"),
        format!(
            "Realtime: {}",
            if runtime.realtime_connected {
                "connected"
            } else {
                "disconnected"
            }
        ),
        format!("Last realtime message: {last_realtime_message}"),
        format!("Protocol: {protocol}"),
        format!(
            "Observing: {}",
            if runtime.observing { "yes" } else { "no" }
        ),
        format!(
            "Pending reports: {}/{}",
            runtime.spool_pending, runtime.spool_capacity
        ),
        format!("Oldest pending: {oldest_pending}"),
        format!(
            "Spool recovery: {}",
            if runtime.spool_recovered {
                "recovered after corruption"
            } else {
                "clean"
            }
        ),
        format!("Update: {update_state}"),
        format!("Last error: {last_error}"),
    ]
    .join("\r\n")
}

pub fn runtime_snapshot() -> RuntimeSnapshot {
    diagnostics()
        .runtime
        .read()
        .expect("diagnostics runtime lock poisoned")
        .clone()
}

pub fn log_file_path() -> Option<PathBuf> {
    diagnostics().log_file_path.clone()
}

pub fn set_installation_mode(mode: InstallationMode) {
    update_runtime(|runtime| runtime.installation_mode = mode);
}

pub fn set_cristalix_running(running: bool) {
    update_runtime(|runtime| runtime.cristalix_running = running);
}

pub fn set_game_mode(mode: impl Into<String>) {
    let mode = mode.into();

    update_runtime(|runtime| runtime.game_mode = mode);
}

pub fn set_realtime_connected(connected: bool) {
    update_runtime(|runtime| {
        runtime.realtime_connected = connected;

        if !connected {
            runtime.observing = false;
        }
    });
}

pub fn mark_realtime_activity() {
    update_runtime(|runtime| runtime.last_realtime_message_at = Some(Utc::now()));
}

pub fn set_protocol_versions(collector: u16, server: Option<u16>) {
    update_runtime(|runtime| {
        runtime.collector_protocol_version = Some(collector);
        runtime.server_protocol_version = server;
    });
}

pub fn set_required_update_version(version: Option<String>) {
    update_runtime(|runtime| {
        runtime.required_update_version = version.clone();

        if let Some(version) = version {
            runtime.last_error = Some(format!(
                "Требуется обновление Collector до версии {version} или новее."
            ));
            runtime.observing = false;
        } else if runtime
            .last_error
            .as_deref()
            .is_some_and(|message| message.starts_with("Требуется обновление Collector"))
        {
            runtime.last_error = None;
        }
    });
}

pub fn set_observing(observing: bool) {
    update_runtime(|runtime| runtime.observing = observing);
}

pub fn set_log_path(path: Option<PathBuf>) {
    update_runtime(|runtime| runtime.log_path = path);
}

pub fn mark_log_activity() {
    update_runtime(|runtime| runtime.last_log_activity_at = Some(Utc::now()));
}

pub fn set_spool_state(pending: usize, capacity: usize, oldest_pending_at: Option<DateTime<Utc>>) {
    update_runtime(|runtime| {
        runtime.spool_pending = pending;
        runtime.spool_capacity = capacity;
        runtime.oldest_pending_at = oldest_pending_at;
    });
}

pub fn set_spool_recovered(recovered: bool) {
    update_runtime(|runtime| runtime.spool_recovered = recovered);
}

pub fn set_available_update_version(version: Option<String>) {
    update_runtime(|runtime| runtime.available_update_version = version);
}

pub fn request_update_install() {
    let diagnostics = diagnostics();
    let update_available = diagnostics
        .runtime
        .read()
        .expect("diagnostics runtime lock poisoned")
        .available_update_version
        .is_some();

    if !update_available {
        return;
    }

    diagnostics
        .update_install_requested
        .store(true, Ordering::Release);
    set_update_waiting_for_slot(true, None);
    info("update", "Manual update requested from desktop UI");
}

pub fn take_update_install_request() -> bool {
    diagnostics()
        .update_install_requested
        .swap(false, Ordering::AcqRel)
}

pub fn set_update_installing(installing: bool) {
    update_runtime(|runtime| {
        runtime.update_installing = installing;

        if installing {
            runtime.update_waiting_for_slot = false;
            runtime.update_retry_after_seconds = None;
        }
    });
}

pub fn set_update_waiting_for_slot(waiting: bool, retry_after_seconds: Option<u64>) {
    update_runtime(|runtime| {
        runtime.update_waiting_for_slot = waiting;
        runtime.update_retry_after_seconds = waiting.then_some(retry_after_seconds).flatten();

        if waiting {
            runtime.update_installing = false;
        }
    });
}

pub fn clear_last_error() {
    update_runtime(|runtime| runtime.last_error = None);
}

fn age_label(timestamp: Option<DateTime<Utc>>) -> String {
    let Some(timestamp) = timestamp else {
        return "never".to_owned();
    };
    let seconds = Utc::now()
        .signed_duration_since(timestamp)
        .num_seconds()
        .max(0);

    match seconds {
        0..=4 => "now".to_owned(),
        5..=59 => format!("{seconds}s ago"),
        60..=3_599 => format!("{}m ago", seconds / 60),
        _ => format!("{}h ago", seconds / 3_600),
    }
}

fn update_runtime(update: impl FnOnce(&mut RuntimeSnapshot)) {
    let mut runtime = diagnostics()
        .runtime
        .write()
        .expect("diagnostics runtime lock poisoned");

    update(&mut runtime);
}

fn diagnostics() -> &'static Diagnostics {
    DIAGNOSTICS.get_or_init(Diagnostics::new)
}

impl Diagnostics {
    fn new() -> Self {
        let log_file_path = default_log_file_path();
        let file = log_file_path.as_ref().and_then(|path| {
            RotatingLogWriter::open(path, MAX_LOG_FILE_BYTES, RETAINED_LOG_FILES).ok()
        });

        Self {
            lines: Mutex::new(VecDeque::with_capacity(MAX_VISIBLE_LOG_LINES)),
            file: Mutex::new(file),
            debug_enabled: AtomicBool::new(true),
            update_install_requested: AtomicBool::new(false),
            runtime: RwLock::new(RuntimeSnapshot {
                game_mode: "Unknown".to_owned(),
                spool_capacity: 0,
                ..RuntimeSnapshot::default()
            }),
            log_file_path,
        }
    }

    fn write(&self, level: &str, category: &str, message: String) {
        let timestamp = Local::now().format("%H:%M:%S%.3f");
        let line = format!("[{timestamp}] [{level:<5}] [{category}] {message}");

        {
            let mut lines = self.lines.lock().expect("diagnostics log mutex poisoned");

            if lines.len() == MAX_VISIBLE_LOG_LINES {
                lines.pop_front();
            }

            lines.push_back(line.clone());
        }

        if let Some(file) = self
            .file
            .lock()
            .expect("diagnostics file mutex poisoned")
            .as_mut()
        {
            let _ = file.write_line(&line);
        }
    }
}

impl RotatingLogWriter {
    fn open(path: &Path, max_bytes: u64, retained_files: usize) -> std::io::Result<Self> {
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "log path has no parent")
        })?;

        fs::create_dir_all(parent)?;

        if path
            .metadata()
            .is_ok_and(|metadata| metadata.len() >= max_bytes)
        {
            rotate_log_files(path, retained_files)?;
        }

        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let bytes_written = file.metadata()?.len();

        Ok(Self {
            path: path.to_path_buf(),
            file: Some(file),
            bytes_written,
            max_bytes,
            retained_files,
        })
    }

    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let line_bytes = line.len() as u64 + 1;

        if self.bytes_written > 0 && self.bytes_written.saturating_add(line_bytes) > self.max_bytes
        {
            self.rotate()?;
        }

        let file = self
            .file
            .as_mut()
            .expect("rotating log writer must keep an open file");

        writeln!(file, "{line}")?;
        file.flush()?;
        self.bytes_written = self.bytes_written.saturating_add(line_bytes);

        Ok(())
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        self.file.take();
        rotate_log_files(&self.path, self.retained_files)?;

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)?;

        self.file = Some(file);
        self.bytes_written = 0;

        Ok(())
    }
}

fn rotate_log_files(path: &Path, retained_files: usize) -> std::io::Result<()> {
    if retained_files == 0 {
        if path.exists() {
            fs::remove_file(path)?;
        }
        return Ok(());
    }

    let oldest = rotated_log_path(path, retained_files);

    if oldest.exists() {
        fs::remove_file(&oldest)?;
    }

    for index in (1..retained_files).rev() {
        let source = rotated_log_path(path, index);

        if !source.exists() {
            continue;
        }

        let destination = rotated_log_path(path, index + 1);

        if destination.exists() {
            fs::remove_file(&destination)?;
        }

        fs::rename(source, destination)?;
    }

    if path.exists() {
        let first = rotated_log_path(path, 1);

        if first.exists() {
            fs::remove_file(&first)?;
        }

        fs::rename(path, first)?;
    }

    Ok(())
}

fn rotated_log_path(path: &Path, index: usize) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("collector");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("log");

    path.with_file_name(format!("{stem}.{index}.{extension}"))
}

fn default_log_file_path() -> Option<PathBuf> {
    let project_dirs = ProjectDirs::from("rest", "knalis", "Mnemos Collector")?;

    Some(
        project_dirs
            .data_local_dir()
            .join("logs")
            .join("collector.log"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn diagnostic_report_never_contains_credentials() {
        let report = diagnostic_report();

        assert!(report.contains("MNEMOS COLLECTOR DIAGNOSTICS"));
        assert!(!report.to_ascii_lowercase().contains("access key"));
        assert!(!report.to_ascii_lowercase().contains("authorization"));
    }

    #[test]
    fn rotating_writer_keeps_bounded_history() {
        let directory = std::env::temp_dir().join(format!("mnemos-log-{}", Uuid::now_v7()));
        let path = directory.join("collector.log");

        fs::create_dir_all(&directory).unwrap();

        let mut writer = RotatingLogWriter::open(&path, 32, 2).unwrap();

        for index in 0..12 {
            writer
                .write_line(&format!("line-{index:02}-xxxxxxxx"))
                .unwrap();
        }

        assert!(path.exists());
        assert!(rotated_log_path(&path, 1).exists());
        assert!(rotated_log_path(&path, 2).exists());
        assert!(!rotated_log_path(&path, 3).exists());

        drop(writer);
        let _ = fs::remove_dir_all(directory);
    }
}
