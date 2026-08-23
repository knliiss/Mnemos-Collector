#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::{Context, Result, bail};
#[cfg(not(target_os = "windows"))]
use mnemos_collector::application::CollectorApplication;
#[cfg(target_os = "windows")]
use mnemos_collector::desktop::{self, DesktopLaunchContext};
use mnemos_collector::diagnostics;
use mnemos_collector::launch::LaunchArguments;
use mnemos_collector::platform::{Autostart, Installation};
use mnemos_collector::provisioning::{ProvisioningClient, default_device_name};
use mnemos_collector::security::CredentialStore;
use mnemos_collector::update::{
    ApplyUpdateCommand, acknowledge_startup, cleanup_helper_when_possible,
};

#[tokio::main]
async fn main() {
    diagnostics::initialize();
    diagnostics::install_panic_hook();

    if let Err(error) = run().await {
        let message = format!("{error:#}");

        diagnostics::error("startup", message.clone());

        #[cfg(target_os = "windows")]
        desktop::show_fatal_error(&message);

        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    if let Some(command) = ApplyUpdateCommand::parse_environment()? {
        diagnostics::info("update", "Starting internal collector update helper");
        return command.run();
    }

    let arguments = LaunchArguments::parse_environment()?;

    if arguments.install {
        let activation_token = arguments
            .activation_token
            .as_deref()
            .context("collector installation requires an activation token")?;

        diagnostics::info(
            "installation",
            "Installing Collector into the stable per-user location",
        );

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
        let device_name = arguments
            .device_name
            .clone()
            .unwrap_or_else(default_device_name);

        diagnostics::info(
            "provisioning",
            "Provisioning Collector from command-line activation token",
        );

        ProvisioningClient::new()?
            .provision(activation_token, &device_name)
            .await
            .context("failed to provision this collector installation")?;
    }

    let access_key = CredentialStore.load()?;

    if !current_installation && access_key.is_some() {
        diagnostics::info(
            "installation",
            "Migrating provisioned Collector into stable installation",
        );

        Installation::migrate_existing_and_launch()
            .context("failed to migrate collector to stable installation")?;

        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        if access_key.is_some() {
            prepare_provisioned_startup(&arguments)?;
        }

        desktop::run(
            DesktopLaunchContext {
                current_installation,
                access_key,
            },
            tokio::runtime::Handle::current(),
        )
    }

    #[cfg(not(target_os = "windows"))]
    {
        let access_key = access_key.context(
            "collector is not provisioned; run the installer with --install --activation-token <TOKEN>",
        )?;

        prepare_provisioned_startup(&arguments)?;

        let application = CollectorApplication::new(access_key).await?;

        application.run().await
    }
}

fn prepare_provisioned_startup(arguments: &LaunchArguments) -> Result<()> {
    Autostart::ensure_enabled().context("failed to ensure collector autostart")?;

    if let (Some(health_file), Some(health_token)) = (
        arguments.update_health_file.as_deref(),
        arguments.update_health_token,
    ) {
        acknowledge_startup(health_file, health_token)
            .context("failed to acknowledge updated collector startup")?;
    }

    if let Some(helper) = arguments.cleanup_helper.clone() {
        tokio::spawn(cleanup_helper_when_possible(helper));
    }

    Ok(())
}
