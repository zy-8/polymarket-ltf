use anyhow::Result;
use polymarket_client_sdk::clob::{Client, Config};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    polymarket_ltf::logging::init();

    let client = Client::new("https://clob.polymarket.com", Config::default())?;
    let ok = client.ok().await?;

    info!(ok, "Polymarket CLOB health check");

    Ok(())
}
