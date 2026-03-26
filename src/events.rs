//! 仓库级事件模型。
//!
//! 这里定义的是业务事件，不是存储表结构。
//! 当前只覆盖：
//! - 订单事件 `Order`
//! - 成交事件 `Trade`
//! - 策略归因事件 `Strategy`

use chrono::Utc;
use polymarket_client_sdk::auth::ApiKey;
use polymarket_client_sdk::clob::{
    types::{Side as MarketSide, TraderSide},
    ws::{OrderMessage, TradeMessage},
};
use rust_decimal::Decimal;
use serde_json::json;

use crate::polymarket::types::positions::Fill;
use crate::strategy::crypto_reversal::model::Side;
use crate::strategy::crypto_reversal::service::Candidate;

#[derive(Debug, Clone, PartialEq)]
pub struct Order {
    pub order_id: String,
    pub side: String,
    pub price: Decimal,
    pub size: Decimal,
    pub status: String,
    pub created_at: i64,
}

impl Order {
    pub fn from_order_message(msg: &OrderMessage) -> Self {
        let created_at = Utc::now().timestamp_millis();
        let status = msg
            .status
            .as_ref()
            .map(|value| format!("{value:?}").to_ascii_lowercase())
            .unwrap_or_else(|| "unknown".to_string());
        let size = msg.original_size.unwrap_or(Decimal::ZERO);

        Self {
            order_id: msg.id.clone(),
            side: market_side_name(msg.side).to_string(),
            price: msg.price,
            size,
            status,
            created_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Trade {
    pub id: String,
    pub order_id: Option<String>,
    pub trade_id: String,
    pub asset_id: String,
    pub side: String,
    pub price: Decimal,
    pub size: Decimal,
    pub fee_bps: Option<Decimal>,
    pub event_time: Option<i64>,
    pub created_at: i64,
}

impl Trade {
    pub fn from_trade_message(msg: &TradeMessage, owner: ApiKey, fill: &Fill) -> Self {
        let order_id = match msg.trader_side.as_ref() {
            Some(TraderSide::Taker) => msg.taker_order_id.clone(),
            Some(TraderSide::Maker) => msg
                .maker_orders
                .iter()
                .find(|order| order.owner == owner)
                .map(|order| order.order_id.clone()),
            _ => None,
        };

        Self {
            id: fill.id.clone(),
            order_id,
            trade_id: msg.id.clone(),
            asset_id: fill.asset_id.to_string(),
            side: market_side_name(fill.side).to_string(),
            price: fill.price,
            size: fill.size,
            fee_bps: fill.fee_rate_bps,
            event_time: fill.timestamp,
            created_at: Utc::now().timestamp_millis(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Strategy {
    pub order_id: String,
    pub strategy: String,
    pub symbol: String,
    pub interval: String,
    pub market_slug: String,
    pub side: String,
    pub created_at: i64,
    pub event: String,
}

impl Strategy {
    pub fn from_candidate(strategy: &str, order_id: String, candidate: &Candidate) -> Self {
        let model = crate::strategy::crypto_reversal::constants::default_model_config();

        Self {
            order_id,
            strategy: strategy.to_string(),
            symbol: candidate.symbol.as_slug().to_string(),
            interval: candidate.interval.as_slug().to_string(),
            market_slug: candidate.market_slug.clone(),
            side: strategy_side_name(candidate.side).to_string(),
            created_at: Utc::now().timestamp_millis(),
            event: json!({
                "signal_time_ms": candidate.signal_time_ms,
                "score": candidate.score,
                "size_factor": candidate.size_factor,
                "conditions": {
                    "warmup_bars": model.warmup_bars,
                    "rsi_period": model.rsi_period,
                    "bb_period": model.bb_period,
                    "bb_stddev": model.bb_stddev,
                    "macd_fast": model.macd_fast,
                    "macd_slow": model.macd_slow,
                    "macd_signal": model.macd_signal,
                    "min_width_pct": model.min_width_pct,
                    "long_rsi_max": model.long_rsi_max,
                    "short_rsi_min": model.short_rsi_min,
                    "band_pad_pct": model.band_pad_pct,
                    "add_score": model.add_score,
                    "max_score": model.max_score,
                },
                "trigger": {
                    "symbol": candidate.symbol.as_slug(),
                    "interval": candidate.interval.as_slug(),
                    "market_slug": &candidate.market_slug,
                    "side": strategy_side_name(candidate.side),
                }
            })
            .to_string(),
        }
    }
}

fn market_side_name(side: MarketSide) -> &'static str {
    match side {
        MarketSide::Buy => "buy",
        MarketSide::Sell => "sell",
        MarketSide::Unknown => "unknown",
        _ => "unknown",
    }
}

fn strategy_side_name(side: Side) -> &'static str {
    match side {
        Side::Up => "up",
        Side::Down => "down",
    }
}
