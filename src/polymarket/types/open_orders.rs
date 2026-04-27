//! Polymarket 本地 open orders 状态模型。
//!
//! 这里维护的是本地 canonical `Order`，不是直接把 REST `OpenOrderResponse`
//! 当成本地状态。原因很简单：
//! - REST `OpenOrderResponse` 是完整快照；
//! - WS `OrderMessage` 是增量补丁；
//! - 本地状态只维护当前活跃挂单。

use crate::errors::{PolyfillError, Result};
use polymarket_client_sdk_v2::auth::ApiKey;
use polymarket_client_sdk_v2::clob::ws::types::response::OrderMessageType;
use polymarket_client_sdk_v2::clob::{
    types::response::OpenOrderResponse,
    types::{OrderStatusType, Side},
    ws::{OrderMessage, TradeMessage},
};
use polymarket_client_sdk_v2::types::{B256, U256};
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub struct Order {
    pub id: String,
    pub market_id: B256,
    pub asset_id: U256,
    pub side: Side,
    pub price: Decimal,
    pub original_size: Decimal,
    pub size_matched: Decimal,
    pub status: OrderStatusType,
    pub outcome: Option<String>,
    pub trade_ids: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct OpenOrders {
    open_orders: HashMap<String, Order>,
}

impl OpenOrders {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, order_id: &str) -> Option<&Order> {
        self.open_orders.get(order_id)
    }

    pub fn all(&self) -> impl Iterator<Item = &Order> {
        self.open_orders.values()
    }

    pub fn apply_open_order(&mut self, order: &OpenOrderResponse) -> Result<()> {
        if order.id.trim().is_empty() {
            return Err(PolyfillError::validation("OpenOrderResponse.id 不能为空"));
        }

        let order = Order::from_open_order_response(order);
        if is_active_order_status(&order.status) {
            self.open_orders.insert(order.id.clone(), order);
        }
        Ok(())
    }

    pub fn apply_order_message(&mut self, msg: &OrderMessage) -> Result<()> {
        if msg.id.trim().is_empty() {
            return Err(PolyfillError::validation("OrderMessage.id 不能为空"));
        }

        // `PLACEMENT` 是 open order 的创建真值；后续 `UPDATE/CANCELLATION`
        // 只是在这张活跃挂单上做校准。这里不保留终态订单，`open_orders`
        // 语义始终保持为“当前仍活跃的挂单视图”。
        let order_exists = self.open_orders.contains_key(&msg.id);
        let mut order = match msg.msg_type.as_ref() {
            Some(OrderMessageType::Placement) => Order::from_order_message(msg),
            _ if order_exists => self
                .open_orders
                .remove(&msg.id)
                .expect("checked contains_key"),
            _ => Order::from_order_message(msg),
        };

        order.market_id = msg.market;
        order.asset_id = msg.asset_id;
        order.side = msg.side;
        order.price = msg.price;

        if let Some(original_size) = msg.original_size {
            order.original_size = original_size;
        }

        if let Some(size_matched) = msg.size_matched {
            order.size_matched = size_matched;
        }

        if let Some(outcome) = &msg.outcome {
            order.outcome = Some(outcome.clone());
        }

        if let Some(trade_ids) = &msg.associate_trades {
            merge_trade_ids(&mut order.trade_ids, trade_ids.iter().cloned());
        }

        order.status =
            derive_open_order_status(msg.status.as_ref(), order.original_size, order.size_matched);

        if is_active_order_status(&order.status) {
            self.open_orders.insert(order.id.clone(), order);
        }
        Ok(())
    }

    pub fn apply_trade_message(&mut self, msg: &TradeMessage, owner: ApiKey) -> Result<()> {
        if msg.id.trim().is_empty() {
            return Err(PolyfillError::validation("TradeMessage.id 不能为空"));
        }

        // `TradeMessage` 代表真正发生的成交事实。对 open_orders 来说，
        // 它的职责只是把本账户 maker 挂单的 `size_matched` 向前推进。
        // 这里用 `owner(ApiKey)` 过滤本账户 maker order，因为当前 SDK 没有
        // 暴露 `maker_address`，这是在现有字段下最稳定的归属判定。
        for maker_order in msg.maker_orders.iter().filter(|order| order.owner == owner) {
            let Some(order) = self.open_orders.get_mut(&maker_order.order_id) else {
                continue;
            };

            if order.trade_ids.iter().any(|trade_id| trade_id == &msg.id) {
                continue;
            }

            order.trade_ids.push(msg.id.clone());
            order.size_matched += maker_order.matched_amount;
            if order.size_matched >= order.original_size {
                order.size_matched = order.original_size;
                order.status = OrderStatusType::Matched;
            }
        }

        self.open_orders
            .retain(|_, order| is_active_order_status(&order.status));
        Ok(())
    }

    pub fn prune_markets(&mut self, market_ids: &HashSet<B256>) -> usize {
        let before = self.open_orders.len();
        self.open_orders
            .retain(|_, order| !market_ids.contains(&order.market_id));
        before - self.open_orders.len()
    }
}

