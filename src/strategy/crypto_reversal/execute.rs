//! `crypto_reversal` 的最小提交入口。
//!
//! 这个模块只负责：
//! - 信号去重；
//! - 提交前账户约束检查；
//! - 最小 sizing；
//! - 订单提交和本地写入。
//!
//! 它不负责：
//! - 候选评估；
//! - 用户账户状态同步；
//! - 复杂执行生命周期。

use std::collections::HashSet;
use std::str::FromStr;
use std::sync::{Arc, Mutex, RwLock};

use alloy_signer::Signer;
use chrono::Utc;
use polymarket_client_sdk::auth::{Normal, state::Authenticated};
use polymarket_client_sdk::clob::Client as ClobClient;
use polymarket_client_sdk::clob::types::OrderStatusType;
use polymarket_client_sdk::clob::types::request::PriceRequest;
use polymarket_client_sdk::clob::types::{Side as ClobSide, Side as MarketSide};
use polymarket_client_sdk::types::U256;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use tracing::{info, warn};

use crate::config::RuntimeConfig;
use crate::errors::{PolyfillError, Result};
use crate::events;
use crate::polymarket::market_registry::MarketRegistry;
use crate::storage::sqlite::Store;
use crate::strategy::crypto_reversal::constants;
use crate::strategy::crypto_reversal::model::Side;
use crate::strategy::crypto_reversal::service::Candidate;

/// 进程内最小执行状态。
///
/// 这里的 `submitted_windows` 在一次成功提交后不会释放。
/// 这是当前 v1 的刻意行为：同一 signal window 只允许提交一次。
#[derive(Debug, Default)]
pub struct State {
    submitted_windows: Mutex<HashSet<String>>,
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    fn reserve(&self, candidate: &Candidate) -> Result<bool> {
        let key = window_key(candidate);
        let mut guard = self
            .submitted_windows
            .lock()
            .map_err(|_| PolyfillError::internal_simple("策略执行状态锁已被污染"))?;
        Ok(guard.insert(key))
    }

    fn release(&self, candidate: &Candidate) -> Result<()> {
        let key = window_key(candidate);
        let mut guard = self
            .submitted_windows
            .lock()
            .map_err(|_| PolyfillError::internal_simple("策略执行状态锁已被污染"))?;
        guard.remove(&key);
        Ok(())
    }
}

struct Reservation<'a> {
    state: &'a State,
    candidate: &'a Candidate,
    keep: bool,
}

impl<'a> Reservation<'a> {
    fn acquire(state: &'a State, candidate: &'a Candidate) -> Result<Option<Self>> {
        if !state.reserve(candidate)? {
            return Ok(None);
        }

        Ok(Some(Self {
            state,
            candidate,
            keep: false,
        }))
    }

    fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if !self.keep {
            let _ = self.state.release(self.candidate);
        }
    }
}

