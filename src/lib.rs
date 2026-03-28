pub mod binance;
pub mod config;
pub mod dashboard;
pub mod errors;
pub mod events;
pub mod logging;
pub mod polymarket;
pub mod snapshot;
pub mod storage;
pub mod strategy;
pub mod types;

pub fn init_process() -> anyhow::Result<()> {
    install_rustls_crypto_provider()?;
    logging::init();
    Ok(())
}

fn install_rustls_crypto_provider() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("failed to install rustls crypto provider"))
}
