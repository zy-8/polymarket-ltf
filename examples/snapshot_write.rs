use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Result;
use polymarket_ltf::binance::Client as BinanceClient;
use polymarket_ltf::polymarket::market_registry::{
    MarketRegistry, refresh_registry, spawn_auto_refresh, spawn_subscription_scheduler,
};
use polymarket_ltf::polymarket::orderbook_stream::Client as OrderbookStreamClient;
use polymarket_ltf::polymarket::rtds_stream::Client as RtdsStreamClient;
use polymarket_ltf::snapshot::Snapshot;
use polymarket_ltf::types::crypto::{Interval, Symbol};
use polymarket_client_sdk::gamma::Client as GammaClient;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    polymarket_ltf::logging::init();

    let mut args = std::env::args().skip(1);
    let symbol = args
        .next()
        .as_deref()
        .unwrap_or("btc")
        .parse::<Symbol>()
        .map_err(anyhow::Error::from)?;
    let intervals = parse_intervals(args.next().as_deref().unwrap_or("both"))?;
    let output_dir = PathBuf::from(args.next().unwrap_or_else(|| "data/snapshots".to_string()));

    let symbols = [symbol];

    let registry = Arc::new(RwLock::new(MarketRegistry::new()));
    let gamma = GammaClient::default();
    let initial = refresh_registry(&registry, &gamma, &symbols, &intervals).await?;
    info!(initial, ?symbol, ?intervals, output_dir = %output_dir.display(), "Initial market registry refresh loaded");

    let _registry_task = spawn_auto_refresh(Arc::clone(&registry), &symbols, &intervals);

    let orderbook = Arc::new(OrderbookStreamClient::connect().await?);
    let _scheduler_task = spawn_subscription_scheduler(
        Arc::clone(&registry),
        Arc::clone(&orderbook),
        &symbols,
        &intervals,
    );

    let binance = Arc::new(BinanceClient::connect(&symbols).await?);
    let chainlink = Arc::new(RtdsStreamClient::connect(&symbols)?);

    let mut snapshot = Snapshot::new(
        Arc::clone(&binance),
        Arc::clone(&chainlink),
        Arc::clone(&orderbook),
        Arc::clone(&registry),
    );

    loop {
        for interval in &intervals {
            match snapshot.write_csv(symbol, *interval, &output_dir)? {
                Some(_row) => {}
                None => {
                    info!(?symbol, ?interval, "Snapshot skipped because current market data is incomplete");
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn parse_intervals(value: &str) -> Result<Vec<Interval>> {
    match value {
        "5m" => Ok(vec![Interval::M5]),
        "15m" => Ok(vec![Interval::M15]),
        "both" | "all" => Ok(vec![Interval::M5, Interval::M15]),
        _ => Err(anyhow::anyhow!(
            "unsupported interval: {value}, expected 5m, 15m, or both"
        )),
    }
}
