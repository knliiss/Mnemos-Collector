use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;

use sysinfo::{Pid, Process, ProcessesToUpdate, System};

const MAX_PARENT_DEPTH: usize = 5;
const CRISTALIX_LOG_SUFFIX: [&str; 4] = ["updates", "Minigames", "logs", "latest.log"];

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
        let mut running = false;
        let mut candidates = BTreeSet::new();
        let mut java_processes = 0;
        let mut direct_matches = 0;
        let mut ancestry_matches = 0;

        for (pid, process) in self.system.processes() {
            let name = process.name().to_string_lossy().to_ascii_lowercase();
            let java_process = is_java_process_name(&name);
            let locations = process_locations(process);
            let direct_match = locations
                .iter()
                .any(|location| references_cristalix_game(location));
            let ancestry_match = java_process
                && descends_from_cristalix_launcher(*pid, &self.system, &launcher_pids);

            if java_process {
                java_processes += 1;
            }

            if direct_match {
                direct_matches += 1;
            }

            if ancestry_match {
                ancestry_matches += 1;
            }

            if !direct_match && !ancestry_match {
                continue;
            }

            running = true;
            collect_log_candidates(&locations, &mut candidates);
        }

        CristalixProcessSnapshot {
            running,
            latest_log_candidates: candidates.into_iter().collect(),
            java_processes,
            launcher_processes: launcher_pids.len(),
            direct_matches,
            ancestry_matches,
            session_fallback_matches: 0,
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
    let Some(path) = extract_path_value(value) else {
        return false;
    };
    let normalized = path.replace('\\', "/").to_ascii_lowercase();

    cristalix_root_end(&normalized).is_some()
        && normalized.contains("minigames")
        && !normalized.contains("mnemos-collector")
}

fn collect_log_candidates(locations: &[String], candidates: &mut BTreeSet<PathBuf>) {
    for location in locations {
        if let Some(path) = explicit_log_from_location(location) {
            candidates.insert(path);
        }

        if let Some(path) = latest_log_from_location(location) {
            candidates.insert(path);
        }
    }
}

fn explicit_log_from_location(location: &str) -> Option<PathBuf> {
    let value = extract_path_value(location)?;

    if value.to_ascii_lowercase().ends_with(".log") {
        Some(PathBuf::from(value))
    } else {
        None
    }
}

fn latest_log_from_location(location: &str) -> Option<PathBuf> {
    let value = extract_path_value(location)?;
    let normalized = value.replace('\\', "/");
    let lowercase = normalized.to_ascii_lowercase();
    let root_end = cristalix_root_end(&lowercase)?;
    let root = normalized.get(..root_end)?.trim();

    if root.is_empty() {
        return None;
    }

    Some(
        CRISTALIX_LOG_SUFFIX
            .iter()
            .fold(PathBuf::from(root), |path, component| path.join(component)),
    )
}

fn cristalix_root_end(normalized_lowercase: &str) -> Option<usize> {
    ["/.cristalix/", "/cristalix/"]
        .into_iter()
        .filter_map(|marker| {
            normalized_lowercase
                .find(marker)
                .map(|start| start + marker.len() - 1)
        })
        .min()
        .or_else(|| {
            ["/.cristalix", "/cristalix"]
                .into_iter()
                .find_map(|marker| {
                    normalized_lowercase
                        .strip_suffix(marker)
                        .map(|prefix| prefix.len() + marker.len())
                })
        })
}

fn extract_path_value(location: &str) -> Option<&str> {
    let value = location
        .rsplit_once('=')
        .map_or(location, |(_, value)| value)
        .trim_matches(['"', '\'', ' ']);

    if value.is_empty() || (!value.contains('/') && !value.contains('\\')) {
        return None;
    }

    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_log_from_windows_dot_cristalix_root() {
        let path = latest_log_from_location(
            r#"-Djava.library.path=C:\Users\Player\.cristalix\updates\Minigames\natives"#,
        )
        .unwrap();
        let normalized = path.to_string_lossy().replace('\\', "/");

        assert_eq!(
            normalized,
            "C:/Users/Player/.cristalix/updates/Minigames/logs/latest.log"
        );
    }

    #[test]
    fn extracts_log_from_macos_application_support_cristalix_root() {
        let path = latest_log_from_location(
            "/Users/player/Library/Application Support/cristalix/updates/Minigames/runtime/bin/java",
        )
        .unwrap();

        assert_eq!(
            path,
            PathBuf::from(
                "/Users/player/Library/Application Support/cristalix/updates/Minigames/logs/latest.log"
            )
        );
    }

    #[test]
    fn accepts_explicit_log_path_outside_the_cristalix_directory() {
        let path = explicit_log_from_location(
            "-Dcristalix.log=/Volumes/Games/Custom Logs/current-session.log",
        )
        .unwrap();

        assert_eq!(
            path,
            PathBuf::from("/Volumes/Games/Custom Logs/current-session.log")
        );
    }

    #[test]
    fn ignores_minigames_path_without_cristalix_root() {
        assert!(
            latest_log_from_location("/Volumes/Games/SomeOtherLauncher/Minigames/client.jar")
                .is_none()
        );
    }

    #[test]
    fn recognizes_cristalix_game_locations_case_insensitively() {
        assert!(references_cristalix_game(
            r#"C:\Games\Cristalix\MiniGames\runtime\bin\javaw.exe"#
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
    fn ignores_unrelated_process_locations() {
        assert!(latest_log_from_location("C:/Program Files/Java/bin/javaw.exe").is_none());
    }
}
