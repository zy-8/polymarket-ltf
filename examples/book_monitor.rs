use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Result;
use polymarket_client_sdk::gamma::Client as GammaClient;
use polymarket_ltf::polymarket::market_registry::{
    MarketRegistry, refresh_registry, spawn_auto_refresh, spawn_subscription_scheduler,
};
use polymarket_ltf::polymarket::orderbook_stream::Client as OrderbookStreamClient;
use polymarket_ltf::polymarket::types::orderbook::Level;
use polymarket_ltf::types::crypto::{Interval, Symbol};
use tracing::info;

#[derive(Debug, Clone, PartialEq)]
struct MarketQuotes {
    up_bid: Option<Level>,
    up_ask: Option<Level>,
    down_bid: Option<Level>,
    down_ask: Option<Level>,
}

#[tokio::main]
async fn main() -> Result<()> {
    polymarket_ltf::init_process()?;

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

    let mut last_markets = None;
    let mut last_quotes = None;

    loop {
        let current_markets = {
            let guard = registry.read().expect("registry lock poisoned");
            guard.current_market(&symbols, &intervals)?
        };
        let current_quotes = current_markets
            .iter()
            .map(|[up_asset_id, down_asset_id]| MarketQuotes {
                up_bid: stream.best_bid(up_asset_id),
                up_ask: stream.best_ask(up_asset_id),
                down_bid: stream.best_bid(down_asset_id),
                down_ask: stream.best_ask(down_asset_id),
            })
            .collect::<Vec<_>>();

        if last_markets.as_ref() != Some(&current_markets) {
            info!(
                markets = current_markets.len(),
                "Current registered markets"
            );

            if current_markets.is_empty() {
                info!("Current market is not registered yet");
            }

            last_markets = Some(current_markets.clone());
        }

        if last_quotes.as_ref() != Some(&current_quotes) {
            let latest_quotes = current_markets
                .iter()
                .zip(&current_quotes)
                .map(|(_, quotes)| {
                    format!(
                        "up: {} | down: {}",
                        format_side(quotes.up_bid, quotes.up_ask),
                        format_side(quotes.down_bid, quotes.down_ask),
                    )
                })
                .collect::<Vec<_>>();

            info!(quotes = ?latest_quotes, "Current market best bid/ask");
            last_quotes = Some(current_quotes);
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn format_side(bid: Option<Level>, ask: Option<Level>) -> String {
    format!("bid={} ask={}", format_level(bid), format_level(ask))
}

fn format_level(level: Option<Level>) -> String {
    match level {
        Some(level) => format!("{}@{}", level.price.round_dp(6), level.size.round_dp(6)),
        None => "0".to_string(),
    }
}
