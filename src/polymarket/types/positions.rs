//! Polymarket 多市场本地持仓模型。
//!
//! 这个版本刻意保持最小：
//! - 只维护本地持仓；
//! - 只按成交更新；
//! - 只按 `asset_id` 读取；
//! - 只处理真正会影响持仓的手续费。
//!
//! 一个关键约束：
//! `POST /order` 的返回只能说明订单被受理，不能精确表达成交和手续费，
//! 所以这里不拿它直接改持仓。
//! 本地持仓应当由成交回报驱动：
//! - `TradeResponse`
//! - `data::Trade`
//!
//! 官方手续费规则里最重要的两点：
//! - taker 买单手续费按公式先算成 USDC，再以 shares 扣除；
//! - taker 卖单手续费直接以 USDC 扣除。

use crate::errors::{PolyfillError, Result};
use polymarket_client_sdk::{
    clob::{
        types::response::TradeResponse,
        types::{Side, TradeStatusType, TraderSide},
    },
    data::types::{Side as DataSide, response::Trade as DataTrade},
    types::{B256, U256},
};
use rust_decimal::{Decimal, RoundingStrategy};
use std::collections::{HashMap, HashSet};

const FEE_PRECISION_DP: u32 = 4;

/// 单个 market 的手续费规则。
///
/// 官方公式：
/// `fee = C × p × feeRate × (p × (1 - p))^exponent`
///
/// 其中：
/// - `C` 是成交 shares
/// - `p` 是成交价格
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeeRule {
    pub fee_rate: Decimal,
    pub exponent: u32,
}

impl Default for FeeRule {
    fn default() -> Self {
        Self::free()
    }
}

impl FeeRule {
    pub const CRYPTO_EXPONENT: u32 = 2;
    pub const SPORTS_EXPONENT: u32 = 1;

    pub fn free() -> Self {
        Self {
            fee_rate: Decimal::ZERO,
            exponent: 0,
        }
    }

    pub fn crypto() -> Self {
        Self {
            fee_rate: Decimal::new(25, 2),
            exponent: Self::CRYPTO_EXPONENT,
        }
    }

    pub fn sports() -> Self {
        Self {
            fee_rate: Decimal::new(175, 4),
            exponent: Self::SPORTS_EXPONENT,
        }
    }

    pub fn new(fee_rate: Decimal, exponent: u32) -> Result<Self> {
        if fee_rate.is_sign_negative() {
            return Err(PolyfillError::validation("fee_rate 不能为负数"));
        }

        Ok(Self { fee_rate, exponent })
    }

    pub fn from_bps(fee_rate_bps: Decimal, exponent: u32) -> Result<Self> {
        if fee_rate_bps.is_sign_negative() {
            return Err(PolyfillError::validation("fee_rate_bps 不能为负数"));
        }

        Self::new(fee_rate_bps / Decimal::new(100, 0), exponent)
    }

    pub fn crypto_from_bps(fee_rate_bps: Decimal) -> Result<Self> {
        Self::from_bps(fee_rate_bps, Self::CRYPTO_EXPONENT)
    }

    pub fn taker_fee_usdc(&self, size: Decimal, price: Decimal) -> Result<Decimal> {
        validate_size(size)?;
        validate_price(price)?;

        if !self.is_enabled() || size.is_zero() || price.is_zero() {
            return Ok(Decimal::ZERO);
        }

        let fee = size
            * price
            * self.fee_rate
            * decimal_pow(price * (Decimal::ONE - price), self.exponent);

        Ok(fee.round_dp_with_strategy(FEE_PRECISION_DP, RoundingStrategy::MidpointAwayFromZero))
    }

    pub fn buy_fee_shares(&self, size: Decimal, price: Decimal) -> Result<Decimal> {
        let fee_usdc = self.taker_fee_usdc(size, price)?;

        if fee_usdc.is_zero() || price.is_zero() {
            return Ok(Decimal::ZERO);
        }

        Ok(fee_usdc / price)
    }