impl Order {
    fn from_open_order_response(order: &OpenOrderResponse) -> Self {
        Self {
            id: order.id.clone(),
            market_id: order.market,
            asset_id: order.asset_id,
            side: order.side,
            price: order.price,
            original_size: order.original_size,
            size_matched: order.size_matched,
            status: order.status.clone(),
            outcome: if order.outcome.is_empty() {
                None
            } else {
                Some(order.outcome.clone())
            },
            trade_ids: order.associate_trades.clone(),
        }
    }

    fn from_order_message(msg: &OrderMessage) -> Self {
        let original_size = msg.original_size.unwrap_or(Decimal::ZERO);
        let size_matched = msg.size_matched.unwrap_or(Decimal::ZERO);
        Self {
            id: msg.id.clone(),
            market_id: msg.market,
            asset_id: msg.asset_id,
            side: msg.side,
            price: msg.price,
            original_size,
            size_matched,
            status: derive_open_order_status(msg.status.as_ref(), original_size, size_matched),
            outcome: msg.outcome.clone(),
            trade_ids: msg.associate_trades.clone().unwrap_or_default(),
        }
    }
}

fn merge_trade_ids(trade_ids: &mut Vec<String>, incoming: impl IntoIterator<Item = String>) {
    for trade_id in incoming {
        if trade_ids.iter().all(|existing| existing != &trade_id) {
            trade_ids.push(trade_id);
        }
    }
}

fn is_active_order_status(status: &OrderStatusType) -> bool {
    !matches!(status, OrderStatusType::Matched | OrderStatusType::Canceled)
}

