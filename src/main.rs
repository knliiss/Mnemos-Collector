#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::{Context, Result};
use mnemos_collector::application::CollectorApplication;
use mnemos_collector::platform::Autostart;
use mnemos_collector::security::CredentialStore;

#[tokio::main]
async fn main() -> Result<()> {
    Autostart::ensure_enabled().context("failed to ensure collector autostart")?;

    let access_key = CredentialStore
        .load()?
        .context("collector is not provisioned with an individual access key")?;

    CollectorApplication::new(access_key).await?.run().await
}