    pub fn is_enabled(&self) -> bool {
        !self.fee_rate.is_zero()
    }
}

/// 归一化后的成交。
///
/// 这里只保留更新持仓真正需要的字段。
#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    pub id: String,
    pub market_id: B256,
    pub asset_id: U256,
    pub side: Side,
    pub size: Decimal,
    pub price: Decimal,
    pub fee_rate_bps: Option<Decimal>,
    pub is_taker: bool,
    pub timestamp: Option<i64>,
    pub outcome: Option<String>,
}

impl Fill {
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(PolyfillError::validation("fill.id 不能为空"));
        }

        validate_side(self.side)?;
        validate_size(self.size)?;
        validate_price(self.price)?;

        Ok(())
    }

    pub fn from_trade_response(trade: &TradeResponse) -> Result<Self> {
        validate_trade_status(&trade.status)?;

        Ok(Self {
            id: format!("trade:{}", trade.id),
            market_id: trade.market,
            asset_id: trade.asset_id,
            side: validate_side(trade.side)?,
            size: trade.size,
            price: trade.price,
            fee_rate_bps: Some(trade.fee_rate_bps),
            is_taker: trader_side_is_taker(trade.trader_side.clone())?,
            timestamp: Some(trade.match_time.timestamp()),
            outcome: Some(trade.outcome.clone()),
        })
    }

    /// `data::Trade` 里没有 `trader_side`，需要外部显式传入。
    pub fn from_data_trade(trade: &DataTrade, is_taker: bool) -> Result<Self> {
        Ok(Self {
            id: synthetic_data_trade_id(trade),
            market_id: trade.condition_id,
            asset_id: trade.asset,
            side: side_from_data(trade.side.clone())?,
            size: trade.size,
            price: trade.price,
            fee_rate_bps: None,
            is_taker,
            timestamp: Some(trade.timestamp),
            outcome: Some(trade.outcome.clone()),
        })
    }
}

/// 单个 asset 的本地持仓。
///
/// 口径非常简单：
/// - `size`：当前净持仓 shares
/// - `avg_price`：当前剩余持仓的平均成本价
/// - `realized_pnl`：已实现盈亏
///
/// 买单 share fee 的处理方式：
/// - 现金成本仍按 `size * price`
/// - 实际入库份额变成 `size - fee_shares`
/// - 所以 `avg_price` 会自然抬高
#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    pub market_id: B256,
    pub asset_id: U256,
    pub outcome: Option<String>,
    pub size: Decimal,
    pub avg_price: Decimal,
    pub realized_pnl: Decimal,
    pub buy_fee_usdc: Decimal,
    pub buy_fee_shares: Decimal,
    pub sell_fee_usdc: Decimal,
    pub last_trade_ts: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TradeFees {
    fee_usdc: Decimal,
    fee_shares: Decimal,
}

impl TradeFees {
    fn from_fill(fill: &Fill, fee_rule: FeeRule) -> Result<Self> {
        if !fill.is_taker {
            return Ok(Self {
                fee_usdc: Decimal::ZERO,
                fee_shares: Decimal::ZERO,
            });
        }

        Ok(Self {
            fee_usdc: fee_rule.taker_fee_usdc(fill.size, fill.price)?,
            fee_shares: fee_rule.buy_fee_shares(fill.size, fill.price)?,
        })
    }
}

impl Position {
    fn new(market_id: B256, asset_id: U256, outcome: Option<String>) -> Self {
        Self {
            market_id,
            asset_id,
            outcome,
            size: Decimal::ZERO,
            avg_price: Decimal::ZERO,
            realized_pnl: Decimal::ZERO,
            buy_fee_usdc: Decimal::ZERO,
            buy_fee_shares: Decimal::ZERO,
            sell_fee_usdc: Decimal::ZERO,
            last_trade_ts: None,
        }
    }

    pub fn open_cost(&self) -> Decimal {
        self.size * self.avg_price
    }

