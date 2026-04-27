//! Polymarket 用户账户状态维护。
//!
//! 这个模块的核心目标只有两个：
//! - 维护本地 open orders；
//! - 维护本地 positions。
//!
//! 启动时先获取远端 `open orders + positions`，
//! 之后再通过 authenticated user WebSocket 增量更新。

use crate::errors::{PolyfillError, Result, StreamErrorKind};
use crate::events;
use crate::polymarket::types::open_orders::{OpenOrders, Order};
use crate::polymarket::types::positions::{FeeRule, Fill, Position, Positions};
use crate::storage::sqlite;
use futures::StreamExt;
use polymarket_client_sdk_v2::auth::{ApiKey, Credentials, Normal, state::Authenticated};
use polymarket_client_sdk_v2::clob::{
    Client as ClobClient,
    types::{Side, TraderSide, request::OrdersRequest},
    ws::{ChannelType, Client as WsClient, WsMessage},
};
use polymarket_client_sdk_v2::data::{
    Client as DataClient, types::request::PositionsRequest as DataPositionsRequest,
};
use polymarket_client_sdk_v2::types::{Address, B256, U256};
use polymarket_client_sdk_v2::ws::connection::ConnectionState;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use tokio::task::AbortHandle;
use tokio::time::{Duration, sleep};
use tracing::{error, warn};

const TERMINAL_CURSOR: &str = "LTE=";
const REBOOTSTRAP_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub trait EventSink: Send + Sync {
    fn ws_status(&self, status: &str);
    fn user_state(&self, open_orders: &[Order], positions: &[Position]);
    fn error(&self, message: String);
    fn order(&self, order: &events::Order);
    fn trade(&self, trade: &events::Trade);
}

#[derive(Debug, Clone)]
struct Config {
    pub credentials: Credentials,
    pub address: Address,
}

impl Config {
    fn new(credentials: Credentials, address: Address) -> Self {
        Self {
            credentials,
            address,
        }
    }
}

#[derive(Debug, Clone)]
struct UserState {
    pub owner: ApiKey,
    pub open_orders: OpenOrders,
    pub order_contexts: HashMap<String, OrderContext>,
    pub positions: Positions,
}

#[derive(Debug, Clone)]
struct OrderContext {
    pub market_id: B256,
    pub asset_id: U256,
    pub side: Side,
    pub outcome: Option<String>,
}

impl UserState {
    pub fn new(owner: ApiKey) -> Self {
        Self {
            owner,
            open_orders: OpenOrders::new(),
            order_contexts: HashMap::new(),
            positions: Positions::new(),
        }
    }

    pub fn apply_order_message(
        &mut self,
        msg: &polymarket_client_sdk_v2::clob::ws::OrderMessage,
    ) -> Result<()> {
        self.open_orders.apply_order_message(msg)?;
        // `OrderMessage` 不更新仓位；它只维护活跃挂单和订单上下文，供 maker
        // 成交在后续 `TradeMessage` 中按 `order_id` 取回真实 `side`。
        self.order_contexts.insert(
            msg.id.clone(),
            OrderContext {
                market_id: msg.market,
                asset_id: msg.asset_id,
                side: msg.side,
                outcome: msg.outcome.clone(),
            },
        );
        Ok(())
    }

    pub fn apply_trade_message(
        &mut self,
        msg: &polymarket_client_sdk_v2::clob::ws::TradeMessage,
    ) -> Result<Option<Fill>> {
        let fill = fill_from_trade_message(msg, self.owner, &self.order_contexts)?;

        if let Some(fill) = fill.clone() {
            self.positions.apply_fill(fill)?;
        }

        self.open_orders.apply_trade_message(msg, self.owner)?;

        Ok(fill)
    }

    pub fn open_order_snapshot(&self) -> Vec<Order> {
        self.open_orders.all().cloned().collect()
    }

    pub fn position_snapshot(&self) -> Vec<Position> {
        self.positions.all().cloned().collect()
    }

