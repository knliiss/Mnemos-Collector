mod clipboard;
mod dpi;
mod mascot;
mod native_shell;
mod single_instance;
mod theme;
mod tray_popup;
mod view;
mod window_placement;

use anyhow::Result;
use tokio::runtime::Handle;

pub struct DesktopLaunchContext {
    pub current_installation: bool,
    pub access_key: Option<String>,
}

pub fn run(context: DesktopLaunchContext, runtime: Handle) -> Result<()> {
    dpi::enable_gdi_scaling_for_thread();

    let Some(_instance_guard) = single_instance::InstanceGuard::acquire()? else {
        single_instance::activate_existing_window();
        return Ok(());
    };

    let _placement_hook = window_placement::StartupPlacementHook::install()?;

    native_shell::run(context, runtime)
}

pub fn show_fatal_error(message: &str) {
    dpi::enable_gdi_scaling_for_thread();
    native_shell::show_fatal_error(message);
}
