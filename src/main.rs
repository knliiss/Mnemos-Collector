use anyhow::{Context, Result};
use mnemos_collector::application::CollectorApplication;
use mnemos_collector::security::CredentialStore;

#[tokio::main]
async fn main() -> Result<()> {
    let access_key = CredentialStore
        .load()?
        .context("collector is not provisioned with an individual access key")?;

    CollectorApplication::new(access_key).await?.run().await
}