    fn apply_open_order(
        &mut self,
        order: &polymarket_client_sdk_v2::clob::types::response::OpenOrderResponse,
    ) -> Result<()> {
        self.open_orders.apply_open_order(order)?;
        self.order_contexts.insert(
            order.id.clone(),
            OrderContext {
                market_id: order.market,
                asset_id: order.asset_id,
                side: order.side,
                outcome: if order.outcome.is_empty() {
                    None
                } else {
                    Some(order.outcome.clone())
                },
            },
        );
        Ok(())
    }

    fn prune_markets(&mut self, market_ids: &HashSet<B256>) -> (usize, usize, usize) {
        let pruned_orders = self.open_orders.prune_markets(market_ids);
        let pruned_positions = self.positions.prune_markets(market_ids);
        let contexts_before = self.order_contexts.len();
        self.order_contexts
            .retain(|_, context| !market_ids.contains(&context.market_id));
        let pruned_contexts = contexts_before - self.order_contexts.len();

        (pruned_orders, pruned_positions, pruned_contexts)
    }
}

fn fill_from_trade_message(
    msg: &polymarket_client_sdk_v2::clob::ws::TradeMessage,
    owner: ApiKey,
    order_contexts: &HashMap<String, OrderContext>,
) -> Result<Option<Fill>> {
    let timestamp = msg.matchtime.or(msg.timestamp).or(msg.last_update);

    match msg.trader_side.clone() {
        Some(TraderSide::Taker) => Ok(Some(build_fill(
            format!("trade:{}", msg.id),
            msg.market,
            msg.asset_id,
            msg.side,
            msg.size,
            msg.price,
            msg.fee_rate_bps,
            true,
            timestamp,
            msg.outcome.clone(),
        ))),
        Some(TraderSide::Unknown(raw)) => Err(PolyfillError::validation(format!(
            "不支持使用 trader_side={raw} 更新持仓"
        ))),
        Some(TraderSide::Maker) => {
            let maker_order = msg
                .maker_orders
                .iter()
                .find(|order| order.owner == owner)
                .ok_or_else(|| {
                    PolyfillError::validation(format!(
                        "maker trade {} 缺少属于当前账户的 maker_order",
                        msg.id
                    ))
                })?;
            let Some(context) = order_contexts.get(&maker_order.order_id) else {
                error!(
                    trade_id = %msg.id,
                    order_id = %maker_order.order_id,
                    "maker trade 缺少本地订单上下文，无法确定方向"
                );
                return Err(PolyfillError::validation(format!(
                    "maker trade {} 缺少本地订单上下文 {}",
                    msg.id, maker_order.order_id
                )));
            };

            Ok(Some(build_fill(
                format!("trade:{}:order:{}", msg.id, maker_order.order_id),
                context.market_id,
                context.asset_id,
                context.side,
                maker_order.matched_amount,
                maker_order.price,
                msg.fee_rate_bps,
                false,
                timestamp,
                context
                    .outcome
                    .clone()
                    .or_else(|| Some(maker_order.outcome.clone())),
            )))
        }
        Some(_) => Err(PolyfillError::validation(
            "不支持使用未知 TraderSide 更新持仓",
        )),
        None => Err(PolyfillError::validation("TradeMessage 缺少 trader_side")),
    }
}

fn build_fill(
    id: String,
    market_id: B256,
    asset_id: U256,
    side: Side,
    size: rust_decimal::Decimal,
    price: rust_decimal::Decimal,
    fee_rate_bps: Option<rust_decimal::Decimal>,
    is_taker: bool,
    timestamp: Option<i64>,
    outcome: Option<String>,
) -> Fill {
    Fill {
        id,
        market_id,
        asset_id,
        side,
        size,
        price,
        fee_rate_bps,
        is_taker,
        timestamp,
        outcome,
    }
}

pub struct Client {
    state: Arc<RwLock<UserState>>,
    tasks: Mutex<Vec<AbortHandle>>,
}

