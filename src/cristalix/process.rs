use std::collections::{BTreeSet, HashSet};
use std::fs::Metadata;
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use sysinfo::{Pid, Process, ProcessesToUpdate, System};

use super::default_latest_log_path;

const MAX_PARENT_DEPTH: usize = 5;
const LOG_CREATION_START_TOLERANCE: Duration = Duration::from_secs(15 * 60);
const LOG_MODIFIED_SESSION_WINDOW: Duration = Duration::from_secs(12 * 60 * 60);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CristalixProcessSnapshot {
    pub running: bool,
    pub latest_log_candidates: Vec<PathBuf>,
    pub java_processes: usize,
    pub launcher_processes: usize,
    pub direct_matches: usize,
    pub ancestry_matches: usize,
    pub session_fallback_matches: usize,
}

#[derive(Debug)]
pub struct CristalixProcessDetector {
    system: System,
}

impl Default for CristalixProcessDetector {
    fn default() -> Self {
        Self {
            system: System::new(),
        }
    }
}

impl CristalixProcessDetector {
    pub fn inspect(&mut self) -> CristalixProcessSnapshot {
        self.system.refresh_processes(ProcessesToUpdate::All, true);

        let launcher_pids = cristalix_launcher_pids(&self.system);
        let default_log_path = default_latest_log_path().filter(|path| path.is_file());
        let default_log_metadata = default_log_path
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok());
        let mut running = false;
        let mut candidates = BTreeSet::new();
        let mut java_processes = 0;
        let mut direct_matches = 0;
        let mut ancestry_matches = 0;
        let mut session_fallback_matches = 0;

        for (pid, process) in self.system.processes() {
            let name = process.name().to_string_lossy().to_ascii_lowercase();
            let java_process = is_java_process_name(&name);
            let locations = process_locations(process);
            let direct_match = locations
                .iter()
                .any(|location| references_cristalix_game(location));
            let ancestry_match = java_process
                && descends_from_cristalix_launcher(*pid, &self.system, &launcher_pids);
            let session_fallback_match = java_process
                && !direct_match
                && !ancestry_match
                && default_log_metadata.as_ref().is_some_and(|metadata| {
                    process_matches_log_session(process.start_time(), metadata)
                });

            if java_process {
                java_processes += 1;
            }

            if direct_match {
                direct_matches += 1;
            }

            if ancestry_match {
                ancestry_matches += 1;
            }

            if session_fallback_match {
                session_fallback_matches += 1;
            }

            if !direct_match && !ancestry_match && !session_fallback_match {
                continue;
            }

            running = true;
            collect_log_candidates(&locations, &mut candidates);

            if session_fallback_match
                && let Some(path) = default_log_path.as_ref()
            {
                candidates.insert(path.clone());
            }
        }

        CristalixProcessSnapshot {
            running,
            latest_log_candidates: candidates.into_iter().collect(),
            java_processes,
            launcher_processes: launcher_pids.len(),
            direct_matches,
            ancestry_matches,
            session_fallback_matches,
        }
    }

    pub fn is_running(&mut self) -> bool {
        self.inspect().running
    }
}

fn cristalix_launcher_pids(system: &System) -> HashSet<Pid> {
    system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            let name = process.name().to_string_lossy().to_ascii_lowercase();

            if name.contains("cristalix") && !name.contains("mnemos") {
                Some(*pid)
            } else {
                None
            }
        })
        .collect()
}

fn is_java_process_name(name: &str) -> bool {
    let executable = name.trim_end_matches(".exe");

    executable == "java" || executable == "javaw"
}

fn descends_from_cristalix_launcher(
    mut pid: Pid,
    system: &System,
    launcher_pids: &HashSet<Pid>,
) -> bool {
    for _ in 0..MAX_PARENT_DEPTH {
        let Some(process) = system.process(pid) else {
            return false;
        };
        let Some(parent) = process.parent() else {
            return false;
        };

        if launcher_pids.contains(&parent) {
            return true;
        }

        pid = parent;
    }

    false
}

fn process_locations(process: &Process) -> Vec<String> {
    let mut locations = process
        .cmd()
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    if let Some(executable) = process.exe() {
        locations.push(executable.to_string_lossy().into_owned());
    }

    if let Some(current_directory) = process.cwd() {
        locations.push(current_directory.to_string_lossy().into_owned());
    }

    locations
}

fn references_cristalix_game(value: &str) -> bool {
    let normalized = value.replace('\\', "/").to_ascii_lowercase();

    normalized.contains("/.cristalix/")
        && (normalized.contains("/updates/minigames/")
            || normalized.contains("/updates/minigames")
            || normalized.contains("minigames"))
}

