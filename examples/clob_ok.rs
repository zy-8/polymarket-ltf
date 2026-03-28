use std::time::Duration;

use alloy_signer::Signer as _;
use alloy_signer_local::PrivateKeySigner;
use anyhow::{Context, Result, anyhow};
use polymarket_client_sdk::POLYGON;
use polymarket_client_sdk::clob::types::{OrderType, Side, SignatureType};
use polymarket_client_sdk::clob::{Client, Config};
use polymarket_client_sdk::gamma::Client as GammaClient;
use polymarket_ltf::config::AppConfig;
use polymarket_ltf::polymarket::market_registry::next_active_market;
use polymarket_ltf::polymarket::types::open_orders::Order;
use polymarket_ltf::polymarket::types::positions::Position;
use polymarket_ltf::polymarket::user_stream::Client as UserClient;
use polymarket_ltf::polymarket::utils::crypto_market::next_slug;
use polymarket_ltf::types::crypto::{Interval, Symbol};
use rust_decimal::Decimal;
use tokio::time::sleep;
use tracing::{info, warn};

const DEFAULT_POLL_INTERVAL_SECS: u64 = 2;

#[tokio::main]
async fn main() -> Result<()> {
    polymarket_ltf::init_process()?;
    let config = AppConfig::load().map_err(anyhow::Error::from)?;
    let trading = &config.trading;
    let (symbols, side, size, up_price, down_price, order_type, poll_interval) =
        parse_args(&trading.symbols)?;
    let signer = load_signer(&trading.private_key)?;
    let client = Client::new(&trading.host, Config::default())?
        .authentication_builder(&signer)
        .signature_type(SignatureType::GnosisSafe)
        .authenticate()
        .await
        .context("Polymarket CLOB 鉴权失败")?;
    let gamma = GammaClient::default();
    let interval = Interval::M5;
    let mut targets = Vec::with_capacity(symbols.len());
    let mut submitted = Vec::new();

    for symbol in symbols {
        let market_slug = next_slug(symbol, interval)?;
        let [up_asset_id, down_asset_id] = next_active_market(&gamma, symbol, interval)
            .await?
            .ok_or_else(|| anyhow!("下一个 5 分钟 market 不存在: {}", market_slug))?;
        targets.push((symbol, market_slug, up_asset_id, down_asset_id));
    }

    let user = UserClient::start(&client).await?;

    for (symbol, market_slug, up_asset_id, down_asset_id) in targets {
        info!(
            symbol = ?symbol,
            interval = ?interval,
            market_slug = %market_slug,
            side = ?side,
            up_asset_id = %up_asset_id,
            down_asset_id = %down_asset_id,
            up_price = %up_price,
            down_price = %down_price,
            size = %size,
            "准备挂下一个 5 分钟 market 双边限价单"
        );

        let up_order = client
            .limit_order()
            .order_type(order_type.clone())
            .token_id(up_asset_id)
            .side(side)
            .price(up_price)
            .size(size)
            .build()
            .await
            .with_context(|| format!("构建 {} up 订单失败", symbol.as_slug()))?;
        let down_order = client
            .limit_order()
            .order_type(order_type.clone())
            .token_id(down_asset_id)
            .side(side)
            .price(down_price)
            .size(size)
            .build()
            .await
            .with_context(|| format!("构建 {} down 订单失败", symbol.as_slug()))?;

        let up_signed = client
            .sign(&signer, up_order)
            .await
            .with_context(|| format!("{} up 订单签名失败", symbol.as_slug()))?;
        let down_signed = client
            .sign(&signer, down_order)
            .await
            .with_context(|| format!("{} down 订单签名失败", symbol.as_slug()))?;
        let posts = client
            .post_orders(vec![up_signed, down_signed])
            .await
            .with_context(|| format!("发送 {} 双边订单失败", symbol.as_slug()))?;

        for (label, post) in [("up", &posts[0]), ("down", &posts[1])] {
            info!(
                symbol = ?symbol,
                side_label = label,
                order_id = %post.order_id,
                status = ?post.status,
                success = post.success,
                trade_ids = ?post.trade_ids,
                "Polymarket 订单已提交"
            );
            submitted.push(post.clone());

            if post.order_id.trim().is_empty() {
                warn!(
                    symbol = ?symbol,
                    side_label = label,
                    success = post.success,
                    status = ?post.status,
                    trade_ids = ?post.trade_ids,
                    error_msg = ?post.error_msg,
                    "Polymarket 返回空 order_id，后续不会查询该订单状态"
                );
            }
        }
    }

    if submitted.iter().all(|post| post.order_id.trim().is_empty()) {
        warn!("本次下单返回的 order_id 为空，后续仅依赖用户 WS 更新挂单和持仓");
    }

    monitor_state(&user, poll_interval).await?;

    Ok(())
}

