use std::time::Duration;

use alloy_signer::Signer as _;
use alloy_signer_local::PrivateKeySigner;
use anyhow::{Context, Result, anyhow};
use polymarket_client_sdk::POLYGON;
use polymarket_client_sdk::clob::types::OrderStatusType;
use polymarket_client_sdk::clob::types::SignatureType;
use polymarket_client_sdk::clob::{Client, Config};
use polymarket_ltf::config::AppConfig;
use polymarket_ltf::polymarket::types::open_orders::Order;
use polymarket_ltf::polymarket::types::positions::Position;
use polymarket_ltf::polymarket::user_stream::Client as UserClient;
use polymarket_ltf::types::crypto::Symbol;
use rust_decimal::Decimal;
use tokio::time::sleep;
use tracing::info;

const DEFAULT_POLL_INTERVAL_SECS: u64 = 2;

#[tokio::main]
async fn main() -> Result<()> {
    polymarket_ltf::init_process()?;

    let config = AppConfig::load().map_err(anyhow::Error::from)?;
    let symbols = parse_symbols(config.trading.symbols.as_slice())?;
    let signer = load_signer(&config.trading.private_key)?;
    let client = Client::new(&config.trading.host, Config::default())?
        .authentication_builder(&signer)
        .signature_type(SignatureType::GnosisSafe)
        .authenticate()
        .await
        .context("Polymarket CLOB 鉴权失败")?;

    let user = UserClient::start(&client).await?;
    info!(symbols = ?symbols, "用户账户监控已启动");

    monitor_state(&user, Duration::from_secs(DEFAULT_POLL_INTERVAL_SECS)).await
}

fn parse_symbols(default_symbols: &[Symbol]) -> Result<Vec<Symbol>> {
    let mut args = std::env::args().skip(1);
    let Some(raw) = args.next() else {
        return Ok(default_symbols.to_vec());
    };

    let symbols = raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<Symbol>().map_err(anyhow::Error::from))
        .collect::<Result<Vec<_>>>()?;

    if symbols.is_empty() {
        return Err(anyhow!("symbols 不能为空，例如 btc,eth,sol"));
    }

    Ok(symbols)
}

fn load_signer(private_key: &str) -> Result<PrivateKeySigner> {
    Ok(private_key
        .parse::<PrivateKeySigner>()
        .context("私钥解析失败")?
        .with_chain_id(Some(POLYGON)))
}

async fn monitor_state(user: &UserClient, poll_interval: Duration) -> Result<()> {
    let mut last_open_orders = Vec::new();
    let mut last_positions = Vec::new();

    loop {
        let open_orders = sorted_open_orders(visible_open_orders(user.open_orders()?));
        let positions = sorted_positions(visible_positions(user.positions()?));

        if open_orders != last_open_orders {
            log_open_orders(&open_orders);
            last_open_orders = open_orders;
        }

        if positions != last_positions {
            log_positions(&positions);
            last_positions = positions;
        }

        sleep(poll_interval).await;
    }
}

fn log_open_orders(open_orders: &[Order]) {
    if open_orders.is_empty() {
        info!("当前无活跃挂单");
        return;
    }

    for order in open_orders {
        info!(
            order_id = %order.id,
            outcome = ?order.outcome,
            side = ?order.side,
            price = %order.price,
            remaining_size = %(order.original_size - order.size_matched),
            size_matched = %order.size_matched,
            "Polymarket open order"
        );
    }
}

fn log_positions(positions: &[Position]) {
    if positions.is_empty() {
        info!("当前无本地持仓");
        return;
    }

    for position in positions {
        info!(
            outcome = ?position.outcome,
            size = %position.size,
            avg_price = %position.avg_price,
            realized_pnl = %position.realized_pnl,
            last_trade_ts = ?position.last_trade_ts,
            "本地持仓"
        );
    }
}

fn visible_open_orders(open_orders: Vec<Order>) -> Vec<Order> {
    open_orders
        .into_iter()
        .filter(|order| {
            !matches!(
                order.status,
                OrderStatusType::Matched | OrderStatusType::Canceled
            ) && order.original_size > order.size_matched
        })
        .collect()
}

fn visible_positions(positions: Vec<Position>) -> Vec<Position> {
    positions
        .into_iter()
        .filter(|position| position.size != Decimal::ZERO)
        .collect()
}

fn sorted_open_orders(mut open_orders: Vec<Order>) -> Vec<Order> {
    open_orders.sort_by(|left, right| left.id.cmp(&right.id));
    open_orders
}

fn sorted_positions(mut positions: Vec<Position>) -> Vec<Position> {
    positions.sort_by(|left, right| left.asset_id.cmp(&right.asset_id));
    positions
}
