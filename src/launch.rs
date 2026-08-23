use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchArguments {
    pub activation_token: Option<String>,
    pub device_name: Option<String>,
}

impl LaunchArguments {
    pub fn parse_environment() -> Result<Self> {
        Self::parse(std::env::args().skip(1))
    }

    pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut activation_token = None;
        let mut device_name = None;
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
                _ => bail!("unsupported collector argument: {argument}"),
            }
        }

        if activation_token.is_none() && device_name.is_some() {
            bail!("--device-name requires --activation-token");
        }

        Ok(Self {
            activation_token,
            device_name,
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

fn set_once(target: &mut Option<String>, value: String, argument_name: &str) -> Result<()> {
    if target.is_some() {
        bail!("{argument_name} may only be specified once");
    }

    *target = Some(value);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
            }
        );
    }

    #[test]
    fn rejects_device_name_without_activation_token() {
        let result = LaunchArguments::parse(["--device-name".to_owned(), "Home PC".to_owned()]);

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
