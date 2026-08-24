mod dpi;
mod mascot;
mod native_shell;
mod theme;
mod tray_popup;
mod view;

use anyhow::Result;
use tokio::runtime::Handle;

pub struct DesktopLaunchContext {
    pub current_installation: bool,
    pub access_key: Option<String>,
}

pub fn run(context: DesktopLaunchContext, runtime: Handle) -> Result<()> {
    dpi::enable_gdi_scaling_for_thread();
    native_shell::run(context, runtime)
}

pub fn show_fatal_error(message: &str) {
    dpi::enable_gdi_scaling_for_thread();
    native_shell::show_fatal_error(message);
}