    fn apply_fill(&mut self, fill: &Fill, fee_rule: FeeRule) -> Result<()> {
        fill.validate()?;

        if self.market_id != fill.market_id {
            return Err(PolyfillError::validation(format!(
                "asset_id {} 已绑定到其他 market {}",
                fill.asset_id, self.market_id
            )));
        }

        maybe_update_outcome(&mut self.outcome, fill.outcome.as_deref());

        match fill.side {
            Side::Buy => self.apply_buy(fill, fee_rule)?,
            Side::Sell => self.apply_sell(fill, fee_rule)?,
            Side::Unknown => unreachable!("unknown side already validated"),
            _ => unreachable!("future sdk side variant already rejected"),
        }

        self.last_trade_ts = fill.timestamp;
        Ok(())
    }

    fn apply_buy(&mut self, fill: &Fill, fee_rule: FeeRule) -> Result<()> {
        let fees = TradeFees::from_fill(fill, fee_rule)?;
        let net_size = fill.size - fees.fee_shares;

        if net_size.is_sign_negative() {
            return Err(PolyfillError::validation("买单手续费份额超过了成交份额"));
        }

        let new_cost = self.open_cost() + fill.size * fill.price;
        self.size += net_size;
        self.avg_price = if self.size.is_zero() {
            Decimal::ZERO
        } else {
            new_cost / self.size
        };
        self.buy_fee_usdc += fees.fee_usdc;
        self.buy_fee_shares += fees.fee_shares;

        Ok(())
    }

    fn apply_sell(&mut self, fill: &Fill, fee_rule: FeeRule) -> Result<()> {
        if fill.size > self.size {
            return Err(PolyfillError::validation(format!(
                "卖出份额 {} 超过当前持仓 {}",
                fill.size, self.size
            )));
        }

        let fees = TradeFees::from_fill(fill, fee_rule)?;
        let cost_released = self.avg_price * fill.size;
        let net_proceeds = fill.size * fill.price - fees.fee_usdc;

        self.realized_pnl += net_proceeds - cost_released;
        self.size -= fill.size;
        self.sell_fee_usdc += fees.fee_usdc;

        if self.size.is_zero() {
            self.avg_price = Decimal::ZERO;
        }

        Ok(())
    }
}

/// 多 market 持仓集合。
///
/// 结构保持最小：
/// - `positions`: 直接按 `asset_id` 存持仓
/// - `market_fees`: 每个 market 的手续费规则
/// - `seen_fills`: 成交去重
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Positions {
    positions: HashMap<U256, Position>,
    market_fees: HashMap<B256, FeeRule>,
    seen_fills: HashSet<String>,
}

