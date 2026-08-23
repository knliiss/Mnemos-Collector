use sysinfo::{ProcessesToUpdate, System};

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
    pub fn is_running(&mut self) -> bool {
        self.system.refresh_processes(ProcessesToUpdate::All, true);

        self.system.processes().values().any(is_cristalix_process)
    }
}

fn is_cristalix_process(process: &sysinfo::Process) -> bool {
    let name = process.name().to_string_lossy().to_lowercase();

    if name.contains("cristalix") {
        return true;
    }

    let command = process
        .cmd()
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();

    command.contains(".cristalix") && command.contains("minigames")
}
