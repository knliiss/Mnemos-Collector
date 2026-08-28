use std::path::{Path, PathBuf};

use directories::UserDirs;

const CRISTALIX_LOG_SUFFIX: [&str; 4] = ["updates", "Minigames", "logs", "latest.log"];

pub fn default_latest_log_path() -> Option<PathBuf> {
    known_latest_log_paths().into_iter().next()
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

    known_latest_log_paths()
        .into_iter()
        .find(|path| path.is_file())
}

pub fn known_latest_log_paths() -> Vec<PathBuf> {
    let Some(user_dirs) = UserDirs::new() else {
        return Vec::new();
    };

    known_latest_log_paths_for_home(user_dirs.home_dir())
}

fn known_latest_log_paths_for_home(home: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "macos")]
    {
        paths.push(latest_log_in_cristalix_root(
            home.join("Library")
                .join("Application Support")
                .join("cristalix"),
        ));
        paths.push(latest_log_in_cristalix_root(
            home.join("Library")
                .join("Application Support")
                .join(".cristalix"),
        ));
    }

    paths.push(latest_log_in_cristalix_root(home.join(".cristalix")));

    paths
}

fn latest_log_in_cristalix_root(root: PathBuf) -> PathBuf {
    CRISTALIX_LOG_SUFFIX
        .iter()
        .fold(root, |path, component| path.join(component))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use uuid::Uuid;

    use super::*;

    #[test]
    fn known_paths_keep_the_cristalix_minigames_log_layout() {
        let paths = known_latest_log_paths_for_home(Path::new("/Users/player"));

        assert!(!paths.is_empty());
        assert!(paths.iter().all(|path| {
            path.to_string_lossy()
                .replace('\\', "/")
                .ends_with("/updates/Minigames/logs/latest.log")
        }));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_prefers_application_support_cristalix() {
        let paths = known_latest_log_paths_for_home(Path::new("/Users/player"));
        let first = paths.first().unwrap().to_string_lossy().replace('\\', "/");

        assert_eq!(
            first,
            "/Users/player/Library/Application Support/cristalix/updates/Minigames/logs/latest.log"
        );
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

        assert_eq!(discover_latest_log(Some(&cached), &[]), Some(cached.clone()));

        let _ = fs::remove_dir_all(directory);
    }
}