fn derive_open_order_status(
    status: Option<&OrderStatusType>,
    original_size: Decimal,
    matched_size: Decimal,
) -> OrderStatusType {
    match status {
        Some(OrderStatusType::Matched) => return OrderStatusType::Matched,
        Some(OrderStatusType::Canceled) => return OrderStatusType::Canceled,
        Some(OrderStatusType::Unmatched) => return OrderStatusType::Unmatched,
        _ => {}
    }

    if original_size > Decimal::ZERO && matched_size >= original_size {
        return OrderStatusType::Matched;
    }

    if matched_size > Decimal::ZERO {
        return OrderStatusType::Live;
    }

    match status.cloned() {
        Some(status) => status,
        None => OrderStatusType::Delayed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use polymarket_client_sdk_v2::auth::ApiKey;
    use polymarket_client_sdk_v2::clob::{
        types::{OrderStatusType, OrderType, Side, TraderSide},
        ws::{
            OrderMessage, TradeMessage,
            types::response::{MakerOrder, TradeMessageStatus},
        },
    };
    use polymarket_client_sdk_v2::types::{Address, B256, U256, b256};

    fn market() -> B256 {
        b256!("0000000000000000000000000000000000000000000000000000000000000001")
    }

    fn asset(value: u64) -> U256 {
        U256::from(value)
    }

    fn open_orders() -> OpenOrders {
        OpenOrders::new()
    }

    fn open_order(order_id: &str) -> OpenOrderResponse {
        let created_at = chrono::Utc::now();
        OpenOrderResponse::builder()
            .id(order_id.to_string())
            .status(OrderStatusType::Live)
            .owner(ApiKey::nil())
            .maker_address(Address::default())
            .market(market())
            .asset_id(asset(1))
            .side(Side::Buy)
            .original_size(Decimal::new(10, 0))
            .size_matched(Decimal::ZERO)
            .price(Decimal::new(55, 2))
            .associate_trades(Vec::new())
            .outcome("UP".to_string())
            .created_at(created_at)
            .expiration(created_at)
            .order_type(OrderType::GTC)
            .build()
    }

    #[test]
    fn test_apply_order_message_tracks_partial_fill() {
        let mut open_orders = open_orders();
        let msg = OrderMessage::builder()
            .id("order-1".to_string())
            .market(market())
            .asset_id(asset(1))
            .side(Side::Buy)
            .price(Decimal::new(55, 2))
            .original_size(Decimal::new(10, 0))
            .size_matched(Decimal::new(4, 0))
            .status(OrderStatusType::Live)
            .build();

        open_orders.apply_order_message(&msg).unwrap();
        let order = open_orders.get("order-1").unwrap();
        assert_eq!(order.status, OrderStatusType::Live);
        assert_eq!(order.size_matched, Decimal::new(4, 0));
    }

    #[test]
    fn test_terminal_order_message_removes_open_order() {
        let mut open_orders = open_orders();
        open_orders
            .apply_open_order(&open_order("order-1"))
            .unwrap();

        let msg = OrderMessage::builder()
            .id("order-1".to_string())
            .market(market())
            .asset_id(asset(1))
            .side(Side::Buy)
            .price(Decimal::new(55, 2))
            .original_size(Decimal::new(10, 0))
            .size_matched(Decimal::new(10, 0))
            .status(OrderStatusType::Matched)
            .build();

        open_orders.apply_order_message(&msg).unwrap();
        assert!(open_orders.get("order-1").is_none());
    }

    #[test]
    fn test_trade_message_advances_partial_fill() {
        let mut open_orders = open_orders();
        open_orders
            .apply_open_order(&open_order("order-1"))
            .unwrap();

        let trade = TradeMessage::builder()
            .id("trade-1".to_string())
            .market(market())
            .asset_id(asset(1))
            .side(Side::Buy)
            .size(Decimal::new(4, 0))
            .price(Decimal::new(55, 2))
            .status(TradeMessageStatus::Matched)
            .trader_side(TraderSide::Maker)
            .maker_orders(vec![
                MakerOrder::builder()
                    .asset_id(asset(1))
                    .matched_amount(Decimal::new(4, 0))
                    .order_id("order-1".to_string())
                    .outcome("UP".to_string())
                    .owner(ApiKey::nil())
                    .price(Decimal::new(55, 2))
                    .build(),
            ])
            .build();

        open_orders
            .apply_trade_message(&trade, ApiKey::nil())
            .unwrap();
        let order = open_orders.get("order-1").unwrap();
        assert_eq!(order.size_matched, Decimal::new(4, 0));
        assert_eq!(order.trade_ids, vec!["trade-1".to_string()]);
    }

    #[test]
    fn test_duplicate_trade_is_idempotent() {
        let mut open_orders = open_orders();
        open_orders
            .apply_open_order(&open_order("order-1"))
            .unwrap();

        let trade = TradeMessage::builder()
            .id("trade-1".to_string())
            .market(market())
            .asset_id(asset(1))
            .side(Side::Buy)
            .size(Decimal::new(4, 0))
            .price(Decimal::new(55, 2))
            .status(TradeMessageStatus::Matched)
            .trader_side(TraderSide::Maker)
            .maker_orders(vec![
                MakerOrder::builder()
                    .asset_id(asset(1))
                    .matched_amount(Decimal::new(4, 0))
                    .order_id("order-1".to_string())
                    .outcome("UP".to_string())
                    .owner(ApiKey::nil())
                    .price(Decimal::new(55, 2))
                    .build(),
            ])
            .build();

        open_orders
            .apply_trade_message(&trade, ApiKey::nil())
            .unwrap();
        open_orders
            .apply_trade_message(&trade, ApiKey::nil())
            .unwrap();

        let order = open_orders.get("order-1").unwrap();
        assert_eq!(order.size_matched, Decimal::new(4, 0));
        assert_eq!(order.trade_ids.len(), 1);
    }

    #[test]
    fn test_trade_message_removes_fully_filled_order() {
        let mut open_orders = open_orders();
        open_orders
            .apply_open_order(&open_order("order-1"))
            .unwrap();

        let trade = TradeMessage::builder()
            .id("trade-1".to_string())
            .market(market())
            .asset_id(asset(1))
            .side(Side::Buy)
            .size(Decimal::new(10, 0))
            .price(Decimal::new(55, 2))
            .status(TradeMessageStatus::Matched)
            .trader_side(TraderSide::Maker)
            .maker_orders(vec![
                MakerOrder::builder()
                    .asset_id(asset(1))
                    .matched_amount(Decimal::new(10, 0))
                    .order_id("order-1".to_string())
                    .outcome("UP".to_string())
                    .owner(ApiKey::nil())
                    .price(Decimal::new(55, 2))
                    .build(),
            ])
            .build();

        open_orders
            .apply_trade_message(&trade, ApiKey::nil())
            .unwrap();
        assert!(open_orders.get("order-1").is_none());
    }
}