/// 最小提交结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Submission {
    pub asset_id: U256,
    pub order_id: String,
    pub trade_ids: Vec<String>,
    pub status: String,
    pub success: bool,
    pub price: Decimal,
    pub size: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    MarketAssetMissing,
    WindowAlreadySubmitted,
    ExistingOpenOrder,
    ExistingPosition,
    PriceUnavailable,
    SizeUnavailable,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MarketAssetMissing => "market_asset_missing",
            Self::WindowAlreadySubmitted => "window_already_submitted",
            Self::ExistingOpenOrder => "existing_open_order",
            Self::ExistingPosition => "existing_position",
            Self::PriceUnavailable => "price_unavailable",
            Self::SizeUnavailable => "size_unavailable",
        }
    }

    pub fn detail_cn(self) -> &'static str {
        match self {
            Self::MarketAssetMissing => "当前 market 没有解析出可下单的 asset_id",
            Self::WindowAlreadySubmitted => "同一个 signal window 已经提交过，不再重复挂单",
            Self::ExistingOpenOrder => "当前 token 仍有未完成买单，避免重复挂单",
            Self::ExistingPosition => "当前 token 仍有非 dust 持仓，避免重复开仓",
            Self::PriceUnavailable => "Polymarket 报价不可用或价格超过入场上限",
            Self::SizeUnavailable => "下单数量不可用，可能是价格或目标名义金额无效",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Attempt {
    Submitted(Submission),
    Skipped(SkipReason),
}

pub async fn submit<S: Signer>(
    candidate: &Candidate,
    runtime: &RuntimeConfig,
    state: &State,
    registry: &Arc<RwLock<MarketRegistry>>,
    client: &ClobClient<Authenticated<Normal>>,
    signer: &S,
    store: &Store,
    user: &crate::polymarket::user_stream::Client,
) -> Result<Attempt> {
    let Some((asset_id, _)) = asset_ids(candidate, registry)? else {
        return Ok(Attempt::Skipped(SkipReason::MarketAssetMissing));
    };

    let Some(reservation) = Reservation::acquire(state, candidate)? else {
        return Ok(Attempt::Skipped(SkipReason::WindowAlreadySubmitted));
    };

    if has_open_order(user, &asset_id)? {
        return Ok(Attempt::Skipped(SkipReason::ExistingOpenOrder));
    }
    if has_position(user, &asset_id)? {
        return Ok(Attempt::Skipped(SkipReason::ExistingPosition));
    }

    let Some(submit_price) = order_price(runtime, &asset_id, client).await? else {
        return Ok(Attempt::Skipped(SkipReason::PriceUnavailable));
    };
    let Some(size) = size(candidate, runtime, submit_price)? else {
        return Ok(Attempt::Skipped(SkipReason::SizeUnavailable));
    };
    let order = client
        .limit_order()
        .order_type(constants::ORDER_TYPE)
        .token_id(asset_id)
        .side(MarketSide::Buy)
        .price(submit_price)
        .size(size)
        .build()
        .await
        .map_err(|error| {
            PolyfillError::internal_simple(format!("构建 Polymarket 限价单失败: {error}"))
        })?;

    let signed = client.sign(signer, order).await.map_err(|error| {
        PolyfillError::internal_simple(format!("Polymarket 订单签名失败: {error}"))
    })?;

    let post = client.post_order(signed).await.map_err(|error| {
        PolyfillError::internal_simple(format!("Polymarket 提交订单失败: {error}"))
    })?;
    reservation.keep();
    spawn_cancel_if_unfilled(candidate, client.clone(), post.order_id.clone());
    persist_strategy_submission(store, candidate, &asset_id, &post.order_id).await;

    Ok(Attempt::Submitted(Submission {
        asset_id,
        order_id: post.order_id,
        trade_ids: post.trade_ids,
        status: format!("{:?}", post.status).to_ascii_lowercase(),
        success: post.success,
        price: submit_price,
        size,
    }))
}

fn window_key(candidate: &Candidate) -> String {
    format!(
        "window:{}:{}:{}:{}:{}",
        candidate.symbol.as_slug(),
        candidate.interval.as_slug(),
        candidate.market_slug,
        side_name(candidate.side),
        candidate.signal_time_ms
    )
}

fn size(
    candidate: &Candidate,
    runtime: &RuntimeConfig,
    submit_price: Decimal,
) -> Result<Option<Decimal>> {
    let price = submit_price
        .to_f64()
        .ok_or_else(|| PolyfillError::internal_simple("submit_price 转 f64 失败"))?;
    if !(price > 0.0) {
        return Ok(None);
    }

    let target_notional = if candidate.size_factor < 1.0 {
        runtime.reduce_order_usdc
    } else {
        runtime.allow_order_usdc
    };
    if target_notional <= 0.0 {
        return Ok(None);
    }

    let shares = effective_order_size_shares(target_notional / price, price);
    if shares <= 0.0 {
        return Ok(None);
    }

    Ok(Some(decimal_from_f64(shares)?))
}

fn decimal_from_f64(value: f64) -> Result<Decimal> {
    Decimal::from_str(&value.to_string())
        .map_err(|error| PolyfillError::internal_simple(format!("shares 转 Decimal 失败: {error}")))
}

fn normalize_shares(shares: f64) -> f64 {
    if shares <= 0.0 {
        0.0
    } else {
        (shares * 100.0).floor() / 100.0
    }
}

fn effective_order_size_shares(shares: f64, price: f64) -> f64 {
    let normalized = normalize_shares(shares);
    let min_notional_shares = if price > 0.0 {
        ((constants::POLY_MIN_ORDER_NOTIONAL_USDC / price) * 100.0).ceil() / 100.0
    } else {
        0.0
    };
    let minimum = constants::POLY_MIN_ORDER_SIZE_SHARES.max(min_notional_shares);

    if normalized <= 0.0 {
        0.0
    } else if normalized < minimum {
        minimum
    } else {
        normalized
    }
}

fn asset_ids(
    candidate: &Candidate,
    registry: &Arc<RwLock<MarketRegistry>>,
) -> Result<Option<(U256, U256)>> {
    let market = registry
        .read()
        .map_err(|_| PolyfillError::internal_simple("Polymarket market registry 读锁已被污染"))?
        .get(&candidate.market_slug);

    Ok(market.map(|[up, down]| match candidate.side {
        Side::Up => (up, down),
        Side::Down => (down, up),
    }))
}

fn has_open_order(user: &crate::polymarket::user_stream::Client, asset_id: &U256) -> Result<bool> {
    // `user.open_orders()` 已经是本地 canonical 活跃挂单视图。
    // 这里不需要额外查交易所，也不需要重复判断终态状态。
    //
    // 对当前策略来说，只要同一 token 仍有未成交剩余买单，就视为重复挂单。
    // `asset_id` 本身就是 market-specific token id，因此这里直接按 token 去重即可。
    Ok(user.open_orders()?.into_iter().any(|order| {
        order.asset_id == *asset_id
            && order.side == MarketSide::Buy
            && order.original_size > order.size_matched
    }))
}

fn has_position(user: &crate::polymarket::user_stream::Client, asset_id: &U256) -> Result<bool> {
    // 当前持仓也是按 token 读取，因此这里继续按 `asset_id` 判断。
    // 但和挂单不同，持仓里可能会有极小残仓；这些残仓不应永久阻塞新单。
    //
    // 当前先用一个固定 dust 阈值过滤掉极小残仓，避免把近似归零的位置当成真实持仓。
    Ok(user.positions()?.into_iter().any(|position| {
        position.asset_id == *asset_id && position.size > *constants::POSITION_DUST_THRESHOLD
    }))
}

fn configured_order_price(runtime: &RuntimeConfig) -> Option<Decimal> {
    runtime.crypto_reversal_order_price.clone()
}

fn within_entry_price(price: Decimal) -> Option<Decimal> {
    if price > *constants::MAX_ENTRY_PRICE {
        None
    } else {
        Some(price)
    }
}

async fn order_price(
    runtime: &RuntimeConfig,
    asset_id: &U256,
    client: &ClobClient<Authenticated<Normal>>,
) -> Result<Option<Decimal>> {
    if let Some(price) = configured_order_price(runtime) {
        return Ok(within_entry_price(price));
    }

    let response = client
        .price(
            &PriceRequest::builder()
                .token_id(*asset_id)
                .side(ClobSide::Buy)
                .build(),
        )
        .await
        .map_err(|error| {
            PolyfillError::internal_simple(format!("查询 Polymarket next market 价格失败: {error}"))
        })?;
    Ok(within_entry_price(response.price))
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Up => "up",
        Side::Down => "down",
    }
}

