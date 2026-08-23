use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use sysinfo::{Pid, Process, ProcessesToUpdate, System};

const MAX_PARENT_DEPTH: usize = 5;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CristalixProcessSnapshot {
    pub running: bool,
    pub latest_log_candidates: Vec<PathBuf>,
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

        for (pid, process) in self.system.processes() {
            if !is_cristalix_game_process(*pid, process, &self.system, &launcher_pids) {
                continue;
            }

            running = true;
            collect_log_candidates(process, &mut candidates);
        }

        CristalixProcessSnapshot {
            running,
            latest_log_candidates: candidates.into_iter().collect(),
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

fn is_cristalix_game_process(
    pid: Pid,
    process: &Process,
    system: &System,
    launcher_pids: &HashSet<Pid>,
) -> bool {
    let name = process.name().to_string_lossy().to_ascii_lowercase();
    let java_process = is_java_process_name(&name);
    let direct_evidence = process_locations(process)
        .iter()
        .any(|location| references_cristalix_game(location));

    if java_process && direct_evidence {
        return true;
    }

    java_process && descends_from_cristalix_launcher(pid, system, launcher_pids)
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

fn collect_log_candidates(process: &Process, candidates: &mut BTreeSet<PathBuf>) {
    for location in process_locations(process) {
        if let Some(path) = latest_log_from_location(&location) {
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
    fn ignores_unrelated_process_locations() {
        assert!(latest_log_from_location("C:/Program Files/Java/bin/javaw.exe").is_none());
    }
}
