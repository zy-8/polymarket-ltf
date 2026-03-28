use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use crate::events;
use crate::polymarket::types::open_orders::Order as OpenOrder;
use crate::polymarket::types::positions::Position;
use crate::storage::sqlite::{DashboardHistory, InfoStats, PositionRecord, Store};
use crate::strategy::crypto_reversal::model::Side;
use crate::strategy::crypto_reversal::service::Candidate;
use crate::types::crypto::Symbol;
use chrono::{NaiveDate, Utc};
use polymarket_client_sdk::types::U256;
use rust_decimal::Decimal;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct InfoPayload {
    pub account: AccountInfo,
    pub strategies: Vec<StrategyInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountInfo {
    pub runtime_status: String,
    pub binance_ws_status: String,
    pub polymarket_ws_status: String,
    pub server_time_ms: i64,
    pub open_order_count: usize,
    pub position_count: usize,
    pub trigger_count: usize,
    pub closed_count: usize,
    pub today_closed_count: usize,
    pub closed_win_count: usize,
    pub closed_loss_count: usize,
    pub missed_count: usize,
    pub missed_win_count: usize,
    pub missed_loss_count: usize,
    pub today_order_count: usize,
    pub today_trade_count: usize,
    pub today_closed_pnl_usdc: String,
    pub settled_pnl_usdc: String,
    pub last_error: Option<String>,
}

impl AccountInfo {
    fn starting() -> Self {
        Self {
            runtime_status: "starting".to_string(),
            binance_ws_status: "connecting".to_string(),
            polymarket_ws_status: "connecting".to_string(),
            server_time_ms: Utc::now().timestamp_millis(),
            open_order_count: 0,
            position_count: 0,
            trigger_count: 0,
            closed_count: 0,
            today_closed_count: 0,
            closed_win_count: 0,
            closed_loss_count: 0,
            missed_count: 0,
            missed_win_count: 0,
            missed_loss_count: 0,
            today_order_count: 0,
            today_trade_count: 0,
            today_closed_pnl_usdc: "0".to_string(),
            settled_pnl_usdc: "0".to_string(),
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StrategyInfo {
    pub strategy: String,
    pub status: String,
    pub outcome: String,
    pub last_scan_ms: Option<i64>,
    pub last_signal_ms: Option<i64>,
    pub last_order_ms: Option<i64>,
    pub last_trade_ms: Option<i64>,
    pub open_order_count: usize,
    pub position_count: usize,
    pub trigger_count: usize,
    pub closed_count: usize,
    pub today_closed_count: usize,
    pub closed_win_count: usize,
    pub closed_loss_count: usize,
    pub missed_count: usize,
    pub missed_win_count: usize,
    pub missed_loss_count: usize,
    pub today_order_count: usize,
    pub today_trade_count: usize,
    pub today_closed_pnl_usdc: String,
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
pub struct RedeemablePositionPayload {
    #[serde(flatten)]
    pub raw: RawPositionPayload,
}

#[derive(Debug, Clone, Serialize)]
pub struct RawPositionPayload {
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: String,
    pub asset: String,
    #[serde(rename = "conditionId")]
    pub condition_id: String,
    #[serde(rename = "marketSlug")]
    pub market_slug: String,
    pub outcome: String,
    #[serde(rename = "avgPrice")]
    pub avg_price: String,
    #[serde(rename = "size", skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(rename = "totalBought", skip_serializing_if = "Option::is_none")]
    pub total_bought: Option<String>,
    #[serde(rename = "currentValue", skip_serializing_if = "Option::is_none")]
    pub current_value: Option<String>,
    #[serde(rename = "cashPnl", skip_serializing_if = "Option::is_none")]
    pub cash_pnl: Option<String>,
    #[serde(rename = "realizedPnl")]
    pub realized_pnl: String,
    #[serde(rename = "curPrice")]
    pub cur_price: String,
    #[serde(rename = "endDate")]
    pub end_date: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PositionsPagePayload {
    pub strategy: String,
    pub range: String,
    pub page: usize,
    pub page_size: usize,
    pub total: usize,
    pub total_pages: usize,
    pub rows: Vec<RedeemablePositionPayload>,
}

impl PositionsPagePayload {
    fn empty(strategy: &str, range: &str, page_size: usize) -> Self {
        Self {
            strategy: strategy.to_string(),
            range: range.to_string(),
            page: 1,
            page_size,
            total: 0,
            total_pages: 0,
            rows: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PositionsPayload {
    pub strategy: String,
    pub total: usize,
    pub rows: Vec<PositionPayload>,
}

impl PositionsPayload {
    fn empty(strategy: &str) -> Self {
        Self {
            strategy: strategy.to_string(),
            total: 0,
            rows: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenOrdersPayload {
    pub strategy: String,
    pub total: usize,
    pub rows: Vec<OrderPayload>,
}

impl OpenOrdersPayload {
    fn empty(strategy: &str) -> Self {
        Self {
            strategy: strategy.to_string(),
            total: 0,
            rows: Vec::new(),
        }
    }
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
    info: StrategyInfo,
    open_orders: Vec<OrderPayload>,
    positions: Vec<PositionPayload>,
}

impl StrategyState {
    fn new(strategy: &str) -> Self {
        Self {
            info: StrategyInfo {
                strategy: strategy.to_string(),
                status: "starting".to_string(),
                outcome: String::new(),
                last_scan_ms: None,
                last_signal_ms: None,
                last_order_ms: None,
                last_trade_ms: None,
                open_order_count: 0,
                position_count: 0,
                trigger_count: 0,
                closed_count: 0,
                today_closed_count: 0,
                closed_win_count: 0,
                closed_loss_count: 0,
                missed_count: 0,
                missed_win_count: 0,
                missed_loss_count: 0,
                today_order_count: 0,
                today_trade_count: 0,
                today_closed_pnl_usdc: "0".to_string(),
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
    account: AccountInfo,
    strategies: BTreeMap<String, StrategyState>,
    order_meta: HashMap<String, OrderMeta>,
    asset_meta: HashMap<String, OrderMeta>,
}

impl InnerState {
    fn new() -> Self {
        Self {
            day: Utc::now().date_naive(),
            account: AccountInfo::starting(),
            strategies: BTreeMap::new(),
            order_meta: HashMap::new(),
            asset_meta: HashMap::new(),
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
        self.account.today_closed_pnl_usdc = "0".to_string();

        for strategy in self.strategies.values_mut() {
            strategy.info.today_order_count = 0;
            strategy.info.today_trade_count = 0;
            strategy.info.today_closed_pnl_usdc = "0".to_string();
        }
    }

    fn register_order_meta(&mut self, order_id: &str, meta: OrderMeta) {
        self.order_meta.insert(order_id.to_string(), meta);
    }

    fn register_strategy_meta(&mut self, strategy: &events::Strategy) {
        let meta = OrderMeta {
            strategy: strategy.strategy.clone(),
            market_slug: strategy.market_slug.clone(),
            side: strategy.side.clone(),
            created_at_ms: Some(strategy.created_at),
        };
        self.register_order_meta(&strategy.order_id, meta.clone());
        self.asset_meta.insert(strategy.asset_id.clone(), meta);
    }
}

#[derive(Clone)]
pub struct Handle {
    state: Arc<RwLock<InnerState>>,
    store: Arc<RwLock<Option<Store>>>,
}

impl Handle {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(InnerState::new())),
            store: Arc::new(RwLock::new(None)),
        }
    }

    pub fn attach_store(&self, store: Store) {
        if let Ok(mut guard) = self.store.write() {
            *guard = Some(store);
        }
    }

    fn store(&self) -> Option<Store> {
        self.store.read().ok().and_then(|guard| guard.clone())
    }

    pub async fn info(&self) -> crate::errors::Result<InfoPayload> {
        let (mut account, mut strategies) = {
            let state = self.state.read().map_err(|_| {
                crate::errors::PolyfillError::internal_simple("dashboard state lock poisoned")
            })?;

            let mut account = state.account.clone();
            account.server_time_ms = now_ms();

            let strategies: Vec<_> = state
                .strategies
                .values()
                .map(|strategy| strategy.info.clone())
                .collect();

            (account, strategies)
        };

        if let Some(store) = self.store() {
            if let Ok(stats) = store.select_info_stats().await {
                apply_info_stats(&mut account, &mut strategies, &stats);
            }
        }
        strategies.sort_by_key(|strategy| std::cmp::Reverse(strategy_latest_time(strategy)));

        Ok(InfoPayload {
            account,
            strategies,
        })
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
            state.ensure_strategy(strategy).info.last_scan_ms = Some(now_ms());
        }
    }

    pub fn strategy_status(&self, strategy: &str, status: &str) {
        if let Ok(mut state) = self.state.write() {
            let info = &mut state.ensure_strategy(strategy).info;
            info.status = status.to_string();
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
            strategy.info.open_order_count = strategy_open_orders
                .get(&strategy.info.strategy)
                .copied()
                .unwrap_or_default();
            let mut open_orders = strategy_open_order_rows
                .remove(&strategy.info.strategy)
                .unwrap_or_default();
            open_orders.sort_by_key(|order| std::cmp::Reverse(order.created_at_ms));
            strategy.open_orders = open_orders;
            let mut positions = strategy_positions
                .remove(&strategy.info.strategy)
                .unwrap_or_default();
            let position_count = positions.len();
            positions
                .sort_by_key(|position| std::cmp::Reverse(position.last_trade_ms.unwrap_or(0)));
            strategy.info.position_count = position_count;
            strategy.positions = positions;
        }
    }

    pub fn signal(&self, strategy: &str, candidate: &Candidate) {
        let signal = signal_payload(candidate);
        let Ok(mut state) = self.state.write() else {
            return;
        };

        state.maybe_reset_day();
        let info = &mut state.ensure_strategy(strategy).info;
        info.last_signal_ms = Some(now_ms());
        info.latest_signal = Some(signal);
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
        strategy_state.info.last_order_ms = Some(order.created_at_ms);
        strategy_state.info.today_order_count += 1;
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
        strategy.info.last_order_ms = Some(order.created_at);
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
            add_decimal_string(&state.account.today_closed_pnl_usdc, notional);
        state.account.today_closed_pnl_usdc = next_account_notional;

        if let Some(meta) = strategy_meta {
            let strategy = state.ensure_strategy(&meta.strategy);
            strategy.info.last_trade_ms = Some(trade.event_time.unwrap_or(trade.created_at));
            strategy.info.today_trade_count += 1;
            strategy.info.today_closed_pnl_usdc =
                add_decimal_string(&strategy.info.today_closed_pnl_usdc, notional);
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
            strategy.info.last_error = Some(message);
            strategy.info.status = "degraded".to_string();
        }
    }

    pub fn load_history(&self, history: &DashboardHistory) {
        let Ok(mut state) = self.state.write() else {
            return;
        };

        state.maybe_reset_day();

        for strategy in history.strategies.iter().rev() {
            state.register_strategy_meta(strategy);

            let signal_time_ms = extract_signal_time_ms(strategy).unwrap_or(strategy.created_at);
            let latest_signal = parse_strategy_signal(strategy);
            let today = is_today(strategy.created_at);

            if today {
                state.account.today_order_count += 1;
            }

            let info = &mut state.ensure_strategy(&strategy.strategy).info;
            info.outcome = strategy.outcome.clone();
            info.last_order_ms = Some(
                info.last_order_ms
                    .unwrap_or(strategy.created_at)
                    .max(strategy.created_at),
            );
            if today {
                info.today_order_count += 1;
            }

            let current_signal_time = info
                .latest_signal
                .as_ref()
                .map(|signal| signal.signal_time_ms)
                .unwrap_or(i64::MIN);
            if current_signal_time < signal_time_ms {
                info.latest_signal = latest_signal;
                info.last_signal_ms = Some(signal_time_ms);
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
                state.account.today_closed_pnl_usdc =
                    add_decimal_string(&state.account.today_closed_pnl_usdc, notional);
            }

            let strategy = state.ensure_strategy(&meta.strategy);
            strategy.info.last_trade_ms = Some(
                strategy
                    .info
                    .last_trade_ms
                    .unwrap_or(trade_time)
                    .max(trade_time),
            );

            if today {
                strategy.info.today_trade_count += 1;
                strategy.info.today_closed_pnl_usdc =
                    add_decimal_string(&strategy.info.today_closed_pnl_usdc, notional);
            }
        }
    }

    pub fn load_strategy_attribution(&self, strategies: &[events::Strategy]) {
        let Ok(mut state) = self.state.write() else {
            return;
        };

        for strategy in strategies.iter().rev() {
            state.register_strategy_meta(strategy);
            state.ensure_strategy(&strategy.strategy).info.outcome = strategy.outcome.clone();
        }
    }

    pub fn positions(&self, strategy: &str) -> PositionsPayload {
        let Ok(state) = self.state.read() else {
            return PositionsPayload::empty(strategy);
        };

        let Some(strategy_state) = state.strategies.get(strategy) else {
            return PositionsPayload::empty(strategy);
        };

        PositionsPayload {
            strategy: strategy.to_string(),
            total: strategy_state.positions.len(),
            rows: strategy_state.positions.clone(),
        }
    }

    pub fn open_orders(&self, strategy: &str) -> OpenOrdersPayload {
        let Ok(state) = self.state.read() else {
            return OpenOrdersPayload::empty(strategy);
        };

        let Some(strategy_state) = state.strategies.get(strategy) else {
            return OpenOrdersPayload::empty(strategy);
        };

        OpenOrdersPayload {
            strategy: strategy.to_string(),
            total: strategy_state.open_orders.len(),
            rows: strategy_state.open_orders.clone(),
        }
    }

    pub async fn positions_page(
        &self,
        strategy: &str,
        range: Option<&str>,
        page: usize,
        page_size: usize,
    ) -> PositionsPagePayload {
        let safe_page_size = page_size.clamp(1, 100);
        let safe_page = page.max(1);
        let safe_range = normalize_range(range);

        let strategy_exists = {
            let Ok(state) = self.state.read() else {
                return PositionsPagePayload::empty(strategy, safe_range, safe_page_size);
            };

            state.strategies.contains_key(strategy)
        };

        if !strategy_exists {
            return PositionsPagePayload::empty(strategy, safe_range, safe_page_size);
        }

        let Some(store) = self.store() else {
            return PositionsPagePayload::empty(strategy, safe_range, safe_page_size);
        };

        let Ok(page_data) = store
            .select_positions_page(strategy, safe_range, safe_page, safe_page_size)
            .await
        else {
            return PositionsPagePayload::empty(strategy, safe_range, safe_page_size);
        };

        PositionsPagePayload {
            strategy: page_data.strategy,
            range: page_data.range,
            page: page_data.page,
            page_size: page_data.page_size,
            total: page_data.total,
            total_pages: page_data.total_pages,
            rows: page_data
                .rows
                .into_iter()
                .map(position_settlement_payload)
                .collect(),
        }
    }
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn apply_info_stats(account: &mut AccountInfo, strategies: &mut [StrategyInfo], stats: &InfoStats) {
    let strategy_stats = stats
        .strategies
        .iter()
        .map(|stats| (stats.strategy.as_str(), stats))
        .collect::<HashMap<_, _>>();

    account.trigger_count = stats.trigger_count;
    account.closed_count = stats.closed_count;
    account.today_closed_count = stats.today_closed_count;
    account.today_closed_pnl_usdc = stats.today_closed_pnl_usdc.clone();
    account.closed_win_count = stats.closed_win_count;
    account.closed_loss_count = stats.closed_loss_count;
    account.missed_count = stats.missed_count;
    account.missed_win_count = stats.missed_win_count;
    account.missed_loss_count = stats.missed_loss_count;
    account.settled_pnl_usdc = stats
        .strategies
        .iter()
        .fold(Decimal::ZERO, |acc, item| {
            acc + parse_decimal(&item.settled_pnl_usdc)
        })
        .round_dp(4)
        .normalize()
        .to_string();

    for strategy in strategies {
        if let Some(current) = strategy_stats.get(strategy.strategy.as_str()) {
            strategy.trigger_count = current.trigger_count;
            strategy.closed_count = current.closed_count;
            strategy.today_closed_count = current.today_closed_count;
            strategy.today_closed_pnl_usdc = current.today_closed_pnl_usdc.clone();
            strategy.closed_win_count = current.closed_win_count;
            strategy.closed_loss_count = current.closed_loss_count;
            strategy.missed_count = current.missed_count;
            strategy.missed_win_count = current.missed_win_count;
            strategy.missed_loss_count = current.missed_loss_count;
            strategy.settled_pnl_usdc = current.settled_pnl_usdc.clone();
        }
    }
}

fn normalize_range(range: Option<&str>) -> &'static str {
    match range.unwrap_or("all").to_ascii_lowercase().as_str() {
        "1d" => "1d",
        "1w" => "1w",
        "1m" => "1m",
        _ => "all",
    }
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

fn position_settlement_payload(position: PositionRecord) -> RedeemablePositionPayload {
    RedeemablePositionPayload {
        raw: RawPositionPayload {
            proxy_wallet: position.proxy_wallet,
            asset: position.asset,
            condition_id: position.condition_id,
            market_slug: position.market_slug,
            outcome: position.outcome,
            avg_price: position.avg_price,
            size: position.size,
            total_bought: position.total_bought,
            current_value: position.current_value,
            cash_pnl: position.cash_pnl,
            realized_pnl: position.realized_pnl,
            cur_price: position.cur_price,
            end_date: position.end_date,
            timestamp: position.timestamp,
        },
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

fn strategy_latest_time(strategy: &StrategyInfo) -> i64 {
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
    use chrono::Utc;
    use polymarket_client_sdk::clob::types::{OrderStatusType, Side as MarketSide};
    use polymarket_client_sdk::data::types::response::Position as DataPosition;
    use polymarket_client_sdk::types::{B256, U256};
    use rust_decimal::Decimal;

    use super::Handle;
    use crate::events;
    use crate::polymarket::types::open_orders::Order as OpenOrder;
    use crate::polymarket::types::positions::Position;
    use crate::storage::sqlite::Store;
    use crate::strategy::crypto_reversal::model::Side;
    use crate::strategy::crypto_reversal::service::Candidate;
    use crate::types::crypto::{Interval, Symbol};

    fn redeemable_position(
        asset: &str,
        slug: &str,
        total_bought: i64,
        pnl_cents: i64,
    ) -> DataPosition {
        serde_json::from_value(serde_json::json!({
            "proxyWallet": "0x0000000000000000000000000000000000000000",
            "asset": asset,
            "conditionId": format!("0x{}", "0".repeat(64)),
            "size": Decimal::new(total_bought, 0),
            "avgPrice": Decimal::new(43, 2),
            "initialValue": Decimal::new(total_bought * 43, 2),
            "currentValue": Decimal::ONE,
            "cashPnl": Decimal::new(pnl_cents, 2),
            "percentPnl": Decimal::ZERO,
            "totalBought": Decimal::new(total_bought, 0),
            "realizedPnl": Decimal::new(pnl_cents, 2),
            "percentRealizedPnl": Decimal::ZERO,
            "curPrice": Decimal::ONE,
            "redeemable": true,
            "mergeable": false,
            "title": "ETH test",
            "slug": slug,
            "icon": "",
            "eventSlug": "eth",
            "eventId": "",
            "outcome": "Yes",
            "outcomeIndex": 0,
            "oppositeOutcome": "No",
            "oppositeAsset": "456",
            "endDate": "2026-03-25",
            "negativeRisk": false,
        }))
        .expect("position should deserialize")
    }

    #[tokio::test]
    async fn positions_page_can_use_hydrated_market_strategy() {
        let dashboard = Handle::new();
        dashboard.register_strategy("crypto_reversal");
        let store = Store::open_memory()
            .await
            .expect("memory sqlite should open");
        store
            .insert_strategy(&events::Strategy {
                order_id: "order-older".to_string(),
                asset_id: "123".to_string(),
                strategy: "crypto_reversal".to_string(),
                symbol: "eth".to_string(),
                interval: "5m".to_string(),
                market_slug: "eth-updown-5m-old".to_string(),
                side: "up".to_string(),
                outcome: "up".to_string(),
                created_at: 100,
                event: "{\"signal_time_ms\":90}".to_string(),
            })
            .await
            .expect("strategy insert should work");

        let position = redeemable_position("123", "eth-updown-5m-old", 8, 57);
        store
            .insert_positions_at(&[position], 1_710_003_600_000)
            .await
            .expect("position insert should work");
        dashboard.load_strategy_attribution(
            &store
                .select_strategy_attribution()
                .await
                .expect("strategy attribution should load"),
        );
        dashboard.attach_store(store);

        let payload = serde_json::to_value(dashboard.info().await.expect("info should build"))
            .expect("info should serialize");
        let strategies = payload["strategies"]
            .as_array()
            .expect("strategies should be an array");
        let strategy = strategies
            .iter()
            .find(|item| item["strategy"] == "crypto_reversal")
            .expect("strategy info should exist");

        assert_eq!(strategy["outcome"], "up");

        let page = dashboard
            .positions_page("crypto_reversal", None, 1, 10)
            .await;
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
        assert_eq!(page.rows[0].raw.market_slug, "eth-updown-5m-old");
        assert_eq!(page.rows[0].raw.outcome, "Yes");
        assert_eq!(page.rows[0].raw.size.as_deref(), Some("8"));
        assert_eq!(page.rows[0].raw.total_bought.as_deref(), Some("8"));
        assert_eq!(page.rows[0].raw.current_value.as_deref(), Some("1"));
        assert_eq!(page.rows[0].raw.cash_pnl.as_deref(), Some("0.57"));
        assert_eq!(page.rows[0].raw.realized_pnl, "0.57");
        assert_eq!(page.rows[0].raw.cur_price, "1");
        assert_eq!(page.rows[0].raw.timestamp, 1710003600000_i64);
    }

    #[tokio::test]
    async fn info_reads_trigger_and_closed_counts_from_store() {
        let dashboard = Handle::new();
        dashboard.register_strategy("crypto_reversal");

        let store = Store::open_memory()
            .await
            .expect("memory sqlite should open");
        store
            .insert_strategy(&events::Strategy {
                order_id: "order-filled".to_string(),
                asset_id: "123".to_string(),
                strategy: "crypto_reversal".to_string(),
                symbol: "eth".to_string(),
                interval: "5m".to_string(),
                market_slug: "eth-updown-5m-filled".to_string(),
                side: "up".to_string(),
                outcome: "up".to_string(),
                created_at: 100,
                event: "{}".to_string(),
            })
            .await
            .expect("filled strategy insert should work");
        store
            .insert_strategy(&events::Strategy {
                order_id: "order-open".to_string(),
                asset_id: "456".to_string(),
                strategy: "crypto_reversal".to_string(),
                symbol: "eth".to_string(),
                interval: "15m".to_string(),
                market_slug: "eth-updown-15m-open".to_string(),
                side: "down".to_string(),
                outcome: "down".to_string(),
                created_at: 101,
                event: "{}".to_string(),
            })
            .await
            .expect("open strategy insert should work");
        store
            .insert_trade(&events::Trade {
                id: "trade-event-1".to_string(),
                order_id: Some("order-filled".to_string()),
                trade_id: "trade-1".to_string(),
                side: "buy".to_string(),
                price: Decimal::new(51, 2),
                size: Decimal::new(8, 0),
                fee_bps: None,
                event_time: Some(102),
                created_at: 102,
            })
            .await
            .expect("trade insert should work");

        let timestamp = Utc::now().timestamp_millis();
        let position = redeemable_position("123", "eth-updown-5m-filled", 8, 57);
        store
            .insert_positions_at(&[position], timestamp)
            .await
            .expect("position insert should work");

        dashboard.attach_store(store);

        let payload = serde_json::to_value(dashboard.info().await.expect("info should build"))
            .expect("info should serialize");
        let strategy = payload["strategies"]
            .as_array()
            .expect("strategies should be an array")
            .iter()
            .find(|item| item["strategy"] == "crypto_reversal")
            .expect("strategy info should exist");

        assert_eq!(payload["account"]["trigger_count"], 2);
        assert_eq!(payload["account"]["closed_count"], 1);
        assert_eq!(payload["account"]["today_closed_count"], 1);
        assert_eq!(payload["account"]["today_closed_pnl_usdc"], "0.57");
        assert_eq!(payload["account"]["closed_win_count"], 1);
        assert_eq!(payload["account"]["closed_loss_count"], 0);
        assert_eq!(payload["account"]["missed_count"], 1);
        assert_eq!(payload["account"]["missed_win_count"], 1);
        assert_eq!(payload["account"]["missed_loss_count"], 0);
        assert_eq!(strategy["trigger_count"], 2);
        assert_eq!(strategy["closed_count"], 1);
        assert_eq!(strategy["today_closed_count"], 1);
        assert_eq!(strategy["today_closed_pnl_usdc"], "0.57");
        assert_eq!(strategy["closed_win_count"], 1);
        assert_eq!(strategy["closed_loss_count"], 0);
        assert_eq!(strategy["missed_count"], 1);
        assert_eq!(strategy["missed_win_count"], 1);
        assert_eq!(strategy["missed_loss_count"], 0);
    }

    #[tokio::test]
    async fn positions_page_supports_pagination() {
        let dashboard = Handle::new();
        dashboard.register_strategy("crypto_reversal");
        let store = Store::open_memory()
            .await
            .expect("memory sqlite should open");
        for index in 0..3 {
            store
                .insert_strategy(&events::Strategy {
                    order_id: format!("order-page-{index}"),
                    asset_id: format!("{}", 100 + index),
                    strategy: "crypto_reversal".to_string(),
                    symbol: "eth".to_string(),
                    interval: "5m".to_string(),
                    market_slug: "eth-updown-5m-page".to_string(),
                    side: "up".to_string(),
                    outcome: String::new(),
                    created_at: 100 + index as i64,
                    event: "{\"signal_time_ms\":90}".to_string(),
                })
                .await
                .expect("strategy insert should work");
        }

        for index in 0..3 {
            let position = redeemable_position(
                &format!("{}", 100 + index),
                "eth-updown-5m-page",
                8 + index as i64,
                57 + index as i64,
            );
            store
                .insert_positions_at(&[position], 1710003600000_i64 + index as i64)
                .await
                .expect("positions insert should work");
        }
        dashboard.attach_store(store);

        let first_page = dashboard
            .positions_page("crypto_reversal", None, 1, 2)
            .await;
        assert_eq!(first_page.total, 3);
        assert_eq!(first_page.total_pages, 2);
        assert_eq!(first_page.rows.len(), 2);
        assert_eq!(first_page.rows[0].raw.asset, "102");
        assert_eq!(first_page.rows[1].raw.asset, "101");

        let second_page = dashboard
            .positions_page("crypto_reversal", None, 2, 2)
            .await;
        assert_eq!(second_page.rows.len(), 1);
        assert_eq!(second_page.rows[0].raw.asset, "100");
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