impl Client {
    async fn start_inner(
        config: Config,
        user_state: UserState,
        clob_client: ClobClient<Authenticated<Normal>>,
        store: Option<sqlite::Store>,
        sink: Option<Arc<dyn EventSink>>,
    ) -> Result<Self> {
        let state = Arc::new(RwLock::new(user_state));
        let ws_client = WsClient::default()
            .authenticate(config.credentials.clone(), config.address)
            .map_err(|error| {
                PolyfillError::internal_simple(format!("建立 Polymarket 用户 WS 失败: {error}"))
            })?;
        let stream_task = tokio::spawn(run_user_stream(
            Arc::clone(&state),
            ws_client.clone(),
            store,
            sink.clone(),
        ))
        .abort_handle();
        let rebootstrap_task = tokio::spawn(run_rebootstrap_loop(
            Arc::clone(&state),
            clob_client,
            ws_client,
            sink,
        ))
        .abort_handle();

        Ok(Self {
            state,
            tasks: Mutex::new(vec![stream_task, rebootstrap_task]),
        })
    }

    pub async fn start(client: &ClobClient<Authenticated<Normal>>) -> Result<Self> {
        Self::start_with_store(client, None, None).await
    }

    pub async fn start_with_store(
        client: &ClobClient<Authenticated<Normal>>,
        store: Option<sqlite::Store>,
        sink: Option<Arc<dyn EventSink>>,
    ) -> Result<Self> {
        let mut user_state = UserState::new(client.credentials().key());
        get_remote_orders(client, &mut user_state).await?;
        get_remote_positions(client.address(), &mut user_state).await?;

        Self::start_inner(
            Config::new(client.credentials().clone(), client.address()),
            user_state,
            client.clone(),
            store,
            sink,
        )
        .await
    }

    pub fn open_orders(&self) -> Result<Vec<Order>> {
        let guard = self
            .state
            .read()
            .map_err(|_| PolyfillError::internal_simple("用户状态读锁已被污染"))?;
        Ok(guard.open_order_snapshot())
    }

    pub fn positions(&self) -> Result<Vec<Position>> {
        let guard = self
            .state
            .read()
            .map_err(|_| PolyfillError::internal_simple("用户状态读锁已被污染"))?;
        Ok(guard.position_snapshot())
    }

    pub fn prune_markets(
        &self,
        market_ids: &HashSet<B256>,
    ) -> Result<Option<(Vec<Order>, Vec<Position>)>> {
        let (open_orders, positions, changed) = {
            let mut guard = self
                .state
                .write()
                .map_err(|_| PolyfillError::internal_simple("用户状态写锁已被污染"))?;
            let (pruned_orders, pruned_positions, pruned_contexts) =
                guard.prune_markets(market_ids);
            let changed = pruned_orders > 0 || pruned_positions > 0 || pruned_contexts > 0;
            (
                guard.open_order_snapshot(),
                guard.position_snapshot(),
                changed,
            )
        };

        if changed {
            Ok(Some((open_orders, positions)))
        } else {
            Ok(None)
        }
    }

