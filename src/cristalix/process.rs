use std::collections::BTreeSet;
use std::path::PathBuf;

use sysinfo::{Process, ProcessesToUpdate, System};

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

        let mut running = false;
        let mut candidates = BTreeSet::new();

        for process in self.system.processes().values() {
            if !is_cristalix_game_process(process) {
                continue;
            }

            running = true;

            for argument in process.cmd() {
                let argument = argument.to_string_lossy();

                if let Some(path) = latest_log_from_argument(&argument) {
                    candidates.insert(path);
                }
            }
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

fn is_cristalix_game_process(process: &Process) -> bool {
    let name = process.name().to_string_lossy().to_lowercase();
    let command = process
        .cmd()
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let references_minigames = command.contains("minigames");

    (command.contains(".cristalix") && references_minigames)
        || (name.contains("cristalix") && references_minigames)
}

fn latest_log_from_argument(argument: &str) -> Option<PathBuf> {
    let lowercase = argument.to_lowercase();
    let cristalix_start = lowercase.find(".cristalix")?;
    let cristalix_end = cristalix_start + ".cristalix".len();
    let root_with_prefix = argument.get(..cristalix_end)?;
    let root = root_with_prefix
        .rsplit_once('=')
        .map_or(root_with_prefix, |(_, path)| path)
        .trim_matches(['"', '\'']);

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
        let path = latest_log_from_argument(
            r#"-Djava.library.path=C:\Users\Player\.cristalix\updates\Minigames\natives"#,
        )
        .unwrap();
        let normalized = path.to_string_lossy().replace('\\', "/");

        assert!(
            normalized.ends_with("C:/Users/Player/.cristalix/updates/Minigames/logs/latest.log")
        );
    }

    #[test]
    fn extracts_cristalix_root_from_unix_argument() {
        let path = latest_log_from_argument("/home/player/.cristalix/updates/Minigames/client.jar")
            .unwrap();

        assert_eq!(
            path,
            PathBuf::from("/home/player/.cristalix/updates/Minigames/logs/latest.log")
        );
    }

    #[test]
    fn ignores_unrelated_process_arguments() {
        assert!(latest_log_from_argument("/usr/bin/java").is_none());
    }
}
