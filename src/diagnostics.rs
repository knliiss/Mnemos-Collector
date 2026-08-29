use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use chrono::Local;
use directories::ProjectDirs;

const MAX_VISIBLE_LOG_LINES: usize = 600;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    pub cristalix_running: bool,
    pub game_mode: String,
    pub realtime_connected: bool,
    pub observing: bool,
    pub log_path: Option<PathBuf>,
    pub last_error: Option<String>,
    pub available_update_version: Option<String>,
    pub update_installing: bool,
}

struct Diagnostics {
    lines: Mutex<VecDeque<String>>,
    file: Mutex<Option<File>>,
    debug_enabled: AtomicBool,
    update_install_requested: AtomicBool,
    runtime: RwLock<RuntimeSnapshot>,
    log_file_path: Option<PathBuf>,
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
    diagnostics()
        .lines
        .lock()
        .expect("diagnostics log mutex poisoned")
        .iter()
        .cloned()
        .collect::<Vec<_>>()
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

pub fn set_observing(observing: bool) {
    update_runtime(|runtime| runtime.observing = observing);
}

pub fn set_log_path(path: Option<PathBuf>) {
    update_runtime(|runtime| runtime.log_path = path);
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
    set_update_installing(true);
    info("update", "Manual update requested from desktop UI");
}

pub fn take_update_install_request() -> bool {
    diagnostics()
        .update_install_requested
        .swap(false, Ordering::AcqRel)
}

pub fn set_update_installing(installing: bool) {
    update_runtime(|runtime| runtime.update_installing = installing);
}

pub fn clear_last_error() {
    update_runtime(|runtime| runtime.last_error = None);
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
        let file = log_file_path.as_ref().and_then(open_log_file);

        Self {
            lines: Mutex::new(VecDeque::with_capacity(MAX_VISIBLE_LOG_LINES)),
            file: Mutex::new(file),
            debug_enabled: AtomicBool::new(true),
            update_install_requested: AtomicBool::new(false),
            runtime: RwLock::new(RuntimeSnapshot {
                game_mode: "Unknown".to_owned(),
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
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }
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

fn open_log_file(path: &PathBuf) -> Option<File> {
    let parent = path.parent()?;

    std::fs::create_dir_all(parent).ok()?;

    OpenOptions::new().create(true).append(true).open(path).ok()
}