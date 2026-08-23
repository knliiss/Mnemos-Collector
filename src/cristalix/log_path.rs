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

pub fn discover_latest_log(cached_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = cached_path.filter(|path| path.is_file()) {
        return Some(path.to_path_buf());
    }

    default_latest_log_path().filter(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_path_ends_with_cristalix_minigames_latest_log() {
        let Some(path) = default_latest_log_path() else {
            return;
        };

        let normalized = path.to_string_lossy().replace('\\', "/");

        assert!(normalized.ends_with("/.cristalix/updates/Minigames/logs/latest.log"));
    }
}
