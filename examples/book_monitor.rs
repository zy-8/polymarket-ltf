use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Result;
use polymarket_ltf::polymarket::market_registry::{
    MarketRegistry, refresh_registry, spawn_auto_refresh, spawn_subscription_scheduler,
};
use polymarket_ltf::polymarket::orderbook_stream::Client as OrderbookStreamClient;
use polymarket_ltf::types::crypto::{Interval, Symbol};
use polymarket_client_sdk::gamma::Client as GammaClient;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    polymarket_ltf::logging::init();

    let symbols = [Symbol::Btc];
    let intervals = [Interval::M5];

    let registry = Arc::new(RwLock::new(MarketRegistry::new()));
    let gamma = GammaClient::default();

    let initial = refresh_registry(&registry, &gamma, &symbols, &intervals).await?;
    info!(initial, "Initial registry refresh loaded");

    let _registry_task = spawn_auto_refresh(Arc::clone(&registry), &symbols, &intervals);

    let stream = Arc::new(OrderbookStreamClient::connect().await?);
    let _scheduler_task = spawn_subscription_scheduler(
        Arc::clone(&registry),
        Arc::clone(&stream),
        &symbols,
        &intervals,
    );

    loop {
        let current_markets = {
            let guard = registry.read().expect("registry lock poisoned");
            guard.current_market(&symbols, &intervals)?
        };

        info!(markets = current_markets.len(), "Current registered markets");

        if current_markets.is_empty() {
            info!("Current market is not registered yet");
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
