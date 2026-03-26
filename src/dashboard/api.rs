use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use crate::events;
use crate::polymarket::types::open_orders::Order as OpenOrder;
use crate::polymarket::types::positions::Position;
use crate::polymarket::user_task::ClosedPositionsCache;
use crate::storage::sqlite::DashboardHistory;
use crate::strategy::crypto_reversal::model::Side;
use crate::strategy::crypto_reversal::service::Candidate;
use crate::types::crypto::Symbol;
use chrono::{NaiveDate, Utc};
use polymarket_client_sdk::data::types::response::ClosedPosition;
use polymarket_client_sdk::types::U256;
use rust_decimal::Decimal;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotPayload {
    pub account: AccountSnapshot,
    pub strategies: Vec<StrategySnapshot>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountSnapshot {
    pub runtime_status: String,
    pub binance_ws_status: String,
    pub polymarket_ws_status: String,
    pub server_time_ms: i64,
    pub open_order_count: usize,
    pub position_count: usize,
    pub today_order_count: usize,
    pub today_trade_count: usize,
    pub today_notional_usdc: String,
    pub settled_count: usize,
    pub settled_pnl_usdc: String,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StrategySnapshot {
    pub strategy: String,
    pub status: String,
    pub last_scan_ms: Option<i64>,
    pub last_signal_ms: Option<i64>,
    pub last_order_ms: Option<i64>,
    pub last_trade_ms: Option<i64>,
    pub open_order_count: usize,
    pub position_count: usize,
    pub today_order_count: usize,
    pub today_trade_count: usize,
    pub today_notional_usdc: String,
    pub settled_count: usize,
    pub settled_pnl_usdc: String,
    pub last_error: Option<String>,
    pub latest_signal: Option<SignalPayload>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SignalPayload {
    pub symbol: String,
    pub interval: String,
    pub market_slug: String,
    pub side: String,
    pub signal_time_ms: i64,
    pub score: f64,
    pub size_factor: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrderPayload {
    pub order_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market_slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_side: Option<String>,
    pub status: String,
    pub price: String,
    pub size: String,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PositionPayload {
    pub asset_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market_slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    pub size: String,
    pub avg_price: String,
    pub open_cost: String,
    pub realized_pnl: String,
    pub buy_fee_usdc: String,
    pub buy_fee_shares: String,
    pub sell_fee_usdc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_trade_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettlementPayload {
    #[serde(flatten)]
    pub raw: RawClosedPositionPayload,
    #[serde(skip_serializing)]
    pub asset_id: String,
    #[serde(skip_serializing)]
    pub market_slug: String,
    #[serde(skip_serializing)]
    pub avg_price: String,
    #[serde(skip_serializing)]
    pub total_bought: String,
    #[serde(skip_serializing)]
    pub realized_pnl: String,
    #[serde(skip_serializing)]
    pub closed_at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RawClosedPositionPayload {
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: String,
    pub asset: String,
    #[serde(rename = "conditionId")]
    pub condition_id: String,
    #[serde(rename = "avgPrice")]
    pub avg_price: String,
    #[serde(rename = "totalBought")]
    pub total_bought: String,
    #[serde(rename = "realizedPnl")]
    pub realized_pnl: String,
    #[serde(rename = "curPrice")]
    pub cur_price: String,
    pub title: String,
    pub slug: String,
    pub icon: String,
    #[serde(rename = "eventSlug")]
    pub event_slug: String,
    pub outcome: String,
    #[serde(rename = "outcomeIndex")]
    pub outcome_index: i32,
    #[serde(rename = "oppositeOutcome")]
    pub opposite_outcome: String,
    #[serde(rename = "oppositeAsset")]
    pub opposite_asset: String,
    #[serde(rename = "endDate")]
    pub end_date: String,
    pub timestamp: i64,
}

impl RawClosedPositionPayload {
    fn from_closed(closed: &ClosedPosition) -> Self {
        Self {
            proxy_wallet: closed.proxy_wallet.to_string(),
            asset: closed.asset.to_string(),
            condition_id: closed.condition_id.to_string(),
            avg_price: closed.avg_price.normalize().to_string(),
            total_bought: closed.total_bought.normalize().to_string(),
            realized_pnl: closed.realized_pnl.normalize().to_string(),
            cur_price: closed.cur_price.normalize().to_string(),
            title: closed.title.clone(),
            slug: closed.slug.clone(),
            icon: closed.icon.clone(),
            event_slug: closed.event_slug.clone(),
            outcome: closed.outcome.clone(),
            outcome_index: closed.outcome_index,
            opposite_outcome: closed.opposite_outcome.clone(),
            opposite_asset: closed.opposite_asset.to_string(),
            end_date: closed.end_date.to_rfc3339(),
            timestamp: closed.timestamp,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ClosedPositionsPagePayload {
    pub strategy: String,
    pub range: String,
    pub page: usize,
    pub page_size: usize,
    pub total: usize,
    pub total_pages: usize,
    pub rows: Vec<SettlementPayload>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PositionsPayload {
    pub strategy: String,
    pub total: usize,
    pub rows: Vec<PositionPayload>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenOrdersPayload {
    pub strategy: String,
    pub total: usize,
    pub rows: Vec<OrderPayload>,
}

#[derive(Debug, Clone)]
struct OrderMeta {
    strategy: String,
    market_slug: String,
    side: String,
    created_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
struct StrategyState {
    snapshot: StrategySnapshot,
    open_orders: Vec<OrderPayload>,
    positions: Vec<PositionPayload>,
}

impl StrategyState {
    fn new(strategy: &str) -> Self {
        Self {
            snapshot: StrategySnapshot {
                strategy: strategy.to_string(),
                status: "starting".to_string(),
                last_scan_ms: None,
                last_signal_ms: None,
                last_order_ms: None,
                last_trade_ms: None,
                open_order_count: 0,
                position_count: 0,
                today_order_count: 0,
                today_trade_count: 0,
                today_notional_usdc: "0".to_string(),
                settled_count: 0,
                settled_pnl_usdc: "0".to_string(),
                last_error: None,
                latest_signal: None,
            },
            open_orders: Vec::new(),
            positions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct InnerState {
    day: NaiveDate,
    account: AccountSnapshot,
    strategies: BTreeMap<String, StrategyState>,
    order_meta: HashMap<String, OrderMeta>,
    asset_meta: HashMap<String, OrderMeta>,
    market_strategy: HashMap<String, String>,
}

impl InnerState {
    fn new() -> Self {
        Self {
            day: Utc::now().date_naive(),
            account: AccountSnapshot {
                runtime_status: "starting".to_string(),
                binance_ws_status: "connecting".to_string(),
                polymarket_ws_status: "connecting".to_string(),
                server_time_ms: Utc::now().timestamp_millis(),
                open_order_count: 0,
                position_count: 0,
                today_order_count: 0,
                today_trade_count: 0,
                today_notional_usdc: "0".to_string(),
                settled_count: 0,
                settled_pnl_usdc: "0".to_string(),
                last_error: None,
            },
            strategies: BTreeMap::new(),
            order_meta: HashMap::new(),
            asset_meta: HashMap::new(),
            market_strategy: HashMap::new(),
        }
    }

    fn ensure_strategy(&mut self, strategy: &str) -> &mut StrategyState {
        self.strategies
            .entry(strategy.to_string())
            .or_insert_with(|| StrategyState::new(strategy))
    }

    fn maybe_reset_day(&mut self) {
        let today = Utc::now().date_naive();
        if self.day == today {
            return;
        }

        self.day = today;
        self.account.today_order_count = 0;
        self.account.today_trade_count = 0;
        self.account.today_notional_usdc = "0".to_string();

        for strategy in self.strategies.values_mut() {
            strategy.snapshot.today_order_count = 0;
            strategy.snapshot.today_trade_count = 0;
            strategy.snapshot.today_notional_usdc = "0".to_string();
        }
    }

    fn register_order_meta(&mut self, order_id: &str, meta: OrderMeta) {
        self.market_strategy
            .insert(meta.market_slug.clone(), meta.strategy.clone());
        self.order_meta.insert(order_id.to_string(), meta);
    }
}

#[derive(Clone)]
pub struct Handle {
    state: Arc<RwLock<InnerState>>,
    closed_positions_cache: Arc<RwLock<Option<ClosedPositionsCache>>>,
}

impl Handle {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(InnerState::new())),
            closed_positions_cache: Arc::new(RwLock::new(None)),
        }
    }

    pub fn attach_closed_positions_cache(&self, cache: ClosedPositionsCache) {
        if let Ok(mut guard) = self.closed_positions_cache.write() {
            *guard = Some(cache);
        }
    }

    pub fn snapshot(&self) -> SnapshotPayload {
        self.snapshot_payload()
    }

    pub fn register_strategy(&self, strategy: &str) {
        if let Ok(mut state) = self.state.write() {
            if !state.strategies.contains_key(strategy) {
                state.ensure_strategy(strategy);
            }
        }
    }

    pub fn runtime_status(&self, status: &str) {
        if let Ok(mut state) = self.state.write() {
            state.account.runtime_status = status.to_string();
        }
    }

    pub fn binance_status(&self, status: &str) {
        if let Ok(mut state) = self.state.write() {
            state.account.binance_ws_status = status.to_string();
        }
    }

    pub fn polymarket_status(&self, status: &str) {
        if let Ok(mut state) = self.state.write() {
            state.account.polymarket_ws_status = status.to_string();
        }
    }

    pub fn scan(&self, strategy: &str) {
        if let Ok(mut state) = self.state.write() {
            state.ensure_strategy(strategy).snapshot.last_scan_ms = Some(now_ms());
        }
    }

    pub fn strategy_status(&self, strategy: &str, status: &str) {
        if let Ok(mut state) = self.state.write() {
            let snapshot = &mut state.ensure_strategy(strategy).snapshot;
            snapshot.status = status.to_string();
        }
    }

    pub fn user_state(&self, open_orders: &[OpenOrder], positions: &[Position]) {
        let Ok(mut state) = self.state.write() else {
            return;
        };

        state.maybe_reset_day();
        state.account.open_order_count = open_orders.len();
        state.account.position_count = positions.len();

        let mut strategy_open_orders = HashMap::<String, usize>::new();
        let mut strategy_open_order_rows = HashMap::<String, Vec<OrderPayload>>::new();
        let mut strategy_positions = HashMap::<String, Vec<PositionPayload>>::new();

        for order in open_orders {
            if let Some(meta) = state.order_meta.get(&order.id) {
                *strategy_open_orders
                    .entry(meta.strategy.clone())
                    .or_default() += 1;
                strategy_open_order_rows
                    .entry(meta.strategy.clone())
                    .or_default()
                    .push(open_order_payload(order, meta));
            }
        }

        for position in positions {
            if position.size <= Decimal::ZERO {
                continue;
            }

            let asset_id = position.asset_id.to_string();
            if let Some(meta) = state.asset_meta.get(&asset_id).cloned() {
                strategy_positions
                    .entry(meta.strategy)
                    .or_default()
                    .push(position_payload(position, Some(&meta.market_slug)));
            }
        }

        for strategy in state.strategies.values_mut() {
            strategy.snapshot.open_order_count = strategy_open_orders
                .get(&strategy.snapshot.strategy)
                .copied()
                .unwrap_or_default();
            let mut open_orders = strategy_open_order_rows
                .remove(&strategy.snapshot.strategy)
                .unwrap_or_default();
            open_orders.sort_by_key(|order| std::cmp::Reverse(order.created_at_ms));
            strategy.open_orders = open_orders;
            let mut positions = strategy_positions
                .remove(&strategy.snapshot.strategy)
                .unwrap_or_default();
            let position_count = positions.len();
            positions
                .sort_by_key(|position| std::cmp::Reverse(position.last_trade_ms.unwrap_or(0)));
            strategy.snapshot.position_count = position_count;
            strategy.positions = positions;
        }
    }

    pub fn signal(&self, strategy: &str, candidate: &Candidate) {
        let signal = signal_payload(candidate);
        let Ok(mut state) = self.state.write() else {
            return;
        };

        state.maybe_reset_day();
        let snapshot = &mut state.ensure_strategy(strategy).snapshot;
        snapshot.last_signal_ms = Some(now_ms());
        snapshot.latest_signal = Some(signal);
    }

    pub fn order_submission(
        &self,
        strategy: &str,
        candidate: &Candidate,
        asset_id: U256,
        order_id: &str,
        price: Decimal,
        size: Decimal,
    ) {
        let order = OrderPayload {
            order_id: order_id.to_string(),
            market_slug: Some(candidate.market_slug.clone()),
            side: Some(strategy_side_name(candidate.side).to_string()),
            order_side: Some("buy".to_string()),
            status: "submitted".to_string(),
            price: price.to_string(),
            size: size.to_string(),
            created_at_ms: now_ms(),
        };

        let Ok(mut state) = self.state.write() else {
            return;
        };

        state.maybe_reset_day();
        let meta = OrderMeta {
            strategy: strategy.to_string(),
            market_slug: candidate.market_slug.clone(),
            side: strategy_side_name(candidate.side).to_string(),
            created_at_ms: Some(now_ms()),
        };
        state.register_order_meta(order_id, meta.clone());
        state.asset_meta.insert(asset_id.to_string(), meta);
        state.account.today_order_count += 1;

        let strategy_state = state.ensure_strategy(strategy);
        strategy_state.snapshot.last_order_ms = Some(order.created_at_ms);
        strategy_state.snapshot.today_order_count += 1;
    }

    pub fn order(&self, order: &events::Order) {
        let Ok(mut state) = self.state.write() else {
            return;
        };

        state.maybe_reset_day();
        let Some(meta) = state.order_meta.get(&order.order_id).cloned() else {
            return;
        };

        let strategy = state.ensure_strategy(&meta.strategy);
        strategy.snapshot.last_order_ms = Some(order.created_at);
    }

    pub fn trade(&self, trade: &events::Trade) {
        let Ok(mut state) = self.state.write() else {
            return;
        };

        state.maybe_reset_day();
        let strategy_meta = trade
            .order_id
            .as_ref()
            .and_then(|order_id| state.order_meta.get(order_id))
            .cloned();

        state.account.today_trade_count += 1;
        let notional = trade.price * trade.size;
        let next_account_notional =
            add_decimal_string(&state.account.today_notional_usdc, notional);
        state.account.today_notional_usdc = next_account_notional;

        if let Some(meta) = strategy_meta {
            state
                .asset_meta
                .insert(trade.asset_id.clone(), meta.clone());
            state
                .market_strategy
                .entry(meta.market_slug.clone())
                .or_insert_with(|| meta.strategy.clone());
            let strategy = state.ensure_strategy(&meta.strategy);
            strategy.snapshot.last_trade_ms = Some(trade.event_time.unwrap_or(trade.created_at));
            strategy.snapshot.today_trade_count += 1;
            strategy.snapshot.today_notional_usdc =
                add_decimal_string(&strategy.snapshot.today_notional_usdc, notional);
        }
    }

    pub fn error(&self, strategy: Option<&str>, message: impl Into<String>) {
        let message = message.into();
        let Ok(mut state) = self.state.write() else {
            return;
        };

        state.account.last_error = Some(message.clone());
        state.account.runtime_status = "degraded".to_string();
        if let Some(name) = strategy {
            let strategy = state.ensure_strategy(name);
            strategy.snapshot.last_error = Some(message);
            strategy.snapshot.status = "degraded".to_string();
        }
    }

    pub fn load_history(&self, history: &DashboardHistory) {
        let Ok(mut state) = self.state.write() else {
            return;
        };

        state.maybe_reset_day();

        for strategy in history.strategies.iter().rev() {
            state.register_order_meta(
                &strategy.order_id,
                OrderMeta {
                    strategy: strategy.strategy.clone(),
                    market_slug: strategy.market_slug.clone(),
                    side: strategy.side.clone(),
                    created_at_ms: Some(strategy.created_at),
                },
            );

            let signal_time_ms = extract_signal_time_ms(strategy).unwrap_or(strategy.created_at);
            let latest_signal = parse_strategy_signal(strategy);
            let today = is_today(strategy.created_at);

            if today {
                state.account.today_order_count += 1;
            }

            let snapshot = &mut state.ensure_strategy(&strategy.strategy).snapshot;
            snapshot.last_order_ms = Some(
                snapshot
                    .last_order_ms
                    .unwrap_or(strategy.created_at)
                    .max(strategy.created_at),
            );
            if today {
                snapshot.today_order_count += 1;
            }

            let current_signal_time = snapshot
                .latest_signal
                .as_ref()
                .map(|signal| signal.signal_time_ms)
                .unwrap_or(i64::MIN);
            if current_signal_time < signal_time_ms {
                snapshot.latest_signal = latest_signal;
                snapshot.last_signal_ms = Some(signal_time_ms);
            }
        }

        for trade in history.trades.iter().rev() {
            let Some(meta) = trade
                .order_id
                .as_ref()
                .and_then(|order_id| state.order_meta.get(order_id))
                .cloned()
            else {
                continue;
            };

            let trade_time = trade.event_time.unwrap_or(trade.created_at);
            let today = is_today(trade.created_at);
            let notional = trade.price * trade.size;

            if today {
                state.account.today_trade_count += 1;
                state.account.today_notional_usdc =
                    add_decimal_string(&state.account.today_notional_usdc, notional);
            }

            state
                .asset_meta
                .insert(trade.asset_id.clone(), meta.clone());

            let strategy = state.ensure_strategy(&meta.strategy);
            strategy.snapshot.last_trade_ms = Some(
                strategy
                    .snapshot
                    .last_trade_ms
                    .unwrap_or(trade_time)
                    .max(trade_time),
            );

            if today {
                strategy.snapshot.today_trade_count += 1;
                strategy.snapshot.today_notional_usdc =
                    add_decimal_string(&strategy.snapshot.today_notional_usdc, notional);
            }
        }
    }

    pub fn load_strategy_attribution(&self, strategies: &[events::Strategy]) {
        let Ok(mut state) = self.state.write() else {
            return;
        };

        for strategy in strategies.iter().rev() {
            state.register_order_meta(
                &strategy.order_id,
                OrderMeta {
                    strategy: strategy.strategy.clone(),
                    market_slug: strategy.market_slug.clone(),
                    side: strategy.side.clone(),
                    created_at_ms: Some(strategy.created_at),
                },
            );
        }
    }

    pub fn positions(&self, strategy: &str) -> PositionsPayload {
        let Ok(state) = self.state.read() else {
            return PositionsPayload {
                strategy: strategy.to_string(),
                total: 0,
                rows: Vec::new(),
            };
        };

        let Some(strategy_state) = state.strategies.get(strategy) else {
            return PositionsPayload {
                strategy: strategy.to_string(),
                total: 0,
                rows: Vec::new(),
            };
        };

        PositionsPayload {
            strategy: strategy.to_string(),
            total: strategy_state.positions.len(),
            rows: strategy_state.positions.clone(),
        }
    }

    pub fn open_orders(&self, strategy: &str) -> OpenOrdersPayload {
        let Ok(state) = self.state.read() else {
            return OpenOrdersPayload {
                strategy: strategy.to_string(),
                total: 0,
                rows: Vec::new(),
            };
        };

        let Some(strategy_state) = state.strategies.get(strategy) else {
            return OpenOrdersPayload {
                strategy: strategy.to_string(),
                total: 0,
                rows: Vec::new(),
            };
        };

        OpenOrdersPayload {
            strategy: strategy.to_string(),
            total: strategy_state.open_orders.len(),
            rows: strategy_state.open_orders.clone(),
        }
    }

    pub fn closed_positions_page(
        &self,
        strategy: &str,
        range: Option<&str>,
        page: usize,
        page_size: usize,
    ) -> ClosedPositionsPagePayload {
        let safe_page_size = page_size.clamp(1, 100);
        let safe_page = page.max(1);
        let safe_range = normalize_range(range);

        let Ok(state) = self.state.read() else {
            return ClosedPositionsPagePayload {
                strategy: strategy.to_string(),
                range: safe_range.to_string(),
                page: 1,
                page_size: safe_page_size,
                total: 0,
                total_pages: 0,
                rows: Vec::new(),
            };
        };

        let Some(_) = state.strategies.get(strategy) else {
            return ClosedPositionsPagePayload {
                strategy: strategy.to_string(),
                range: safe_range.to_string(),
                page: 1,
                page_size: safe_page_size,
                total: 0,
                total_pages: 0,
                rows: Vec::new(),
            };
        };

        let rows_source = self
            .closed_positions_snapshot()
            .and_then(|closed_positions| closed_positions_rows(&state, strategy, &closed_positions))
            .unwrap_or_default();
        let rows = filter_closed_positions(&rows_source, safe_range);
        let total = rows.len();
        let total_pages = total.div_ceil(safe_page_size);
        let page = if total_pages == 0 {
            1
        } else {
            safe_page.min(total_pages)
        };
        let start = (page - 1) * safe_page_size;
        let end = total.min(start + safe_page_size);
        let rows = if start >= total {
            Vec::new()
        } else {
            rows[start..end].to_vec()
        };

        ClosedPositionsPagePayload {
            strategy: strategy.to_string(),
            range: safe_range.to_string(),
            page,
            page_size: safe_page_size,
            total,
            total_pages,
            rows,
        }
    }

    fn snapshot_payload(&self) -> SnapshotPayload {
        let Ok(state) = self.state.read() else {
            return SnapshotPayload {
                account: AccountSnapshot {
                    runtime_status: "degraded".to_string(),
                    binance_ws_status: "disconnected".to_string(),
                    polymarket_ws_status: "disconnected".to_string(),
                    server_time_ms: now_ms(),
                    open_order_count: 0,
                    position_count: 0,
                    today_order_count: 0,
                    today_trade_count: 0,
                    today_notional_usdc: "0".to_string(),
                    settled_count: 0,
                    settled_pnl_usdc: "0".to_string(),
                    last_error: Some("dashboard state lock poisoned".to_string()),
                },
                strategies: Vec::new(),
            };
        };

        let mut account = state.account.clone();
        account.server_time_ms = now_ms();

        let mut strategies: Vec<_> = state
            .strategies
            .values()
            .map(|strategy| strategy.snapshot.clone())
            .collect();

        if let Some(closed_positions) = self.closed_positions_snapshot() {
            let settlements = settlement_summary_by_strategy(&state, &closed_positions);
            account.settled_count = settlements.values().map(|rows| rows.len()).sum();
            account.settled_pnl_usdc = settlements
                .values()
                .flat_map(|rows| rows.iter())
                .fold(Decimal::ZERO, |acc, row| {
                    acc + parse_decimal(&row.realized_pnl)
                })
                .round_dp(4)
                .normalize()
                .to_string();

            for strategy in &mut strategies {
                let rows = settlements
                    .get(&strategy.strategy)
                    .cloned()
                    .unwrap_or_default();
                strategy.settled_count = rows.len();
                strategy.settled_pnl_usdc = rows
                    .iter()
                    .fold(Decimal::ZERO, |acc, row| {
                        acc + parse_decimal(&row.realized_pnl)
                    })
                    .round_dp(4)
                    .normalize()
                    .to_string();
            }
        }
        strategies.sort_by_key(|strategy| std::cmp::Reverse(strategy_latest_time(strategy)));

        SnapshotPayload {
            account,
            strategies,
        }
    }

    fn closed_positions_snapshot(&self) -> Option<Vec<ClosedPosition>> {
        self.closed_positions_cache
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().cloned())
            .map(|cache| cache.snapshot())
    }
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn normalize_range(range: Option<&str>) -> &'static str {
    match range.unwrap_or("all").to_ascii_lowercase().as_str() {
        "1d" => "1d",
        "1w" => "1w",
        "1m" => "1m",
        _ => "all",
    }
}

fn filter_closed_positions(rows: &[SettlementPayload], range: &str) -> Vec<SettlementPayload> {
    let Some(min_closed_at_ms) = range_start_ms(range) else {
        return rows.to_vec();
    };

    rows.iter()
        .filter(|row| row.closed_at_ms >= min_closed_at_ms)
        .cloned()
        .collect()
}

fn settlement_summary_by_strategy(
    state: &InnerState,
    closed_positions: &[ClosedPosition],
) -> HashMap<String, Vec<SettlementPayload>> {
    let mut grouped = HashMap::<String, Vec<SettlementPayload>>::new();

    for closed in closed_positions {
        let strategy_name = state
            .asset_meta
            .get(&closed.asset.to_string())
            .map(|meta| meta.strategy.clone())
            .or_else(|| state.market_strategy.get(&closed.slug).cloned());

        let Some(strategy_name) = strategy_name else {
            continue;
        };

        grouped
            .entry(strategy_name)
            .or_default()
            .push(settlement_payload(closed));
    }

    for rows in grouped.values_mut() {
        rows.sort_by_key(|row| std::cmp::Reverse(row.closed_at_ms));
    }

    grouped
}

fn closed_positions_rows(
    state: &InnerState,
    strategy: &str,
    closed_positions: &[ClosedPosition],
) -> Option<Vec<SettlementPayload>> {
    let mut grouped = settlement_summary_by_strategy(state, closed_positions);
    grouped.remove(strategy)
}

fn range_start_ms(range: &str) -> Option<i64> {
    let now = Utc::now();
    let start = match range {
        "1d" => now - chrono::Duration::days(1),
        "1w" => now - chrono::Duration::weeks(1),
        "1m" => now - chrono::Duration::days(30),
        _ => return None,
    };

    Some(start.timestamp_millis())
}

fn signal_payload(candidate: &Candidate) -> SignalPayload {
    SignalPayload {
        symbol: symbol_slug(candidate.symbol).to_string(),
        interval: candidate.interval.as_slug().to_string(),
        market_slug: candidate.market_slug.clone(),
        side: strategy_side_name(candidate.side).to_string(),
        signal_time_ms: candidate.signal_time_ms,
        score: candidate.score,
        size_factor: candidate.size_factor,
    }
}

fn open_order_payload(order: &OpenOrder, meta: &OrderMeta) -> OrderPayload {
    OrderPayload {
        order_id: order.id.clone(),
        market_slug: Some(meta.market_slug.clone()),
        side: order.outcome.clone().or_else(|| Some(meta.side.clone())),
        order_side: Some(format!("{:?}", order.side).to_ascii_lowercase()),
        status: format!("{:?}", order.status).to_ascii_lowercase(),
        price: order.price.round_dp(4).normalize().to_string(),
        size: (order.original_size - order.size_matched)
            .max(Decimal::ZERO)
            .round_dp(4)
            .normalize()
            .to_string(),
        created_at_ms: meta.created_at_ms.unwrap_or_default(),
    }
}

fn position_payload(position: &Position, market_slug: Option<&str>) -> PositionPayload {
    PositionPayload {
        asset_id: position.asset_id.to_string(),
        market_slug: market_slug.map(str::to_string),
        outcome: position.outcome.clone(),
        size: position.size.round_dp(4).normalize().to_string(),
        avg_price: position.avg_price.round_dp(4).normalize().to_string(),
        open_cost: position.open_cost().round_dp(4).normalize().to_string(),
        realized_pnl: position.realized_pnl.round_dp(4).normalize().to_string(),
        buy_fee_usdc: position.buy_fee_usdc.round_dp(4).normalize().to_string(),
        buy_fee_shares: position.buy_fee_shares.round_dp(4).normalize().to_string(),
        sell_fee_usdc: position.sell_fee_usdc.round_dp(4).normalize().to_string(),
        last_trade_ms: position.last_trade_ts,
    }
}

fn settlement_payload(closed: &ClosedPosition) -> SettlementPayload {
    SettlementPayload {
        raw: RawClosedPositionPayload::from_closed(closed),
        asset_id: closed.asset.to_string(),
        market_slug: closed.slug.clone(),
        avg_price: closed.avg_price.round_dp(4).normalize().to_string(),
        total_bought: closed.total_bought.round_dp(4).normalize().to_string(),
        realized_pnl: closed.realized_pnl.round_dp(4).normalize().to_string(),
        closed_at_ms: normalize_timestamp_ms(closed.timestamp),
    }
}

fn normalize_timestamp_ms(timestamp: i64) -> i64 {
    if timestamp.abs() < 1_000_000_000_000 {
        timestamp.saturating_mul(1_000)
    } else {
        timestamp
    }
}

fn strategy_side_name(side: Side) -> &'static str {
    match side {
        Side::Up => "up",
        Side::Down => "down",
    }
}

fn symbol_slug(symbol: Symbol) -> &'static str {
    symbol.as_slug()
}

fn add_decimal_string(base: &str, value: Decimal) -> String {
    let base = base.parse::<Decimal>().unwrap_or(Decimal::ZERO);
    (base + value).round_dp(4).normalize().to_string()
}

fn parse_decimal(value: &str) -> Decimal {
    value.parse::<Decimal>().unwrap_or(Decimal::ZERO)
}

fn strategy_latest_time(strategy: &StrategySnapshot) -> i64 {
    [
        strategy.last_trade_ms.unwrap_or(0),
        strategy.last_order_ms.unwrap_or(0),
        strategy.last_signal_ms.unwrap_or(0),
        strategy.last_scan_ms.unwrap_or(0),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

fn parse_strategy_signal(strategy: &events::Strategy) -> Option<SignalPayload> {
    let event: serde_json::Value = serde_json::from_str(&strategy.event).ok()?;
    let trigger = event.get("trigger")?;

    Some(SignalPayload {
        symbol: trigger
            .get("symbol")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&strategy.symbol)
            .to_string(),
        interval: trigger
            .get("interval")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&strategy.interval)
            .to_string(),
        market_slug: trigger
            .get("market_slug")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&strategy.market_slug)
            .to_string(),
        side: trigger
            .get("side")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&strategy.side)
            .to_string(),
        signal_time_ms: extract_signal_time_ms(strategy).unwrap_or(strategy.created_at),
        score: event
            .get("score")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_default(),
        size_factor: event
            .get("size_factor")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0),
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use polymarket_client_sdk::clob::types::{OrderStatusType, Side as MarketSide};
    use polymarket_client_sdk::data::types::response::ClosedPosition;
    use polymarket_client_sdk::types::{B256, U256};
    use rust_decimal::Decimal;

    use super::Handle;
    use crate::events;
    use crate::polymarket::types::open_orders::Order as OpenOrder;
    use crate::polymarket::types::positions::Position;
    use crate::polymarket::user_task::ClosedPositionsCache;
    use crate::strategy::crypto_reversal::model::Side;
    use crate::strategy::crypto_reversal::service::Candidate;
    use crate::types::crypto::{Interval, Symbol};

    #[test]
    fn closed_positions_can_use_hydrated_market_strategy() {
        let dashboard = Handle::new();
        dashboard.register_strategy("crypto_reversal");
        dashboard.load_strategy_attribution(&[events::Strategy {
            order_id: "order-older".to_string(),
            strategy: "crypto_reversal".to_string(),
            symbol: "eth".to_string(),
            interval: "5m".to_string(),
            market_slug: "eth-updown-5m-old".to_string(),
            side: "up".to_string(),
            created_at: 100,
            event: "{\"signal_time_ms\":90}".to_string(),
        }]);

        let closed: ClosedPosition = serde_json::from_value(serde_json::json!({
            "proxyWallet": "0x0000000000000000000000000000000000000000",
            "asset": "123",
            "conditionId": format!("0x{}", "0".repeat(64)),
            "avgPrice": Decimal::new(43, 2),
            "totalBought": Decimal::new(8, 0),
            "realizedPnl": Decimal::new(57, 2),
            "curPrice": Decimal::ONE,
            "timestamp": 1710003600000_i64,
            "title": "ETH test",
            "slug": "eth-updown-5m-old",
            "icon": "",
            "eventSlug": "eth",
            "outcome": "Yes",
            "outcomeIndex": 0,
            "oppositeOutcome": "No",
            "oppositeAsset": "456",
            "endDate": Utc
                .timestamp_millis_opt(1710003600000)
                .single()
                .unwrap()
                .to_rfc3339(),
        }))
        .expect("closed position should deserialize");
        let cache = ClosedPositionsCache::new();
        cache.replace(vec![closed]);
        dashboard.attach_closed_positions_cache(cache);

        let payload =
            serde_json::to_value(dashboard.snapshot()).expect("snapshot should serialize");
        let strategies = payload["strategies"]
            .as_array()
            .expect("strategies should be an array");
        let strategy = strategies
            .iter()
            .find(|item| item["strategy"] == "crypto_reversal")
            .expect("strategy snapshot should exist");

        assert_eq!(payload["account"]["settled_count"], 1);
        assert_eq!(payload["account"]["settled_pnl_usdc"], "0.57");
        assert_eq!(strategy["settled_count"], 1);
        assert_eq!(strategy["settled_pnl_usdc"], "0.57");
        assert_eq!(strategy["settled_count"], 1);

        let page = dashboard.closed_positions_page("crypto_reversal", None, 1, 10);
        assert_eq!(
            page.rows[0].raw.proxy_wallet.to_string(),
            "0x0000000000000000000000000000000000000000"
        );
        assert_eq!(page.rows[0].raw.asset.to_string(), "123");
        assert_eq!(
            page.rows[0].raw.condition_id.to_string(),
            format!("0x{}", "0".repeat(64))
        );
        assert_eq!(page.rows[0].raw.avg_price, "0.43");
        assert_eq!(page.rows[0].raw.total_bought, "8");
        assert_eq!(page.rows[0].raw.realized_pnl, "0.57");
        assert_eq!(page.rows[0].raw.cur_price, "1");
        assert_eq!(page.rows[0].raw.title, "ETH test");
        assert_eq!(page.rows[0].raw.slug, "eth-updown-5m-old");
        assert_eq!(page.rows[0].raw.event_slug, "eth");
        assert_eq!(page.rows[0].raw.outcome_index, 0);
        assert_eq!(page.rows[0].raw.opposite_outcome, "No");
        assert_eq!(page.rows[0].raw.opposite_asset.to_string(), "456");
        assert_eq!(page.rows[0].raw.timestamp, 1710003600000_i64);
    }

    #[test]
    fn closed_positions_page_supports_pagination() {
        let dashboard = Handle::new();
        dashboard.register_strategy("crypto_reversal");
        dashboard.load_strategy_attribution(&[events::Strategy {
            order_id: "order-page".to_string(),
            strategy: "crypto_reversal".to_string(),
            symbol: "eth".to_string(),
            interval: "5m".to_string(),
            market_slug: "eth-updown-5m-page".to_string(),
            side: "up".to_string(),
            created_at: 100,
            event: "{\"signal_time_ms\":90}".to_string(),
        }]);

        let mut closed_positions = Vec::new();
        for index in 0..3 {
            let timestamp = 1710003600000_i64 + index as i64;
            let closed: ClosedPosition = serde_json::from_value(serde_json::json!({
                "proxyWallet": "0x0000000000000000000000000000000000000000",
                "asset": format!("{}", 100 + index),
                "conditionId": format!("0x{}", "0".repeat(64)),
                "avgPrice": Decimal::new(43, 2),
                "totalBought": Decimal::new(8 + index as i64, 0),
                "realizedPnl": Decimal::new(57 + index as i64, 2),
                "curPrice": Decimal::ONE,
                "timestamp": timestamp,
                "title": "ETH test",
                "slug": "eth-updown-5m-page",
                "icon": "",
                "eventSlug": "eth",
                "outcome": "Yes",
                "outcomeIndex": 0,
                "oppositeOutcome": "No",
                "oppositeAsset": "456",
                "endDate": Utc
                    .timestamp_millis_opt(timestamp)
                    .single()
                    .unwrap()
                    .to_rfc3339(),
            }))
            .expect("closed position should deserialize");
            closed_positions.push(closed);
        }

        let cache = ClosedPositionsCache::new();
        cache.replace(closed_positions);
        dashboard.attach_closed_positions_cache(cache);

        let first_page = dashboard.closed_positions_page("crypto_reversal", None, 1, 2);
        assert_eq!(first_page.total, 3);
        assert_eq!(first_page.total_pages, 2);
        assert_eq!(first_page.rows.len(), 2);
        assert_eq!(first_page.rows[0].asset_id, "102");
        assert_eq!(first_page.rows[1].asset_id, "101");

        let second_page = dashboard.closed_positions_page("crypto_reversal", None, 2, 2);
        assert_eq!(second_page.rows.len(), 1);
        assert_eq!(second_page.rows[0].asset_id, "100");
    }

    #[test]
    fn positions_and_open_orders_are_available_via_detail_apis() {
        let dashboard = Handle::new();
        dashboard.register_strategy("crypto_reversal");
        dashboard.order_submission(
            "crypto_reversal",
            &Candidate {
                symbol: Symbol::Eth,
                interval: Interval::M5,
                market_slug: "eth-updown-5m-live".to_string(),
                side: Side::Up,
                signal_time_ms: 1710000000000,
                score: 1.0,
                size_factor: 1.0,
            },
            U256::from(123_u64),
            "order-live",
            Decimal::new(43, 2),
            Decimal::new(10, 0),
        );

        dashboard.user_state(
            &[OpenOrder {
                id: "order-live".to_string(),
                market_id: B256::ZERO,
                asset_id: U256::from(123_u64),
                side: MarketSide::Buy,
                price: Decimal::new(43, 2),
                original_size: Decimal::new(10, 0),
                size_matched: Decimal::new(2, 0),
                status: OrderStatusType::Live,
                outcome: Some("Yes".to_string()),
                trade_ids: Vec::new(),
            }],
            &[Position {
                market_id: B256::ZERO,
                asset_id: U256::from(123_u64),
                outcome: Some("Yes".to_string()),
                size: Decimal::new(8, 0),
                avg_price: Decimal::new(43, 2),
                realized_pnl: Decimal::new(12, 2),
                buy_fee_usdc: Decimal::ZERO,
                buy_fee_shares: Decimal::ZERO,
                sell_fee_usdc: Decimal::ZERO,
                last_trade_ts: Some(1710000000001),
            }],
        );

        let positions = dashboard.positions("crypto_reversal");
        assert_eq!(positions.total, 1);
        assert_eq!(
            positions.rows[0].market_slug.as_deref(),
            Some("eth-updown-5m-live")
        );

        let open_orders = dashboard.open_orders("crypto_reversal");
        assert_eq!(open_orders.total, 1);
        assert_eq!(
            open_orders.rows[0].market_slug.as_deref(),
            Some("eth-updown-5m-live")
        );
        assert_eq!(open_orders.rows[0].size, "8");
    }
}

fn extract_signal_time_ms(strategy: &events::Strategy) -> Option<i64> {
    let event: serde_json::Value = serde_json::from_str(&strategy.event).ok()?;
    event
        .get("signal_time_ms")
        .and_then(serde_json::Value::as_i64)
}

fn is_today(timestamp_ms: i64) -> bool {
    match chrono::DateTime::from_timestamp_millis(timestamp_ms) {
        Some(value) => value.date_naive() == Utc::now().date_naive(),
        None => false,
    }
}