fn process_matches_log_session(process_start: u64, metadata: &Metadata) -> bool {
    if let Ok(created) = metadata.created()
        && let Ok(created_since_epoch) = created.duration_since(UNIX_EPOCH)
    {
        return timestamps_within(
            process_start,
            created_since_epoch.as_secs(),
            LOG_CREATION_START_TOLERANCE,
        );
    }

    let Ok(modified) = metadata.modified() else {
        return false;
    };
    let Ok(modified_since_epoch) = modified.duration_since(UNIX_EPOCH) else {
        return false;
    };
    let modified_at = modified_since_epoch.as_secs();

    process_start <= modified_at
        && modified_at.saturating_sub(process_start) <= LOG_MODIFIED_SESSION_WINDOW.as_secs()
}

fn timestamps_within(left: u64, right: u64, tolerance: Duration) -> bool {
    left.abs_diff(right) <= tolerance.as_secs()
}

fn collect_log_candidates(locations: &[String], candidates: &mut BTreeSet<PathBuf>) {
    for location in locations {
        if let Some(path) = latest_log_from_location(location) {
            candidates.insert(path);
        }
    }
}

fn latest_log_from_location(location: &str) -> Option<PathBuf> {
    let normalized = location.replace('\\', "/");
    let lowercase = normalized.to_ascii_lowercase();
    let cristalix_start = lowercase.find(".cristalix")?;
    let cristalix_end = cristalix_start + ".cristalix".len();
    let root_with_prefix = normalized.get(..cristalix_end)?;
    let root = root_with_prefix
        .rsplit_once('=')
        .map_or(root_with_prefix, |(_, path)| path)
        .trim_matches(['"', '\'', ' ']);

    if root.is_empty() {
        return None;
    }

    Some(
        PathBuf::from(root)
            .join("updates")
            .join("Minigames")
            .join("logs")
            .join("latest.log"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_cristalix_root_from_windows_java_argument() {
        let path = latest_log_from_location(
            r#"-Djava.library.path=C:\Users\Player\.cristalix\updates\Minigames\natives"#,
        )
        .unwrap();
        let normalized = path.to_string_lossy().replace('\\', "/");

        assert!(
            normalized.ends_with("C:/Users/Player/.cristalix/updates/Minigames/logs/latest.log")
        );
    }

    #[test]
    fn extracts_cristalix_root_from_executable_path() {
        let path = latest_log_from_location(
            r#"C:\Users\Player\.cristalix\updates\Minigames\runtime\bin\javaw.exe"#,
        )
        .unwrap();
        let normalized = path.to_string_lossy().replace('\\', "/");

        assert!(
            normalized.ends_with("C:/Users/Player/.cristalix/updates/Minigames/logs/latest.log")
        );
    }

    #[test]
    fn extracts_cristalix_root_from_unix_argument() {
        let path = latest_log_from_location("/home/player/.cristalix/updates/Minigames/client.jar")
            .unwrap();

        assert_eq!(
            path,
            PathBuf::from("/home/player/.cristalix/updates/Minigames/logs/latest.log")
        );
    }

    #[test]
    fn recognizes_minigames_locations_case_insensitively() {
        assert!(references_cristalix_game(
            r#"C:\Users\Player\.cristalix\updates\MiniGames\runtime\bin\javaw.exe"#
        ));
    }

    #[test]
    fn does_not_treat_cristalix_launcher_path_as_game_evidence() {
        assert!(!references_cristalix_game(
            r#"C:\Users\Player\.cristalix\launcher\Cristalix.exe"#
        ));
    }

    #[test]
    fn recognizes_java_and_javaw_process_names() {
        assert!(is_java_process_name("java"));
        assert!(is_java_process_name("java.exe"));
        assert!(is_java_process_name("javaw.exe"));
        assert!(!is_java_process_name("cristalix.exe"));
    }

    #[test]
    fn accepts_session_timestamps_within_creation_tolerance() {
        let tolerance = LOG_CREATION_START_TOLERANCE;

        assert!(timestamps_within(1_000, 1_000, tolerance));
        assert!(timestamps_within(
            1_000,
            1_000 + tolerance.as_secs(),
            tolerance
        ));
    }

    #[test]
    fn rejects_session_timestamps_outside_creation_tolerance() {
        let tolerance = LOG_CREATION_START_TOLERANCE;

        assert!(!timestamps_within(
            1_000,
            1_001 + tolerance.as_secs(),
            tolerance
        ));
    }

    #[test]
    fn ignores_unrelated_process_locations() {
        assert!(latest_log_from_location("C:/Program Files/Java/bin/javaw.exe").is_none());
    }
}
