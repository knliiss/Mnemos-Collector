#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::{Context, Result};
use mnemos_collector::application::CollectorApplication;
use mnemos_collector::launch::LaunchArguments;
use mnemos_collector::platform::Autostart;
use mnemos_collector::provisioning::{ProvisioningClient, default_device_name};
use mnemos_collector::security::CredentialStore;

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = LaunchArguments::parse_environment()?;

    if let Some(activation_token) = arguments.activation_token.as_deref() {
        let device_name = arguments
            .device_name
            .unwrap_or_else(default_device_name);

        ProvisioningClient::new()?
            .provision(activation_token, &device_name)
            .await
            .context("failed to provision this collector installation")?;
    }

    let access_key = CredentialStore
        .load()?
        .context(
            "collector is not provisioned; launch it once with --activation-token <TOKEN>",
        )?;

    Autostart::ensure_enabled().context("failed to ensure collector autostart")?;

    CollectorApplication::new(access_key).await?.run().await
}