async fn persist_strategy_submission(
    store: &Store,
    candidate: &Candidate,
    asset_id: &U256,
    order_id: &str,
) {
    let event = events::Strategy::from_candidate(
        constants::STRATEGY_NAME,
        order_id.to_string(),
        asset_id.to_string(),
        candidate,
        String::new(),
    );

    if let Err(error) = store.insert_strategy(&event).await {
        warn!(
            order_id = %order_id,
            market_slug = %candidate.market_slug,
            symbol = candidate.symbol.as_slug(),
            interval = candidate.interval.as_slug(),
            error = %error,
            "failed to persist strategy attribution after successful order submission"
        );
    }
}

fn spawn_cancel_if_unfilled(
    candidate: &Candidate,
    client: ClobClient<Authenticated<Normal>>,
    order_id: String,
) {
    let deadline_ms = cancel_deadline_ms(candidate);

    tokio::spawn(async move {
        let wait_ms = (deadline_ms - Utc::now().timestamp_millis()).max(0) as u64;
        if wait_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
        }

        let order = match client.order(&order_id).await {
            Ok(order) => order,
            Err(error) => {
                warn!(order_id = %order_id, error = %error, "failed to query order before cancel deadline");
                return;
            }
        };

        if order.size_matched > Decimal::ZERO
            || matches!(
                order.status,
                OrderStatusType::Matched | OrderStatusType::Canceled
            )
        {
            return;
        }

        match client.cancel_order(&order_id).await {
            Ok(_) => info!(order_id = %order_id, "canceled unfilled order at deadline"),
            Err(error) => {
                warn!(order_id = %order_id, error = %error, "failed to cancel unfilled order at deadline")
            }
        }
    });
}