fn parse_args(
    default_symbols: &[Symbol],
) -> Result<(
    Vec<Symbol>,
    Side,
    Decimal,
    Decimal,
    Decimal,
    OrderType,
    Duration,
)> {
    let mut args = std::env::args().skip(1);

    let symbols = match args.next() {
        Some(raw) => parse_symbols(&raw)?,
        None => default_symbols.to_vec(),
    };

    let side = match args.next().as_deref() {
        Some("sell") => Side::Sell,
        Some("buy") | None => Side::Buy,
        Some(other) => return Err(anyhow!("不支持的 side: {other}，预期 buy 或 sell")),
    };

    let size = match args.next() {
        Some(raw) => raw.parse::<Decimal>().context("size 解析失败")?,
        None => Decimal::new(5, 0),
    };

    let up_price = match args.next() {
        Some(raw) => raw.parse::<Decimal>().context("up_price 解析失败")?,
        None => Decimal::new(55, 2),
    };

    let down_price = match args.next() {
        Some(raw) => raw.parse::<Decimal>().context("down_price 解析失败")?,
        None => Decimal::new(55, 2),
    };

    let order_type = match args.next().as_deref() {
        None | Some("gtc") => OrderType::GTC,
        Some("fok") => OrderType::FOK,
        Some("gtd") => OrderType::GTD,
        Some(other) => {
            return Err(anyhow!(
                "不支持的 order_type: {other}，预期 gtc、fok 或 gtd"
            ));
        }
    };

    let poll_interval = Duration::from_secs(match args.next() {
        Some(raw) => raw.parse::<u64>().context("poll_interval_secs 解析失败")?,
        None => DEFAULT_POLL_INTERVAL_SECS,
    });

    Ok((
        symbols,
        side,
        size,
        up_price,
        down_price,
        order_type,
        poll_interval,
    ))
}

fn parse_symbols(value: &str) -> Result<Vec<Symbol>> {
    let symbols = value
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
        let open_orders = sorted_open_orders(user.open_orders()?);
        let positions = sorted_positions(user.positions()?);

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
    for (index, order) in open_orders.iter().enumerate() {
        info!(
            leg = index + 1,
            order_id = %order.id,
            asset_id = %order.asset_id,
            status = ?order.status,
            size_matched = %order.size_matched,
            "Polymarket open order"
        );
    }
}

fn log_positions(positions: &[Position]) {
    for position in positions {
        info!(
            market = %position.market_id,
            asset_id = %position.asset_id,
            outcome = ?position.outcome,
            size = %position.size,
            avg_price = %position.avg_price,
            realized_pnl = %position.realized_pnl,
            buy_fee_usdc = %position.buy_fee_usdc,
            buy_fee_shares = %position.buy_fee_shares,
            sell_fee_usdc = %position.sell_fee_usdc,
            last_trade_ts = ?position.last_trade_ts,
            "本地持仓"
        );
    }
}

fn sorted_open_orders(mut open_orders: Vec<Order>) -> Vec<Order> {
    open_orders.sort_by(|left, right| left.id.cmp(&right.id));
    open_orders
}

fn sorted_positions(mut positions: Vec<Position>) -> Vec<Position> {
    positions.sort_by(|left, right| left.asset_id.cmp(&right.asset_id));
    positions
}