impl Positions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_market_fee(&mut self, market_id: B256, fee_rule: FeeRule) {
        self.market_fees.insert(market_id, fee_rule);
    }

    pub fn ensure_market_fee(&mut self, market_id: B256, fee_rule: FeeRule) {
        self.market_fees.entry(market_id).or_insert(fee_rule);
    }

    pub fn get(&self, asset_id: &U256) -> Option<&Position> {
        self.positions.get(asset_id)
    }

    pub fn all(&self) -> impl Iterator<Item = &Position> {
        self.positions.values()
    }

    pub fn for_market(&self, market_id: &B256) -> Vec<&Position> {
        self.positions
            .values()
            .filter(|position| &position.market_id == market_id)
            .collect()
    }

    pub fn apply_fill(&mut self, fill: Fill) -> Result<()> {
        fill.validate()?;

        if self.seen_fills.contains(&fill.id) {
            return Ok(());
        }

        let fee_rule = match fill.fee_rate_bps {
            Some(fee_rate_bps) => FeeRule::crypto_from_bps(fee_rate_bps)?,
            None => self
                .market_fees
                .get(&fill.market_id)
                .copied()
                .ok_or_else(|| {
                    PolyfillError::validation(format!(
                        "market {} 尚未注册手续费规则，且 fill 未提供 fee_rate_bps",
                        fill.market_id
                    ))
                })?,
        };

        let position = self
            .positions
            .entry(fill.asset_id)
            .or_insert_with(|| Position::new(fill.market_id, fill.asset_id, fill.outcome.clone()));

        position.apply_fill(&fill, fee_rule)?;
        self.seen_fills.insert(fill.id);
        Ok(())
    }

    pub fn bootstrap_position(
        &mut self,
        market_id: B256,
        asset_id: U256,
        outcome: Option<String>,
        size: Decimal,
        avg_price: Decimal,
        realized_pnl: Decimal,
        fee_rule: FeeRule,
    ) -> Result<()> {
        if size <= Decimal::ZERO {
            return Err(PolyfillError::validation(
                "bootstrap position.size 必须大于 0",
            ));
        }

        validate_price(avg_price)?;
        self.register_market_fee(market_id, fee_rule);
        self.positions.insert(
            asset_id,
            Position {
                market_id,
                asset_id,
                outcome,
                size,
                avg_price,
                realized_pnl,
                buy_fee_usdc: Decimal::ZERO,
                buy_fee_shares: Decimal::ZERO,
                sell_fee_usdc: Decimal::ZERO,
                last_trade_ts: None,
            },
        );
        Ok(())
    }

    pub fn apply_trade_response(&mut self, trade: &TradeResponse) -> Result<()> {
        self.apply_fill(Fill::from_trade_response(trade)?)
    }

    pub fn apply_data_trade(&mut self, trade: &DataTrade, is_taker: bool) -> Result<()> {
        self.apply_fill(Fill::from_data_trade(trade, is_taker)?)
    }
}

fn synthetic_data_trade_id(trade: &DataTrade) -> String {
    format!(
        "data-trade:{}:{}:{}:{}:{}:{}",
        trade.transaction_hash, trade.timestamp, trade.asset, trade.side, trade.price, trade.size
    )
}

fn maybe_update_outcome(slot: &mut Option<String>, outcome: Option<&str>) {
    if slot.is_none() {
        *slot = normalize_outcome(outcome);
    }
}

fn normalize_outcome(outcome: Option<&str>) -> Option<String> {
    let outcome = outcome?.trim();
    if outcome.is_empty() {
        None
    } else {
        Some(outcome.to_string())
    }
}

fn trader_side_is_taker(side: TraderSide) -> Result<bool> {
    match side {
        TraderSide::Taker => Ok(true),
        TraderSide::Maker => Ok(false),
        TraderSide::Unknown(raw) => Err(PolyfillError::validation(format!(
            "不支持使用 trader_side={raw} 更新持仓"
        ))),
        _ => Err(PolyfillError::validation(
            "不支持使用未知 TraderSide 更新持仓",
        )),
    }
}

fn side_from_data(side: DataSide) -> Result<Side> {
    match side {
        DataSide::Buy => Ok(Side::Buy),
        DataSide::Sell => Ok(Side::Sell),
        DataSide::Unknown(raw) => Err(PolyfillError::validation(format!(
            "不支持使用 data::Side::Unknown({raw}) 更新持仓"
        ))),
        _ => Err(PolyfillError::validation(
            "不支持使用未知 data::Side 更新持仓",
        )),
    }
}

fn validate_trade_status(status: &TradeStatusType) -> Result<()> {
    match status {
        TradeStatusType::Matched | TradeStatusType::Mined | TradeStatusType::Confirmed => Ok(()),
        TradeStatusType::Retrying => Err(PolyfillError::validation(
            "trade 仍在 retrying，不能用于更新本地持仓",
        )),
        TradeStatusType::Failed => Err(PolyfillError::validation(
            "trade 已失败，不能用于更新本地持仓",
        )),
        TradeStatusType::Unknown(raw) => Err(PolyfillError::validation(format!(
            "未知 trade status: {raw}"
        ))),
        _ => Err(PolyfillError::validation(
            "不支持使用未知 TradeStatusType 更新持仓",
        )),
    }
}