fn cancel_deadline_ms(candidate: &Candidate) -> i64 {
    candidate.signal_time_ms + 1 + cancel_after_open_ms(candidate.interval)
}

fn cancel_after_open_ms(interval: crate::types::crypto::Interval) -> i64 {
    match interval {
        crate::types::crypto::Interval::M5 => constants::M5_CANCEL_AFTER_OPEN_MS,
        crate::types::crypto::Interval::M15 => constants::M15_CANCEL_AFTER_OPEN_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RuntimeConfig;
    use crate::strategy::crypto_reversal::model::Side;
    use crate::types::crypto::{Interval, Symbol};
    use std::path::PathBuf;
    use std::time::Duration;

    fn candidate(size_factor: f64) -> Candidate {
        Candidate {
            symbol: Symbol::Eth,
            interval: Interval::M5,
            market_slug: "eth-updown-5m-test".to_string(),
            side: Side::Up,
            signal_time_ms: 1,
            score: 0.3,
            size_factor,
        }
    }

    fn runtime(
        allow_order_usdc: f64,
        reduce_order_usdc: f64,
        crypto_reversal_order_price: Option<&str>,
    ) -> RuntimeConfig {
        RuntimeConfig {
            intervals: vec![Interval::M5],
            sqlite_path: PathBuf::from("data/runtime/events.sqlite3"),
            scan_interval: Duration::from_millis(1_000),
            allow_order_usdc,
            reduce_order_usdc,
            crypto_reversal_order_price: crypto_reversal_order_price
                .map(|value| Decimal::from_str(value).unwrap()),
        }
    }

    #[test]
    fn size_uses_allow_order_usdc() {
        let runtime = runtime(4.0, 3.0, None);
        let size = size(
            &candidate(1.25),
            &runtime,
            Decimal::from_str("0.50").unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(size, Decimal::from_str("8").unwrap());
    }

    #[test]
    fn size_uses_reduce_order_usdc() {
        let runtime = runtime(4.0, 3.0, None);
        let size = size(
            &candidate(0.75),
            &runtime,
            Decimal::from_str("0.50").unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(size, Decimal::from_str("6").unwrap());
    }

    #[test]
    fn size_applies_polymarket_minimum_shares() {
        let runtime = runtime(4.0, 3.0, None);
        let size = size(
            &candidate(1.0),
            &runtime,
            Decimal::from_str("1.20").unwrap(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(size, Decimal::from_str("5").unwrap());
    }

    #[test]
    fn cancel_deadline_matches_interval_policy() {
        let mut five = candidate(1.0);
        five.interval = Interval::M5;
        five.signal_time_ms = 299_999;
        assert_eq!(cancel_deadline_ms(&five), 330_000);

        let mut fifteen = candidate(1.0);
        fifteen.interval = Interval::M15;
        fifteen.signal_time_ms = 899_999;
        assert_eq!(cancel_deadline_ms(&fifteen), 1_020_000);
    }

    #[test]
    fn configured_order_price_uses_runtime_override() {
        let runtime = runtime(4.0, 3.0, Some("0.48"));
        assert_eq!(
            configured_order_price(&runtime),
            Some(Decimal::from_str("0.48").unwrap())
        );
    }

    #[test]
    fn within_entry_price_rejects_override_above_entry_limit() {
        assert_eq!(within_entry_price(Decimal::from_str("0.60").unwrap()), None);
    }
}
