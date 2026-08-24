use std::cmp::Ordering;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CollectorVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

impl CollectorVersion {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl FromStr for CollectorVersion {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let mut parts = value.split('.');
        let Some(major) = parts.next() else {
            bail!("collector version is missing its major component");
        };
        let Some(minor) = parts.next() else {
            bail!("collector version is missing its minor component");
        };
        let Some(patch) = parts.next() else {
            bail!("collector version is missing its patch component");
        };

        if parts.next().is_some() {
            bail!("collector version must use major.minor.patch format");
        }

        Ok(Self {
            major: parse_component(major)?,
            minor: parse_component(minor)?,
            patch: parse_component(patch)?,
        })
    }
}

impl Display for CollectorVersion {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl PartialOrd for CollectorVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CollectorVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch))
    }
}

fn parse_component(component: &str) -> Result<u64> {
    if component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("collector version components must contain decimal digits only");
    }

    if component.len() > 1 && component.starts_with('0') {
        bail!("collector version components must not contain leading zeroes");
    }

    component
        .parse()
        .map_err(|_| anyhow::anyhow!("collector version component is too large"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_stable_versions_numerically() {
        let current = CollectorVersion::from_str("0.9.10").unwrap();
        let next = CollectorVersion::from_str("0.10.0").unwrap();

        assert!(next > current);
    }

    #[test]
    fn rejects_non_stable_or_ambiguous_versions() {
        assert!(CollectorVersion::from_str("1.2").is_err());
        assert!(CollectorVersion::from_str("1.2.3-beta").is_err());
        assert!(CollectorVersion::from_str("01.2.3").is_err());
        assert!(CollectorVersion::from_str("1.2.3.4").is_err());
    }
}
