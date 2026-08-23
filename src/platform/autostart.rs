use std::env;

use anyhow::{Context, Result};

pub struct Autostart;

impl Autostart {
    pub fn ensure_enabled() -> Result<()> {
        let executable = env::current_exe().context("failed to resolve collector executable path")?;

        platform::enable(&executable)
    }

    pub fn disable() -> Result<()> {
        platform::disable()
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::{Command, ExitStatus};

    use anyhow::{Context, Result, bail};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "Mnemos Collector";

    pub fn enable(executable: &Path) -> Result<()> {
        let command_value = format!("\"{}\"", executable.display());
        let status = run_reg(&[
            "add",
            RUN_KEY,
            "/v",
            VALUE_NAME,
            "/t",
            "REG_SZ",
            "/d",
            &command_value,
            "/f",
        ])
        .context("failed to configure Windows collector autostart")?;

        if !status.success() {
            bail!("Windows rejected the collector autostart registration");
        }

        Ok(())
    }

    pub fn disable() -> Result<()> {
        let query_status = run_reg(&["query", RUN_KEY, "/v", VALUE_NAME])
            .context("failed to inspect Windows collector autostart")?;

        if !query_status.success() {
            return Ok(());
        }

        let status = run_reg(&["delete", RUN_KEY, "/v", VALUE_NAME, "/f"])
            .context("failed to remove Windows collector autostart")?;

        if !status.success() {
            bail!("Windows rejected removal of collector autostart");
        }

        Ok(())
    }

    fn run_reg(arguments: &[&str]) -> std::io::Result<ExitStatus> {
        let mut command = Command::new("reg");

        command.args(arguments);
        command.creation_flags(CREATE_NO_WINDOW);
        command.status()
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::fs;
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result};
    use directories::UserDirs;

    const LAUNCH_AGENT_NAME: &str = "rest.knalis.mnemos-collector.plist";

    pub fn enable(executable: &Path) -> Result<()> {
        let path = launch_agent_path()?;
        let content = launch_agent_content(executable)?;

        write_if_changed(&path, &content)
    }

    pub fn disable() -> Result<()> {
        let path = launch_agent_path()?;

        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }

        Ok(())
    }

    fn launch_agent_path() -> Result<PathBuf> {
        let user_dirs = UserDirs::new().context("macOS home directory is unavailable")?;

        Ok(user_dirs
            .home_dir()
            .join("Library")
            .join("LaunchAgents")
            .join(LAUNCH_AGENT_NAME))
    }

    fn launch_agent_content(executable: &Path) -> Result<String> {
        let executable = executable
            .to_str()
            .context("collector executable path is not valid UTF-8")?;
        let executable = xml_escape(executable);

        Ok(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
    <key>Label</key>\n\
    <string>rest.knalis.mnemos-collector</string>\n\
    <key>ProgramArguments</key>\n\
    <array>\n\
        <string>{executable}</string>\n\
    </array>\n\
    <key>RunAtLoad</key>\n\
    <true/>\n\
    <key>ProcessType</key>\n\
    <string>Background</string>\n\
    <key>KeepAlive</key>\n\
    <dict>\n\
        <key>SuccessfulExit</key>\n\
        <false/>\n\
    </dict>\n\
</dict>\n\
</plist>\n"
        ))
    }

    fn xml_escape(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }

    fn write_if_changed(path: &Path, content: &str) -> Result<()> {
        if fs::read_to_string(path).ok().as_deref() == Some(content) {
            return Ok(());
        }

        let parent = path
            .parent()
            .context("macOS LaunchAgent path has no parent directory")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;

        let temporary = path.with_extension("plist.tmp");
        fs::write(&temporary, content)
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        fs::rename(&temporary, path)
            .with_context(|| format!("failed to install {}", path.display()))?;

        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn escapes_launch_agent_executable_path() {
            let content =
                launch_agent_content(Path::new("/Applications/Mnemos & Tools/collector")).unwrap();

            assert!(content.contains("/Applications/Mnemos &amp; Tools/collector"));
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use std::fs;
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result};
    use directories::BaseDirs;

    const AUTOSTART_FILE_NAME: &str = "rest.knalis.mnemos-collector.desktop";

    pub fn enable(executable: &Path) -> Result<()> {
        let path = autostart_path()?;
        let content = desktop_entry(executable)?;

        write_if_changed(&path, &content)
    }

    pub fn disable() -> Result<()> {
        let path = autostart_path()?;

        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }

        Ok(())
    }

    fn autostart_path() -> Result<PathBuf> {
        let base_dirs = BaseDirs::new().context("Linux configuration directory is unavailable")?;

        Ok(base_dirs
            .config_dir()
            .join("autostart")
            .join(AUTOSTART_FILE_NAME))
    }

    fn desktop_entry(executable: &Path) -> Result<String> {
        let executable = executable
            .to_str()
            .context("collector executable path is not valid UTF-8")?;
        let executable = desktop_exec_quote(executable);

        Ok(format!(
            "[Desktop Entry]\n\
Type=Application\n\
Version=1.0\n\
Name=Mnemos Collector\n\
Exec={executable}\n\
Terminal=false\n\
NoDisplay=true\n\
X-GNOME-Autostart-enabled=true\n"
        ))
    }

    fn desktop_exec_quote(value: &str) -> String {
        let escaped = value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$");

        format!("\"{escaped}\"")
    }

    fn write_if_changed(path: &Path, content: &str) -> Result<()> {
        if fs::read_to_string(path).ok().as_deref() == Some(content) {
            return Ok(());
        }

        let parent = path
            .parent()
            .context("Linux autostart path has no parent directory")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;

        let temporary = path.with_extension("desktop.tmp");
        fs::write(&temporary, content)
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        fs::rename(&temporary, path)
            .with_context(|| format!("failed to install {}", path.display()))?;

        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn quotes_desktop_entry_executable_path() {
            let content =
                desktop_entry(Path::new("/home/player/Mnemos Collector/collector")).unwrap();

            assert!(content.contains("Exec=\"/home/player/Mnemos Collector/collector\""));
            assert!(content.contains("Terminal=false"));
        }
    }
}
