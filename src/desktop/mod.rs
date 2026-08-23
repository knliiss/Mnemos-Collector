mod mascot;
mod window;

use anyhow::Result;
use tokio::runtime::Handle;

pub struct DesktopLaunchContext {
    pub current_installation: bool,
    pub access_key: Option<String>,
}

pub fn run(context: DesktopLaunchContext, runtime: Handle) -> Result<()> {
    window::run(context, runtime)
}

pub fn show_fatal_error(message: &str) {
    window::show_fatal_error(message);
}
