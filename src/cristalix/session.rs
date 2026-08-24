use std::path::Path;
use std::time::{Duration, SystemTime};

pub fn log_updated_within(path: &Path, freshness: Duration) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };

    timestamp_is_recent(modified, SystemTime::now(), freshness)
}

fn timestamp_is_recent(modified: SystemTime, now: SystemTime, freshness: Duration) -> bool {
    match now.duration_since(modified) {
        Ok(age) => age <= freshness,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_timestamp_confirms_session() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let modified = now - Duration::from_secs(45);

        assert!(timestamp_is_recent(
            modified,
            now,
            Duration::from_secs(60)
        ));
    }

    #[test]
    fn stale_timestamp_does_not_confirm_session() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let modified = now - Duration::from_secs(61);

        assert!(!timestamp_is_recent(
            modified,
            now,
            Duration::from_secs(60)
        ));
    }

    #[test]
    fn future_timestamp_is_treated_as_recent() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let modified = now + Duration::from_secs(5);

        assert!(timestamp_is_recent(
            modified,
            now,
            Duration::from_secs(60)
        ));
    }
}
