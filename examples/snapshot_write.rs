use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Result;
use polymarket_client_sdk::gamma::Client as GammaClient;
use polymarket_ltf::binance::Client as BinanceClient;
use polymarket_ltf::polymarket::market_registry::{
    MarketRegistry, refresh_registry, spawn_auto_refresh, spawn_subscription_scheduler,
};
use polymarket_ltf::polymarket::orderbook_stream::Client as OrderbookStreamClient;
use polymarket_ltf::polymarket::rtds_stream::Client as RtdsStreamClient;
use polymarket_ltf::snapshot::Snapshot;
use polymarket_ltf::types::crypto::{Interval, Symbol};
use tracing::{error, info};

#[tokio::main]
async fn main() {
    if let Err(err) = polymarket_ltf::init_process() {
        eprintln!("failed to initialize process: {err}");
        std::process::exit(1);
    }

    if let Err(err) = run().await {
        error!(error = %err, error_debug = ?err, "snapshot_write exited with error");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // 解析参数：[symbols] [interval] [output_dir]
    // symbols 用逗号分隔，例如 "btc,eth,sol,xrp"
    let symbols: Vec<Symbol> = args
        .first()
        .map(|s| s.as_str())
        .unwrap_or("btc,eth,sol,xrp")
        .split(',')
        .map(|s| s.trim().parse::<Symbol>())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(anyhow::Error::from)?;
    let intervals = parse_intervals(args.get(1).map(|s| s.as_str()).unwrap_or("both"))?;
    let output_dir = PathBuf::from(
        args.get(2)
            .cloned()
            .unwrap_or_else(|| "data/snapshots".to_string()),
    );

    let registry = Arc::new(RwLock::new(MarketRegistry::new()));
    let gamma = GammaClient::default();
    let initial = refresh_registry(&registry, &gamma, &symbols, &intervals).await?;
    info!(initial, ?symbols, ?intervals, output_dir = %output_dir.display(), "Initial market registry refresh loaded");

    let _registry_task = spawn_auto_refresh(Arc::clone(&registry), &symbols, &intervals);

    let orderbook = Arc::new(OrderbookStreamClient::connect().await?);
    let _scheduler_task = spawn_subscription_scheduler(
        Arc::clone(&registry),
        Arc::clone(&orderbook),
        &symbols,
        &intervals,
    );

    let binance = Arc::new(BinanceClient::connect().await?);
    binance.subscribe_books(&symbols)?;
    let chainlink = Arc::new(RtdsStreamClient::connect(&symbols)?);

    let mut snapshot = Snapshot::new(
        Arc::clone(&binance),
        Arc::clone(&chainlink),
        Arc::clone(&orderbook),
        Arc::clone(&registry),
    );

    loop {
        for &symbol in &symbols {
            for interval in &intervals {
                match snapshot.write_csv(symbol, *interval, &output_dir)? {
                    Some(_row) => {}
                    None => {
                        info!(
                            ?symbol,
                            ?interval,
                            "Snapshot skipped because current market data is incomplete"
                        );
                    }
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
