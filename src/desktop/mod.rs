#[cfg(target_os = "windows")]
#[path = "view.rs"]
mod base_view;
#[cfg(target_os = "windows")]
mod clipboard;
#[cfg(target_os = "windows")]
mod dpi;
#[cfg(target_os = "windows")]
mod mascot;
#[cfg(target_os = "windows")]
mod native_shell;
#[cfg(not(target_os = "windows"))]
mod portable;
#[cfg(not(target_os = "windows"))]
mod portable_instance;
#[cfg(target_os = "windows")]
mod single_instance;
#[cfg(target_os = "windows")]
mod theme;
#[cfg(target_os = "windows")]
mod tray_popup;
#[cfg(target_os = "windows")]
#[path = "versioned_view.rs"]
mod view;
#[cfg(target_os = "windows")]
mod window_placement;

use anyhow::Result;
use tokio::runtime::Handle;

pub struct DesktopLaunchContext {
    pub current_installation: bool,
    pub access_key: Option<String>,
}

#[cfg(target_os = "windows")]
pub fn run(context: DesktopLaunchContext, runtime: Handle) -> Result<()> {
    dpi::enable_gdi_scaling_for_thread();

    let Some(_instance_guard) = single_instance::InstanceGuard::acquire()? else {
        single_instance::activate_existing_window();
        return Ok(());
    };

    let _placement_hook = window_placement::StartupPlacementHook::install()?;

    native_shell::run(context, runtime)
}

#[cfg(not(target_os = "windows"))]
pub fn run(context: DesktopLaunchContext, runtime: Handle) -> Result<()> {
    let Some(_instance_guard) = portable_instance::InstanceGuard::acquire()? else {
        crate::diagnostics::info(
            "desktop",
            "Another Collector instance is already running; duplicate launch ignored",
        );
        return Ok(());
    };

    portable::run(context, runtime)
}

#[cfg(target_os = "windows")]
pub fn show_fatal_error(message: &str) {
    dpi::enable_gdi_scaling_for_thread();
    native_shell::show_fatal_error(message);
}

#[cfg(not(target_os = "windows"))]
pub fn show_fatal_error(message: &str) {
    portable::show_fatal_error(message);
}
