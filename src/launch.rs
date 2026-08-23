use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::update::paths::{require_safe_health_path, require_safe_helper_path};
use crate::update::process::{
    CLEANUP_HELPER_ARGUMENT, HEALTH_FILE_ARGUMENT, HEALTH_TOKEN_ARGUMENT,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchArguments {
    pub activation_token: Option<String>,
    pub device_name: Option<String>,
    pub update_health_file: Option<PathBuf>,
    pub update_health_token: Option<Uuid>,
    pub cleanup_helper: Option<PathBuf>,
}

impl LaunchArguments {
    pub fn parse_environment() -> Result<Self> {
        Self::parse(std::env::args().skip(1))
    }

    pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut activation_token = None;
        let mut device_name = None;
        let mut update_health_file = None;
        let mut update_health_token = None;
        let mut cleanup_helper = None;
        let mut arguments = arguments.into_iter();

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--activation-token" => {
                    let token = next_value(&mut arguments, "--activation-token")?;

                    set_once(&mut activation_token, token, "--activation-token")?;
                }
                "--device-name" => {
                    let name = next_value(&mut arguments, "--device-name")?;

                    set_once(&mut device_name, name, "--device-name")?;
                }
                HEALTH_FILE_ARGUMENT => {
                    let path = PathBuf::from(next_value(&mut arguments, HEALTH_FILE_ARGUMENT)?);

                    require_safe_health_path(&path)?;
                    set_once(&mut update_health_file, path, HEALTH_FILE_ARGUMENT)?;
                }
                HEALTH_TOKEN_ARGUMENT => {
                    let raw_token = next_value(&mut arguments, HEALTH_TOKEN_ARGUMENT)?;
                    let token = Uuid::parse_str(&raw_token)
                        .context("collector update health token is not a UUID")?;

                    set_once(&mut update_health_token, token, HEALTH_TOKEN_ARGUMENT)?;
                }
                CLEANUP_HELPER_ARGUMENT => {
                    let path = PathBuf::from(next_value(&mut arguments, CLEANUP_HELPER_ARGUMENT)?);

                    require_safe_helper_path(&path)?;
                    set_once(&mut cleanup_helper, path, CLEANUP_HELPER_ARGUMENT)?;
                }
                _ => bail!("unsupported collector argument: {argument}"),
            }
        }

        if activation_token.is_none() && device_name.is_some() {
            bail!("--device-name requires --activation-token");
        }

        if update_health_file.is_some() != update_health_token.is_some() {
            bail!("collector update health file and token must be supplied together");
        }

        Ok(Self {
            activation_token,
            device_name,
            update_health_file,
            update_health_token,
            cleanup_helper,
        })
    }
}

fn next_value(arguments: &mut impl Iterator<Item = String>, argument_name: &str) -> Result<String> {
    let Some(value) = arguments.next() else {
        bail!("{argument_name} requires a value");
    };

    if value.starts_with("--") {
        bail!("{argument_name} requires a value");
    }

    Ok(value)
}

fn set_once<T>(target: &mut Option<T>, value: T, argument_name: &str) -> Result<()> {
    if target.is_some() {
        bail!("{argument_name} may only be specified once");
    }

    *target = Some(value);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::paths::{health_file_path, helper_path};

    #[test]
    fn parses_activation_token_and_device_name() {
        let arguments = LaunchArguments::parse([
            "--activation-token".to_owned(),
            "activation-token".to_owned(),
            "--device-name".to_owned(),
            "Home PC".to_owned(),
        ])
        .unwrap();

        assert_eq!(
            arguments.activation_token.as_deref(),
            Some("activation-token")
        );
        assert_eq!(arguments.device_name.as_deref(), Some("Home PC"));
    }

    #[test]
    fn allows_normal_start_without_arguments() {
        let arguments = LaunchArguments::parse(Vec::<String>::new()).unwrap();

        assert_eq!(
            arguments,
            LaunchArguments {
                activation_token: None,
                device_name: None,
                update_health_file: None,
                update_health_token: None,
                cleanup_helper: None,
            }
        );
    }

    #[test]
    fn parses_internal_update_startup_arguments() {
        let health_file = health_file_path().unwrap();
        let helper = helper_path().unwrap();
        let health_token = Uuid::now_v7();
        let arguments = LaunchArguments::parse([
            HEALTH_FILE_ARGUMENT.to_owned(),
            health_file.to_string_lossy().into_owned(),
            HEALTH_TOKEN_ARGUMENT.to_owned(),
            health_token.to_string(),
            CLEANUP_HELPER_ARGUMENT.to_owned(),
            helper.to_string_lossy().into_owned(),
        ])
        .unwrap();

        assert_eq!(arguments.update_health_file.as_deref(), Some(health_file.as_path()));
        assert_eq!(arguments.update_health_token, Some(health_token));
        assert_eq!(arguments.cleanup_helper.as_deref(), Some(helper.as_path()));
    }

    #[test]
    fn rejects_device_name_without_activation_token() {
        let result = LaunchArguments::parse(["--device-name".to_owned(), "Home PC".to_owned()]);

        assert!(result.is_err());
    }

    #[test]
    fn rejects_partial_update_health_arguments() {
        let health_file = health_file_path().unwrap();
        let result = LaunchArguments::parse([
            HEALTH_FILE_ARGUMENT.to_owned(),
            health_file.to_string_lossy().into_owned(),
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn rejects_duplicate_arguments() {
        let result = LaunchArguments::parse([
            "--activation-token".to_owned(),
            "first".to_owned(),
            "--activation-token".to_owned(),
            "second".to_owned(),
        ]);

        assert!(result.is_err());
    }
}