fn validate_side(side: Side) -> Result<Side> {
    match side {
        Side::Buy | Side::Sell => Ok(side),
        Side::Unknown => Err(PolyfillError::validation("不支持使用 UNKNOWN 方向更新持仓")),
        _ => Err(PolyfillError::validation(
            "不支持使用未知 SDK Side 方向更新持仓",
        )),
    }
}

fn validate_size(size: Decimal) -> Result<()> {
    if size <= Decimal::ZERO {
        return Err(PolyfillError::validation("size 必须大于 0"));
    }

    Ok(())
}

fn validate_price(price: Decimal) -> Result<()> {
    if price.is_sign_negative() {
        return Err(PolyfillError::validation("价格不能为负数"));
    }

    if price > Decimal::ONE {
        return Err(PolyfillError::validation("价格不能大于 1"));
    }

    Ok(())
}

fn decimal_pow(base: Decimal, exponent: u32) -> Decimal {
    let mut result = Decimal::ONE;

    for _ in 0..exponent {
        result *= base;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn market(id: u8) -> B256 {
        B256::from([id; 32])
    }

    fn asset(id: u64) -> U256 {
        U256::from(id)
    }

    fn fill(
        id: &str,
        market_id: B256,
        asset_id: U256,
        side: Side,
        size: Decimal,
        price: Decimal,
        is_taker: bool,
    ) -> Fill {
        Fill {
            id: id.to_string(),
            market_id,
            asset_id,
            side,
            size,
            price,
            fee_rate_bps: None,
            is_taker,
            timestamp: Some(1),
            outcome: Some("Yes".to_string()),
        }
    }

    #[test]
    fn test_taker_buy_fee_is_collected_in_shares() {
        let rule = FeeRule::crypto_from_bps(Decimal::new(25, 0)).expect("bps should be valid");
        let fee_usdc = rule
            .taker_fee_usdc(Decimal::new(100, 0), Decimal::new(5, 1))
            .expect("fee should be calculated");
        let fee_shares = rule
            .buy_fee_shares(Decimal::new(100, 0), Decimal::new(5, 1))
            .expect("share fee should be calculated");

        assert_eq!(fee_usdc, Decimal::new(7813, 4));
        assert_eq!(fee_shares, Decimal::new(15626, 4));
    }

    #[test]
    fn test_apply_taker_buy_updates_size_and_avg_price() {
        let market_id = market(1);
        let asset_id = asset(1);
        let mut positions = Positions::new();
        positions.register_market_fee(market_id, FeeRule::crypto());

        positions
            .apply_fill(fill(
                "trade-1",
                market_id,
                asset_id,
                Side::Buy,
                Decimal::new(100, 0),
                Decimal::new(5, 1),
                true,
            ))
            .expect("buy fill should work");

        let position = positions.get(&asset_id).expect("position should exist");
        assert_eq!(position.size, Decimal::new(984374, 4));
        assert_eq!(
            position.avg_price.round_dp(12),
            Decimal::new(507937023936, 12)
        );
        assert_eq!(position.buy_fee_usdc, Decimal::new(7813, 4));
        assert_eq!(position.buy_fee_shares, Decimal::new(15626, 4));
    }

    #[test]
    fn test_apply_maker_buy_has_no_fee() {
        let market_id = market(2);
        let asset_id = asset(2);
        let mut positions = Positions::new();
        positions.register_market_fee(market_id, FeeRule::crypto());

        positions
            .apply_fill(fill(
                "trade-1",
                market_id,
                asset_id,
                Side::Buy,
                Decimal::new(10, 0),
                Decimal::new(4, 1),
                false,
            ))
            .expect("maker buy should work");

        let position = positions.get(&asset_id).expect("position should exist");
        assert_eq!(position.size, Decimal::new(10, 0));
        assert_eq!(position.avg_price, Decimal::new(4, 1));
        assert_eq!(position.buy_fee_usdc, Decimal::ZERO);
        assert_eq!(position.buy_fee_shares, Decimal::ZERO);
    }

    #[test]
    fn test_apply_taker_sell_updates_realized_pnl() {
        let market_id = market(3);
        let asset_id = asset(3);
        let mut positions = Positions::new();
        positions.register_market_fee(market_id, FeeRule::crypto());

        positions
            .apply_fill(fill(
                "trade-1",
                market_id,
                asset_id,
                Side::Buy,
                Decimal::new(100, 0),
                Decimal::new(4, 1),
                false,
            ))
            .expect("maker buy should work");

        positions
            .apply_fill(fill(
                "trade-2",
                market_id,
                asset_id,
                Side::Sell,
                Decimal::new(40, 0),
                Decimal::new(6, 1),
                true,
            ))
            .expect("taker sell should work");

        let position = positions.get(&asset_id).expect("position should exist");
        assert_eq!(position.size, Decimal::new(60, 0));
        assert_eq!(position.avg_price, Decimal::new(4, 1));
        assert_eq!(position.sell_fee_usdc, Decimal::new(3456, 4));
        assert_eq!(position.realized_pnl, Decimal::new(76544, 4));
    }

    #[test]
    fn test_apply_fill_is_idempotent() {
        let market_id = market(4);
        let asset_id = asset(4);
        let mut positions = Positions::new();
        positions.register_market_fee(market_id, FeeRule::free());

        let fill = fill(
            "trade-1",
            market_id,
            asset_id,
            Side::Buy,
            Decimal::new(10, 0),
            Decimal::new(55, 2),
            false,
        );

        positions
            .apply_fill(fill.clone())
            .expect("first fill should work");
        positions
            .apply_fill(fill)
            .expect("duplicate should be ignored");

        let position = positions.get(&asset_id).expect("position should exist");
        assert_eq!(position.size, Decimal::new(10, 0));
    }

    #[test]
    fn test_apply_fill_uses_fee_rate_bps_from_fill_without_market_registration() {
        let market_id = market(7);
        let asset_id = asset(7);
        let mut positions = Positions::new();
        let mut taker_fill = fill(
            "trade-1",
            market_id,
            asset_id,
            Side::Buy,
            Decimal::new(100, 0),
            Decimal::new(5, 1),
            true,
        );
        taker_fill.fee_rate_bps = Some(Decimal::new(25, 0));

        positions
            .apply_fill(taker_fill)
            .expect("fill with fee_rate_bps should work");

        let position = positions.get(&asset_id).expect("position should exist");
        assert_eq!(position.buy_fee_usdc, Decimal::new(7813, 4));
        assert_eq!(position.buy_fee_shares, Decimal::new(15626, 4));
    }

    #[test]
    fn test_unregistered_market_is_rejected() {
        let mut positions = Positions::new();

        let err = positions
            .apply_fill(fill(
                "trade-1",
                market(5),
                asset(5),
                Side::Buy,
                Decimal::new(10, 0),
                Decimal::new(5, 1),
                true,
            ))
            .expect_err("missing fee rule should fail");

        assert!(
            err.to_string()
                .contains("尚未注册手续费规则，且 fill 未提供 fee_rate_bps")
        );
    }

    #[test]
    fn test_bootstrap_position_allows_followup_sell() {
        let market_id = market(6);
        let asset_id = asset(6);
        let mut positions = Positions::new();

        positions
            .bootstrap_position(
                market_id,
                asset_id,
                Some("Yes".to_string()),
                Decimal::new(10, 0),
                Decimal::new(4, 1),
                Decimal::new(12, 1),
                FeeRule::free(),
            )
            .expect("bootstrap should work");

        positions
            .apply_fill(fill(
                "trade-2",
                market_id,
                asset_id,
                Side::Sell,
                Decimal::new(4, 0),
                Decimal::new(6, 1),
                false,
            ))
            .expect("sell after bootstrap should work");

        let position = positions.get(&asset_id).expect("position should exist");
        assert_eq!(position.size, Decimal::new(6, 0));
        assert_eq!(position.avg_price, Decimal::new(4, 1));
        assert_eq!(position.realized_pnl, Decimal::new(20, 1));
    }
}
