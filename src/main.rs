#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::{Context, Result, bail};
use mnemos_collector::application::CollectorApplication;
use mnemos_collector::launch::LaunchArguments;
use mnemos_collector::platform::{Autostart, Installation};
use mnemos_collector::provisioning::{ProvisioningClient, default_device_name};
use mnemos_collector::security::CredentialStore;
use mnemos_collector::update::{
    ApplyUpdateCommand, acknowledge_startup, cleanup_helper_when_possible,
};

#[tokio::main]
async fn main() -> Result<()> {
    if let Some(command) = ApplyUpdateCommand::parse_environment()? {
        return command.run();
    }

    let arguments = LaunchArguments::parse_environment()?;

    if arguments.install {
        let activation_token = arguments
            .activation_token
            .as_deref()
            .context("collector installation requires an activation token")?;

        Installation::install_and_launch(activation_token, arguments.device_name.as_deref())
            .context("failed to install Mnemos Collector")?;

        return Ok(());
    }

    let current_installation = Installation::is_current_installation()
        .context("failed to verify collector installation location")?;

    if arguments.activation_token.is_some() && !current_installation {
        bail!(
            "collector provisioning must run from the stable installation; use --install --activation-token <TOKEN>"
        );
    }

    if let Some(activation_token) = arguments.activation_token.as_deref() {
        let device_name = arguments.device_name.unwrap_or_else(default_device_name);

        ProvisioningClient::new()?
            .provision(activation_token, &device_name)
            .await
            .context("failed to provision this collector installation")?;
    }

    let access_key = CredentialStore.load()?.context(
        "collector is not provisioned; run the installer with --install --activation-token <TOKEN>",
    )?;

    if !current_installation {
        Installation::migrate_existing_and_launch()
            .context("failed to migrate collector to stable installation")?;

        return Ok(());
    }

    Autostart::ensure_enabled().context("failed to ensure collector autostart")?;

    let application = CollectorApplication::new(access_key).await?;

    if let (Some(health_file), Some(health_token)) = (
        arguments.update_health_file.as_deref(),
        arguments.update_health_token,
    ) {
        acknowledge_startup(health_file, health_token)
            .context("failed to acknowledge updated collector startup")?;
    }

    if let Some(helper) = arguments.cleanup_helper {
        tokio::spawn(cleanup_helper_when_possible(helper));
    }

    application.run().await
}
