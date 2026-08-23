use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

const HASH_BUFFER_SIZE: usize = 64 * 1024;

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("failed to open {} for SHA-256 verification", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; HASH_BUFFER_SIZE];

    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read {} for SHA-256 verification", path.display()))?;

        if read == 0 {
            break;
        }

        hasher.update(&buffer[..read]);
    }

    Ok(hex_digest(hasher.finalize()))
}

pub fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        bail!("collector update SHA-256 must be 64 lowercase hexadecimal characters");
    }

    Ok(())
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sha256_as_lowercase_hexadecimal() {
        assert_eq!(
            hex_digest(Sha256::digest(b"collector-binary")),
            "b2e64f429b4ede4f8873ae95cc9e31fcc21905da23acd0d42bb47c48db1e0acb"
        );
    }

    #[test]
    fn rejects_noncanonical_sha256() {
        assert!(validate_sha256("short").is_err());
        assert!(
            validate_sha256(
                "B2E64F429B4EDE4F8873AE95CC9E31FCC21905DA23ACD0D42BB47C48DB1E0ACB"
            )
            .is_err()
        );
    }
}
