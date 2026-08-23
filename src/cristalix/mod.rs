mod log_path;
mod process;
mod tailer;

pub use log_path::{default_latest_log_path, discover_latest_log};
pub use process::{CristalixProcessDetector, CristalixProcessSnapshot};
pub use tailer::{LogTailer, scan_existing_log_lines};
