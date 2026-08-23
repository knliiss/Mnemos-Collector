use std::time::{Duration, Instant};

use anyhow::Result;
use mnemos_collector::cristalix::{CristalixProcessDetector, LogTailer, discover_latest_log};
use mnemos_collector::parser::{EventDeduplicator, LogParser};
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<()> {
    let mut process_detector = CristalixProcessDetector::default();
    let mut parser = LogParser::default();
    let mut deduplicator = EventDeduplicator::new(Duration::from_secs(2));
    let mut tailer: Option<LogTailer> = None;

    loop {
        if !process_detector.is_running() {
            tailer = None;
            sleep(Duration::from_secs(2)).await;
            continue;
        }

        if tailer.is_none()
            && let Some(path) = discover_latest_log(None)
        {
            tailer = Some(LogTailer::open_from_end(path).await?);
        }

        let Some(active_tailer) = tailer.as_mut() else {
            sleep(Duration::from_secs(1)).await;
            continue;
        };

        for line in active_tailer.read_new_lines().await? {
            for event in parser.consume_line(&line) {
                if deduplicator.accept(&event, Instant::now()) {
                    println!("{}", serde_json::to_string(&event)?);
                }
            }
        }

        for event in parser.flush() {
            if deduplicator.accept(&event, Instant::now()) {
                println!("{}", serde_json::to_string(&event)?);
            }
        }

        sleep(Duration::from_millis(250)).await;
    }
}