    pub fn close(&self) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|poisoned| {
            warn!("用户 WS 任务列表锁已被污染，继续强制关闭");
            poisoned.into_inner()
        });

        for task in tasks.drain(..) {
            task.abort();
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

async fn get_remote_positions(address: Address, state: &mut UserState) -> Result<()> {
    let data = DataClient::default();
    let mut offset = 0i32;
    const PAGE_SIZE: i32 = 500;

    loop {
        let request = DataPositionsRequest::builder()
            .user(address)
            .limit(PAGE_SIZE)
            .map_err(|error| {
                PolyfillError::validation(format!("get remote positions limit 非法: {error}"))
            })?
            .offset(offset)
            .map_err(|error| {
                PolyfillError::validation(format!("get remote positions offset 非法: {error}"))
            })?
            .build();
        let page = data.positions(&request).await.map_err(|error| {
            PolyfillError::internal_simple(format!("查询远端仓位失败 address={address}: {error}"))
        })?;

        if page.is_empty() {
            break;
        }

        for remote in &page {
            state.positions.bootstrap_position(
                remote.condition_id,
                remote.asset,
                Some(remote.outcome.clone()),
                remote.size,
                remote.avg_price,
                remote.realized_pnl,
                FeeRule::crypto(),
            )?;
        }

        if page.len() < PAGE_SIZE as usize {
            break;
        }

        offset += PAGE_SIZE;
    }

    Ok(())
}

async fn get_remote_orders(
    client: &ClobClient<Authenticated<Normal>>,
    state: &mut UserState,
) -> Result<()> {
    let request = OrdersRequest::builder().build();
    let mut next_cursor = None;

    loop {
        let page = client
            .orders(&request, next_cursor.clone())
            .await
            .map_err(|error| {
                PolyfillError::internal_simple(format!("查询远端挂单失败: {error}"))
            })?;

        for order in &page.data {
            state.apply_open_order(order)?;
        }

        let cursor = page.next_cursor.trim();
        if cursor.is_empty() || cursor == TERMINAL_CURSOR {
            break;
        }

        next_cursor = Some(cursor.to_string());
    }

    Ok(())
}
async fn run_user_stream(
    state: Arc<RwLock<UserState>>,
    ws_client: WsClient<Authenticated<Normal>>,
    store: Option<sqlite::Store>,
    sink: Option<Arc<dyn EventSink>>,
) {
    let mut stream = match ws_client.subscribe_user_events(Vec::new()) {
        Ok(stream) => Box::pin(stream),
        Err(error) => {
            warn!("订阅 Polymarket 用户频道失败: {}", error);
            return;
        }
    };

    while let Some(message) = stream.next().await {
        match message {
            Ok(WsMessage::Order(order)) => {
                if let Err(error) =
                    apply_order(&state, &order, store.as_ref(), sink.as_deref()).await
                {
                    warn!("应用用户 OrderMessage 失败: {}", error);
                }
            }
            Ok(WsMessage::Trade(trade)) => {
                if let Err(error) =
                    apply_trade(&state, &trade, store.as_ref(), sink.as_deref()).await
                {
                    warn!("应用用户 TradeMessage 失败: {}", error);
                }
            }
            Ok(_) => {}
            Err(error) => {
                warn!(
                    "Polymarket 用户 WS 流错误: {}",
                    PolyfillError::stream(error.to_string(), StreamErrorKind::ConnectionLost)
                );
            }
        }
    }
}

async fn run_rebootstrap_loop(
    state: Arc<RwLock<UserState>>,
    clob_client: ClobClient<Authenticated<Normal>>,
    ws_client: WsClient<Authenticated<Normal>>,
    sink: Option<Arc<dyn EventSink>>,
) {
    let mut was_connected = matches!(
        ws_client.connection_state(ChannelType::User),
        ConnectionState::Connected { .. }
    );

    loop {
        sleep(REBOOTSTRAP_POLL_INTERVAL).await;

        let is_connected = matches!(
            ws_client.connection_state(ChannelType::User),
            ConnectionState::Connected { .. }
        );

        if let Some(sink) = sink.as_ref() {
            sink.ws_status(if is_connected {
                "connected"
            } else {
                "reconnecting"
            });
        }

        if !was_connected && is_connected {
            if let Err(error) = rebootstrap_state(&state, &clob_client).await {
                warn!("Polymarket 用户状态重连后 re-bootstrap 失败: {}", error);
                if let Some(sink) = sink.as_ref() {
                    sink.error(format!("user stream rebootstrap failed: {error}"));
                }
            } else if let Some(sink) = sink.as_ref() {
                if let Ok(guard) = state.read() {
                    sink.user_state(&guard.open_order_snapshot(), &guard.position_snapshot());
                }
            }
        }

        was_connected = is_connected;
    }
}

async fn rebootstrap_state(
    state: &Arc<RwLock<UserState>>,
    client: &ClobClient<Authenticated<Normal>>,
) -> Result<()> {
    let mut next_state = UserState::new(client.credentials().key());
    get_remote_orders(client, &mut next_state).await?;
    get_remote_positions(client.address(), &mut next_state).await?;

    let mut guard = state
        .write()
        .map_err(|_| PolyfillError::internal_simple("用户状态写锁已被污染"))?;
    *guard = next_state;
    Ok(())
}

async fn apply_order(
    state: &Arc<RwLock<UserState>>,
    order: &polymarket_client_sdk_v2::clob::ws::OrderMessage,
    store: Option<&sqlite::Store>,
    sink: Option<&dyn EventSink>,
) -> Result<()> {
    let (open_orders, positions) = {
        let mut guard = state
            .write()
            .map_err(|_| PolyfillError::internal_simple("用户状态写锁已被污染"))?;
        guard.apply_order_message(order)?;
        (guard.open_order_snapshot(), guard.position_snapshot())
    };
    let event = events::Order::from_order_message(order);

    if let Some(store) = store {
        store.insert_order(&event).await?;
    }
    if let Some(sink) = sink {
        sink.user_state(&open_orders, &positions);
        sink.order(&event);
    }

    Ok(())
}

async fn apply_trade(
    state: &Arc<RwLock<UserState>>,
    trade: &polymarket_client_sdk_v2::clob::ws::TradeMessage,
    store: Option<&sqlite::Store>,
    sink: Option<&dyn EventSink>,
) -> Result<()> {
    let (owner, fill, open_orders, positions) = {
        let mut guard = state
            .write()
            .map_err(|_| PolyfillError::internal_simple("用户状态写锁已被污染"))?;
        let owner = guard.owner;
        let fill = guard.apply_trade_message(trade)?;
        (
            owner,
            fill,
            guard.open_order_snapshot(),
            guard.position_snapshot(),
        )
    };
    let event = fill
        .as_ref()
        .map(|fill| events::Trade::from_trade_message(trade, owner, fill));

    if let (Some(store), Some(event)) = (store, event.as_ref()) {
        store.insert_trade(event).await?;
    }
    if let (Some(sink), Some(event)) = (sink, event.as_ref()) {
        sink.user_state(&open_orders, &positions);
        sink.trade(event);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use polymarket_client_sdk_v2::auth::ApiKey;
    use polymarket_client_sdk_v2::clob::{
        types::{OrderStatusType, Side, TraderSide},
        ws::{
            OrderMessage, TradeMessage,
            types::response::{MakerOrder, TradeMessageStatus},
        },
    };
    use polymarket_client_sdk_v2::types::{B256, U256, b256};
    use rust_decimal::Decimal;

    fn market() -> B256 {
        b256!("0000000000000000000000000000000000000000000000000000000000000001")
    }

    fn asset() -> U256 {
        U256::from(1_u64)
    }

    fn user_state() -> UserState {
        UserState::new(ApiKey::nil())
    }

    fn placement_order(side: Side) -> OrderMessage {
        OrderMessage::builder()
            .id("order-1".to_string())
            .market(market())
            .asset_id(asset())
            .side(side)
            .price(Decimal::new(36, 2))
            .original_size(Decimal::new(5, 0))
            .size_matched(Decimal::ZERO)
            .status(OrderStatusType::Live)
            .msg_type(polymarket_client_sdk_v2::clob::ws::types::response::OrderMessageType::Placement)
            .owner(ApiKey::nil())
            .outcome("Up".to_string())
            .build()
    }

    #[test]
    fn test_trade_message_updates_positions() {
        let mut state = user_state();

        let msg = TradeMessage::builder()
            .id("trade-1".to_string())
            .market(market())
            .asset_id(asset())
            .side(Side::Buy)
            .size(Decimal::new(10, 0))
            .fee_rate_bps(Decimal::new(25, 0))
            .price(Decimal::new(5, 1))
            .status(TradeMessageStatus::Matched)
            .trader_side(TraderSide::Taker)
            .maker_orders(Vec::new())
            .build();

        state
            .apply_trade_message(&msg)
            .expect("trade update should work");

        let position = state
            .positions
            .get(&asset())
            .expect("position should exist");
        assert!(position.size > Decimal::ZERO);
        assert!(position.buy_fee_usdc > Decimal::ZERO);
    }

    #[test]
    fn test_invalid_trade_does_not_partially_mutate_state() {
        let mut state = user_state();
        let msg = TradeMessage::builder()
            .id("trade-2".to_string())
            .market(market())
            .asset_id(asset())
            .side(Side::Buy)
            .size(Decimal::new(10, 0))
            .price(Decimal::new(5, 1))
            .status(TradeMessageStatus::Matched)
            .maker_orders(Vec::new())
            .build();

        state
            .apply_trade_message(&msg)
            .expect_err("missing trader_side should fail");

        assert!(state.open_orders.all().next().is_none());
        assert!(state.positions.all().next().is_none());
    }

    #[test]
    fn test_prune_markets_removes_orders_positions_and_contexts() {
        let mut state = user_state();
        let closed_market = market();
        let open_market = b256!("0000000000000000000000000000000000000000000000000000000000000002");
        let closed_asset = asset();
        let open_asset = U256::from(2_u64);

        state
            .apply_order_message(&placement_order(Side::Buy))
            .expect("closed market order should apply");
        state.order_contexts.insert(
            "other-closed-order".to_string(),
            OrderContext {
                market_id: closed_market,
                asset_id: closed_asset,
                side: Side::Sell,
                outcome: Some("Up".to_string()),
            },
        );
        state.order_contexts.insert(
            "open-market-order".to_string(),
            OrderContext {
                market_id: open_market,
                asset_id: open_asset,
                side: Side::Buy,
                outcome: Some("Down".to_string()),
            },
        );
        state
            .positions
            .bootstrap_position(
                closed_market,
                closed_asset,
                Some("Up".to_string()),
                Decimal::new(1, 0),
                Decimal::new(5, 1),
                Decimal::ZERO,
                FeeRule::crypto(),
            )
            .unwrap();
        state
            .positions
            .bootstrap_position(
                open_market,
                open_asset,
                Some("Down".to_string()),
                Decimal::new(2, 0),
                Decimal::new(4, 1),
                Decimal::ZERO,
                FeeRule::crypto(),
            )
            .unwrap();

        let mut closed_markets = HashSet::new();
        closed_markets.insert(closed_market);
        let (pruned_orders, pruned_positions, pruned_contexts) =
            state.prune_markets(&closed_markets);

        assert_eq!(pruned_orders, 1);
        assert_eq!(pruned_positions, 1);
        assert_eq!(pruned_contexts, 2);
        assert!(state.open_orders.get("order-1").is_none());
        assert!(state.positions.get(&closed_asset).is_none());
        assert!(state.positions.get(&open_asset).is_some());
        assert!(!state.order_contexts.contains_key("order-1"));
        assert!(!state.order_contexts.contains_key("other-closed-order"));
        assert!(state.order_contexts.contains_key("open-market-order"));
    }

    #[test]
    fn test_maker_trade_advances_open_order_only() {
        let mut state = user_state();
        state
            .apply_order_message(&placement_order(Side::Buy))
            .unwrap();

        let msg = TradeMessage::builder()
            .id("trade-3".to_string())
            .market(market())
            .asset_id(asset())
            .side(polymarket_client_sdk_v2::clob::types::Side::Buy)
            .size(Decimal::new(5, 0))
            .fee_rate_bps(Decimal::new(25, 0))
            .price(Decimal::new(36, 2))
            .status(TradeMessageStatus::Matched)
            .trader_side(TraderSide::Maker)
            .maker_orders(vec![
                MakerOrder::builder()
                    .asset_id(asset())
                    .matched_amount(Decimal::new(5, 0))
                    .order_id("order-1".to_string())
                    .outcome("Up".to_string())
                    .owner(ApiKey::nil())
                    .price(Decimal::new(36, 2))
                    .build(),
            ])
            .build();

        state.apply_trade_message(&msg).unwrap();
        let position = state.positions.get(&asset()).unwrap();
        assert!(position.size > Decimal::ZERO);
        assert!(state.open_orders.get("order-1").is_none());
    }

    #[test]
    fn test_maker_sell_trade_reduces_position_using_order_context() {
        let mut state = user_state();
        state
            .apply_order_message(&placement_order(Side::Sell))
            .unwrap();

        state
            .positions
            .bootstrap_position(
                market(),
                asset(),
                Some("Up".to_string()),
                Decimal::new(5, 0),
                Decimal::new(3, 1),
                Decimal::ZERO,
                FeeRule::crypto(),
            )
            .unwrap();

        let msg = TradeMessage::builder()
            .id("trade-4".to_string())
            .market(market())
            .asset_id(asset())
            .side(Side::Buy)
            .size(Decimal::new(5, 0))
            .fee_rate_bps(Decimal::new(25, 0))
            .price(Decimal::new(36, 2))
            .status(TradeMessageStatus::Matched)
            .trader_side(TraderSide::Maker)
            .maker_orders(vec![
                MakerOrder::builder()
                    .asset_id(asset())
                    .matched_amount(Decimal::new(5, 0))
                    .order_id("order-1".to_string())
                    .outcome("Up".to_string())
                    .owner(ApiKey::nil())
                    .price(Decimal::new(36, 2))
                    .build(),
            ])
            .build();

        state.apply_trade_message(&msg).unwrap();

        let position = state.positions.get(&asset()).unwrap();
        assert_eq!(position.size, Decimal::ZERO);
        assert!(position.realized_pnl > Decimal::ZERO);
        assert!(state.open_orders.get("order-1").is_none());
    }

    #[test]
    fn test_duplicate_maker_trade_is_idempotent_for_open_orders() {
        let mut state = user_state();
        state
            .apply_order_message(&placement_order(Side::Buy))
            .unwrap();

        let msg = TradeMessage::builder()
            .id("trade-5".to_string())
            .market(market())
            .asset_id(asset())
            .side(Side::Buy)
            .size(Decimal::new(2, 0))
            .fee_rate_bps(Decimal::new(25, 0))
            .price(Decimal::new(55, 2))
            .status(TradeMessageStatus::Matched)
            .trader_side(TraderSide::Maker)
            .maker_orders(vec![
                MakerOrder::builder()
                    .asset_id(asset())
                    .matched_amount(Decimal::new(2, 0))
                    .order_id("order-1".to_string())
                    .outcome("Up".to_string())
                    .owner(ApiKey::nil())
                    .price(Decimal::new(55, 2))
                    .build(),
            ])
            .build();

        state.apply_trade_message(&msg).unwrap();
        state.apply_trade_message(&msg).unwrap();

        let position = state.positions.get(&asset()).unwrap();
        assert!(position.size > Decimal::ZERO);
        let order = state.open_orders.get("order-1").unwrap();
        assert_eq!(order.size_matched, Decimal::new(2, 0));
        assert_eq!(order.trade_ids.len(), 1);
    }

    #[test]
    fn test_maker_trade_without_context_fails() {
        let mut state = user_state();
        let msg = TradeMessage::builder()
            .id("trade-6".to_string())
            .market(market())
            .asset_id(asset())
            .side(Side::Buy)
            .size(Decimal::new(2, 0))
            .fee_rate_bps(Decimal::new(25, 0))
            .price(Decimal::new(55, 2))
            .status(TradeMessageStatus::Matched)
            .trader_side(TraderSide::Maker)
            .maker_orders(vec![
                MakerOrder::builder()
                    .asset_id(asset())
                    .matched_amount(Decimal::new(2, 0))
                    .order_id("missing-order".to_string())
                    .outcome("Up".to_string())
                    .owner(ApiKey::nil())
                    .price(Decimal::new(55, 2))
                    .build(),
            ])
            .build();

        state
            .apply_trade_message(&msg)
            .expect_err("missing context must fail");
        assert!(state.positions.get(&asset()).is_none());
    }
}
