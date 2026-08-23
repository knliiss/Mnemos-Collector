use std::path::{Path, PathBuf};

use directories::UserDirs;

pub fn default_latest_log_path() -> Option<PathBuf> {
    let user_dirs = UserDirs::new()?;

    Some(
        user_dirs
            .home_dir()
            .join(".cristalix")
            .join("updates")
            .join("Minigames")
            .join("logs")
            .join("latest.log"),
    )
}

pub fn discover_latest_log(
    cached_path: Option<&Path>,
    process_candidates: &[PathBuf],
) -> Option<PathBuf> {
    if let Some(path) = process_candidates.iter().find(|path| path.is_file()) {
        return Some(path.clone());
    }

    if let Some(path) = cached_path.filter(|path| path.is_file()) {
        return Some(path.to_path_buf());
    }

    default_latest_log_path().filter(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    #[test]
    fn default_path_ends_with_cristalix_minigames_latest_log() {
        let Some(path) = default_latest_log_path() else {
            return;
        };

        let normalized = path.to_string_lossy().replace('\\', "/");

        assert!(normalized.ends_with("/.cristalix/updates/Minigames/logs/latest.log"));
    }

    #[test]
    fn running_process_candidate_has_priority_over_cached_path() {
        let directory = std::env::temp_dir().join(format!("mnemos-discovery-{}", Uuid::now_v7()));
        let cached = directory.join("cached").join("latest.log");
        let process = directory.join("process").join("latest.log");

        fs::create_dir_all(cached.parent().unwrap()).unwrap();
        fs::create_dir_all(process.parent().unwrap()).unwrap();
        fs::write(&cached, b"").unwrap();
        fs::write(&process, b"").unwrap();

        let discovered = discover_latest_log(Some(&cached), std::slice::from_ref(&process));

        assert_eq!(discovered, Some(process));

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn cached_path_is_used_when_process_has_no_resolvable_path() {
        let directory = std::env::temp_dir().join(format!("mnemos-discovery-{}", Uuid::now_v7()));
        let cached = directory.join("latest.log");

        fs::create_dir_all(&directory).unwrap();
        fs::write(&cached, b"").unwrap();

        assert_eq!(
            discover_latest_log(Some(&cached), &[]),
            Some(cached.clone())
        );

        let _ = fs::remove_dir_all(directory);
    }
}
