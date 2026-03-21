//! Polymarket 订单簿订阅与本地缓存。
//!
//! 这个模块只做一件事：
//! 通过官方 Rust SDK 订阅多个 asset 的订单簿快照，并在内存里持续维护一份
//! 本地可读的二元市场订单簿。
//!
//! 当前设计尽量贴近你现有代码：
//! - 对外只暴露一个 `Client`
//! - 内部直接复用 `types::orderbook::OrderBooks`
//! - 订阅源使用官方 `clob::ws::Client::subscribe_orderbook`
//!   和 `clob::ws::Client::subscribe_prices`
//! - 存储按你现有的二元镜像订单簿模型维护
//!
//! 一个重要前提：
//! SDK 的市场 WS 是按单个 `asset_id` 推送订单簿，
//! 但你本地的 `OrderBooks` 是二元市场模型。
//! 所以这里采用一个更干净的约定：
//! - 每个二元市场只订阅一个 anchor asset（这里固定为 `up_asset_id`）
//! - 本地只维护这一份 canonical book
//! - `down_asset_id` 永远通过镜像视图读取

use crate::errors::{PolyfillError, Result};
use crate::polymarket::types::orderbook::{BinaryOrderBook, Level, OrderBooks};
use futures::StreamExt;
use polymarket_client_sdk::clob::ws::{BookUpdate, Client as WsClient, PriceChange};
use polymarket_client_sdk::types::U256;
use rust_decimal::Decimal;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tracing::{info, warn};

const COMMAND_BUFFER: usize = 64;

/// Polymarket 订单簿订阅客户端。
///
/// `connect()` 之后会启动一个后台任务：
/// - 等待外部通过 `subscribe()` 动态订阅
/// - 收到新快照后直接替换本地簿
/// - 市场集合变化时重建本地过滤流
///
/// SDK 已经负责市场 WS 的自动重连和自动重订阅，这里只维护本地订阅集合。
pub struct Client {
    books: Arc<RwLock<OrderBooks>>,
    tx: mpsc::Sender<Command>,
    task: Mutex<Option<AbortHandle>>,
}

impl Client {
    /// 启动后台订阅任务，并以空市场集合开始。
    pub async fn connect() -> Result<Self> {
        let books = Arc::new(RwLock::new(OrderBooks::new()));
        let (tx, rx) = mpsc::channel(COMMAND_BUFFER);

        let task = tokio::spawn(run_subscription_loop(Arc::clone(&books), rx)).abort_handle();

        info!("已启动 Polymarket 订单簿订阅，等待动态订阅市场");

        Ok(Self {
            books,
            tx,
            task: Mutex::new(Some(task)),
        })
    }

    /// 当前已注册的二元市场数量。
    pub fn len(&self) -> usize {
        self.books
            .read()
            .map(|guard| guard.len())
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 直接读取 best bid。
    pub fn best_bid(&self, asset_id: &U256) -> Option<Level> {
        self.books.read().ok()?.best_bid(asset_id)
    }

    /// 直接读取 best ask。
    pub fn best_ask(&self, asset_id: &U256) -> Option<Level> {
        self.books.read().ok()?.best_ask(asset_id)
    }

    /// 直接读取中间价。
    pub fn mid(&self, asset_id: &U256) -> Option<Decimal> {
        self.books.read().ok()?.mid(asset_id)
    }

    /// 直接读取价差。
    pub fn spread(&self, asset_id: &U256) -> Option<Decimal> {
        self.books.read().ok()?.spread(asset_id)
    }

    /// 读取前 `depth` 档买盘。
    pub fn bids(&self, asset_id: &U256, depth: usize) -> Option<Vec<Level>> {
        Some(self.books.read().ok()?.get(asset_id)?.bids(depth))
    }

    /// 读取前 `depth` 档卖盘。
    pub fn asks(&self, asset_id: &U256, depth: usize) -> Option<Vec<Level>> {
        Some(self.books.read().ok()?.get(asset_id)?.asks(depth))
    }

    pub async fn subscribe(&self, markets: Vec<[U256; 2]>) -> Result<()> {
        self.tx
            .send(Command::Subscribe(markets))
            .await
            .map_err(|_| PolyfillError::internal_simple("Polymarket 订阅任务已关闭"))
    }

    pub async fn unsubscribe(&self, asset_ids: Vec<U256>) -> Result<()> {
        self.tx
            .send(Command::Unsubscribe(asset_ids))
            .await
            .map_err(|_| PolyfillError::internal_simple("Polymarket 订阅任务已关闭"))
    }

    /// 关闭后台订阅任务。
    pub fn close(&self) {
        self.close_inner();
    }

    fn close_inner(&self) {
        let mut task = self.task.lock().unwrap_or_else(|poisoned| {
            warn!("Polymarket 订单簿任务锁已被污染，继续强制关闭");
            poisoned.into_inner()
        });

        if let Some(task) = task.take() {
            let _ = self.tx.try_send(Command::Shutdown);
            task.abort();
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close_inner();
    }
}

async fn run_subscription_loop(
    books: Arc<RwLock<OrderBooks>>,
    mut commands: mpsc::Receiver<Command>,
) {
    // 这里不再手写外层 reconnect loop。
    // SDK 会自动重连 market channel，并按已跟踪的 asset 集合自动重订阅；
    // 本层只在 subscribe/unsubscribe 改变过滤集合时重建本地 stream 视图。
    let client = WsClient::default();
    let mut desired_asset_ids = HashSet::new();
    let mut current_asset_ids = Vec::new();
    let mut book_stream = None;
    let mut price_stream = None;

    loop {
        if current_asset_ids.is_empty() {
            match commands.recv().await {
                Some(command) => match apply_command(command, &books, &mut desired_asset_ids) {
                    Ok(ControlFlow::Continue) => continue,
                    Ok(ControlFlow::Rebuild) => {
                        match rebuild_streams(
                            &client,
                            &mut current_asset_ids,
                            &mut desired_asset_ids,
                            &mut book_stream,
                            &mut price_stream,
                        ) {
                            Ok(()) => continue,
                            Err(error) => {
                                warn!("重建 Polymarket 订单簿订阅失败: {}", error);
                                current_asset_ids.clear();
                                book_stream = None;
                                price_stream = None;
                                continue;
                            }
                        }
                    }
                    Ok(ControlFlow::Shutdown) | Err(_) => break,
                },
                None => break,
            }
        }

        tokio::select! {
            biased;
            command = commands.recv() => {
                match command {
                    Some(command) => {
                        match apply_command(command, &books, &mut desired_asset_ids) {
                            Ok(ControlFlow::Continue) => {}
                            Ok(ControlFlow::Rebuild) => {
                                if let Err(error) = rebuild_streams(
                                    &client,
                                    &mut current_asset_ids,
                                    &mut desired_asset_ids,
                                    &mut book_stream,
                                    &mut price_stream,
                                ) {
                                    warn!("重建 Polymarket 订单簿订阅失败: {}", error);
                                    current_asset_ids.clear();
                                    book_stream = None;
                                    price_stream = None;
                                }
                            }
                            Ok(ControlFlow::Shutdown) | Err(_) => {
                                unsubscribe_all(&client, &current_asset_ids);
                                return;
                            }
                        }
                    }
                    None => {
                        unsubscribe_all(&client, &current_asset_ids);
                        return;
                    }
                }
            },
            message = next_book_message(&mut book_stream), if book_stream.is_some() => match message {
                Some(Ok(update)) => {
                    if let Err(error) = apply_book_update(&books, update) {
                        warn!("应用 Polymarket BookUpdate 失败: {}", error);
                    }
                }
                Some(Err(error)) => {
                    warn!("Polymarket 订单簿流错误: {}", error);
                }
                None => {
                    warn!("Polymarket 订单簿流已结束");
                    book_stream = None;
                }
            },
            message = next_price_message(&mut price_stream), if price_stream.is_some() => match message {
                Some(Ok(change)) => {
                    if let Err(error) = apply_price_change(&books, change) {
                        warn!("应用 Polymarket PriceChange 失败: {}", error);
                    }
                }
                Some(Err(error)) => {
                    warn!("Polymarket PriceChange 流错误: {}", error);
                }
                None => {
                    warn!("Polymarket PriceChange 流已结束");
                    price_stream = None;
                }
            },
        }
    }
}

fn rebuild_streams(
    client: &WsClient,
    current_asset_ids: &mut Vec<U256>,
    desired_asset_ids: &mut HashSet<U256>,
    book_stream: &mut Option<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = polymarket_client_sdk::Result<BookUpdate>> + Send>,
        >,
    >,
    price_stream: &mut Option<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = polymarket_client_sdk::Result<PriceChange>> + Send>,
        >,
    >,
) -> Result<()> {
    unsubscribe_all(client, current_asset_ids);
    *current_asset_ids = desired_asset_ids.iter().copied().collect();

    if current_asset_ids.is_empty() {
        *book_stream = None;
        *price_stream = None;
        return Ok(());
    }

    let next_book_stream = client
        .subscribe_orderbook(current_asset_ids.clone())
        .map_err(|error| {
            PolyfillError::internal_simple(format!("创建 Polymarket 订单簿订阅失败: {error}"))
        })?
        .boxed();
    let next_price_stream = client
        .subscribe_prices(current_asset_ids.clone())
        .map_err(|error| {
            PolyfillError::internal_simple(format!("创建 Polymarket PriceChange 订阅失败: {error}"))
        })?
        .boxed();

    info!(
        "Polymarket 订单簿与 PriceChange 订阅已建立: asset_ids={}",
        current_asset_ids.len()
    );

    *book_stream = Some(next_book_stream);
    *price_stream = Some(next_price_stream);
    Ok(())
}

async fn next_book_message(
    stream: &mut Option<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = polymarket_client_sdk::Result<BookUpdate>> + Send>,
        >,
    >,
) -> Option<polymarket_client_sdk::Result<BookUpdate>> {
    match stream.as_mut() {
        Some(stream) => stream.next().await,
        None => None,
    }
}

async fn next_price_message(
    stream: &mut Option<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = polymarket_client_sdk::Result<PriceChange>> + Send>,
        >,
    >,
) -> Option<polymarket_client_sdk::Result<PriceChange>> {
    match stream.as_mut() {
        Some(stream) => stream.next().await,
        None => None,
    }
}

// 把 SDK 的整本快照翻译成 `OrderBooks::replace(...)`。
fn apply_book_update(books: &Arc<RwLock<OrderBooks>>, update: BookUpdate) -> Result<()> {
    let BookUpdate {
        asset_id,
        bids,
        asks,
        ..
    } = update;

    let mut books_guard = books
        .write()
        .map_err(|_| PolyfillError::internal_simple("Polymarket 订单簿写锁已被污染"))?;
    books_guard.replace_from_iters(
        &asset_id,
        bids.into_iter()
            .map(|level| Level::new(level.price, level.size)),
        asks.into_iter()
            .map(|level| Level::new(level.price, level.size)),
    )?;

    Ok(())
}

// PriceChange 主路径先尝试 2-entry 镜像 pair。
// 这是当前真实 WS 样本里的主流形状；未命中时发 warning 并退回逐条应用，
// 让协议漂移变成可观测事件，而不是为冷路径常驻通用 dedupe 成本。
fn apply_price_change(books: &Arc<RwLock<OrderBooks>>, change: PriceChange) -> Result<()> {
    let PriceChange { price_changes, .. } = change;

    let mut books_guard = books
        .write()
        .map_err(|_| PolyfillError::internal_simple("Polymarket 订单簿写锁已被污染"))?;
    if try_apply_pair_fast_path(&mut books_guard, &price_changes)? {
        return Ok(());
    }

    warn!(
        entries = price_changes.len(),
        "Polymarket PriceChange 未命中 2-entry 镜像 fast path，退回逐条应用"
    );
    for entry in price_changes {
        let size = match entry.size {
            Some(size) => size,
            None => continue,
        };
        books_guard.set_level(&entry.asset_id, entry.side, entry.price, size)?;
    }

    Ok(())
}

fn try_apply_pair_fast_path(
    books: &mut OrderBooks,
    price_changes: &[polymarket_client_sdk::clob::ws::types::response::PriceChangeBatchEntry],
) -> Result<bool> {
    let [first, second] = price_changes else {
        return Ok(false);
    };

    let Some(first_size) = first.size else {
        return Ok(false);
    };
    let Some(second_size) = second.size else {
        return Ok(false);
    };

    let first_update =
        books.normalize_level(&first.asset_id, first.side, first.price, first_size)?;
    let second_update =
        books.normalize_level(&second.asset_id, second.side, second.price, second_size)?;

    // fast path 只接受两个 entry 归一化后完全相同的情况。
    // 这样 up/down 镜像对会被压成一次 canonical 写入，而非镜像批次不会被误折叠。
    if first_update != second_update {
        return Ok(false);
    }

    books.apply_canonical(
        &first_update.asset_id,
        first_update.side,
        first_update.price,
        first_update.size,
    )?;

    Ok(true)
}

fn unsubscribe_all(client: &WsClient, asset_ids: &[U256]) {
    let _ = client.unsubscribe_orderbook(asset_ids);
    let _ = client.unsubscribe_prices(asset_ids);
}

#[cfg(test)]
fn build_books(markets: Vec<[U256; 2]>) -> Result<(Arc<RwLock<OrderBooks>>, Vec<U256>)> {
    if markets.is_empty() {
        return Err(PolyfillError::validation(
            "Polymarket 订单簿订阅至少需要一个二元市场",
        ));
    }

    let mut orderbooks = OrderBooks::new();
    let mut asset_ids = Vec::with_capacity(markets.len());
    let mut seen_asset_ids = HashSet::with_capacity(markets.len());

    for [up_asset_id, down_asset_id] in markets {
        if up_asset_id == down_asset_id {
            return Err(PolyfillError::validation(
                "二元市场的两个 asset_id 不能相同",
            ));
        }

        orderbooks.insert(BinaryOrderBook::new(up_asset_id, down_asset_id)?)?;

        if seen_asset_ids.insert(up_asset_id) {
            asset_ids.push(up_asset_id);
        }
    }

    Ok((Arc::new(RwLock::new(orderbooks)), asset_ids))
}

enum Command {
    Subscribe(Vec<[U256; 2]>),
    Unsubscribe(Vec<U256>),
    Shutdown,
}

enum ControlFlow {
    Continue,
    Rebuild,
    Shutdown,
}

fn apply_command(
    command: Command,
    books: &Arc<RwLock<OrderBooks>>,
    desired_asset_ids: &mut HashSet<U256>,
) -> Result<ControlFlow> {
    // 命令结果不仅表示是否成功，还要告诉外层订阅循环是否必须重建连接。
    // subscribe/unsubscribe 改变了 asset 集合，因此由外层统一决定何时 rebuild。
    match command {
        Command::Subscribe(markets) => {
            let changed = register_markets(books, desired_asset_ids, markets)?;
            Ok(if changed {
                ControlFlow::Rebuild
            } else {
                ControlFlow::Continue
            })
        }
        Command::Unsubscribe(asset_ids) => {
            let changed = remove_markets(books, desired_asset_ids, asset_ids)?;
            Ok(if changed {
                ControlFlow::Rebuild
            } else {
                ControlFlow::Continue
            })
        }
        Command::Shutdown => Ok(ControlFlow::Shutdown),
    }
}

fn register_markets(
    books: &Arc<RwLock<OrderBooks>>,
    desired_asset_ids: &mut HashSet<U256>,
    markets: Vec<[U256; 2]>,
) -> Result<bool> {
    let mut books_guard = books
        .write()
        .map_err(|_| PolyfillError::internal_simple("Polymarket 订单簿写锁已被污染"))?;
    let mut changed = false;

    for [up_asset_id, down_asset_id] in markets {
        if up_asset_id == down_asset_id {
            return Err(PolyfillError::validation(
                "二元市场的两个 asset_id 不能相同",
            ));
        }

        match books_guard.get(&up_asset_id) {
            Some(view) => {
                if *view.other_asset_id() != down_asset_id {
                    return Err(PolyfillError::validation(format!(
                        "asset_id {} 已绑定到其他二元市场",
                        up_asset_id
                    )));
                }
            }
            None => {
                if books_guard.get(&down_asset_id).is_some() {
                    return Err(PolyfillError::validation(format!(
                        "asset_id {} 已绑定到其他二元市场",
                        down_asset_id
                    )));
                }
                books_guard.insert(BinaryOrderBook::new(up_asset_id, down_asset_id)?)?;
                changed = true;
            }
        }

        if desired_asset_ids.insert(up_asset_id) {
            changed = true;
        }
    }

    Ok(changed)
}

fn remove_markets(
    books: &Arc<RwLock<OrderBooks>>,
    desired_asset_ids: &mut HashSet<U256>,
    asset_ids: Vec<U256>,
) -> Result<bool> {
    let mut books_guard = books
        .write()
        .map_err(|_| PolyfillError::internal_simple("Polymarket 订单簿写锁已被污染"))?;
    let mut changed = false;

    for asset_id in asset_ids {
        if let Some([up_asset_id, _down_asset_id]) = books_guard.remove(&asset_id) {
            desired_asset_ids.remove(&up_asset_id);
            changed = true;
        }
    }

    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polymarket::types::orderbook::Side;
    use polymarket_client_sdk::clob::ws::BookUpdate;
    use polymarket_client_sdk::clob::ws::types::response::OrderBookLevel;

    fn asset(id: u64) -> U256 {
        U256::from(id)
    }

    #[test]
    fn test_build_books_registers_binary_markets() {
        let markets = vec![[asset(1), asset(2)], [asset(3), asset(4)]];

        let (books, asset_ids) = build_books(markets).unwrap();
        let books = books.read().unwrap();

        assert_eq!(asset_ids.len(), 2);
        assert_eq!(books.len(), 2);
    }

    #[test]
    fn test_handle_book_update_replaces_local_orderbook() {
        let (books, ..) = build_books(vec![[asset(1), asset(2)]]).unwrap();

        let update = BookUpdate::builder()
            .asset_id(asset(1))
            .market([9; 32].into())
            .timestamp(123)
            .bids(vec![
                OrderBookLevel::builder()
                    .price(Decimal::new(44, 2))
                    .size(Decimal::new(100, 0))
                    .build(),
            ])
            .asks(vec![
                OrderBookLevel::builder()
                    .price(Decimal::new(56, 2))
                    .size(Decimal::new(120, 0))
                    .build(),
            ])
            .build();

        apply_book_update(&books, update).unwrap();

        let view = books.read().unwrap();
        assert_eq!(
            view.best_bid(&asset(1)),
            Some(Level::new(Decimal::new(44, 2), Decimal::new(100, 0)))
        );
        assert_eq!(
            view.best_ask(&asset(1)),
            Some(Level::new(Decimal::new(56, 2), Decimal::new(120, 0)))
        );
    }

    #[test]
    fn test_other_asset_uses_mirror_view() {
        let (books, ..) = build_books(vec![[asset(1), asset(2)]]).unwrap();

        let update = BookUpdate::builder()
            .asset_id(asset(1))
            .market([8; 32].into())
            .timestamp(456)
            .bids(vec![
                OrderBookLevel::builder()
                    .price(Decimal::new(40, 2))
                    .size(Decimal::new(50, 0))
                    .build(),
            ])
            .asks(vec![
                OrderBookLevel::builder()
                    .price(Decimal::new(60, 2))
                    .size(Decimal::new(70, 0))
                    .build(),
            ])
            .build();

        apply_book_update(&books, update).unwrap();

        assert_eq!(
            books.read().unwrap().best_bid(&asset(2)),
            Some(Level::new(Decimal::new(40, 2), Decimal::new(70, 0)))
        );
        assert_eq!(
            books.read().unwrap().best_ask(&asset(2)),
            Some(Level::new(Decimal::new(60, 2), Decimal::new(50, 0)))
        );
        assert_eq!(
            books.read().unwrap().mid(&asset(2)),
            Some(Decimal::new(50, 2))
        );
    }

    #[test]
    fn test_handle_price_change_updates_single_level() {
        let (books, ..) = build_books(vec![[asset(1), asset(2)]]).unwrap();

        let update = BookUpdate::builder()
            .asset_id(asset(1))
            .market([7; 32].into())
            .timestamp(100)
            .bids(vec![
                OrderBookLevel::builder()
                    .price(Decimal::new(44, 2))
                    .size(Decimal::new(100, 0))
                    .build(),
            ])
            .asks(vec![
                OrderBookLevel::builder()
                    .price(Decimal::new(56, 2))
                    .size(Decimal::new(120, 0))
                    .build(),
            ])
            .build();
        apply_book_update(&books, update).unwrap();

        let change = PriceChange::builder()
            .market([7; 32].into())
            .timestamp(101)
            .price_changes(vec![
                polymarket_client_sdk::clob::ws::types::response::PriceChangeBatchEntry::builder()
                    .asset_id(asset(1))
                    .price(Decimal::new(45, 2))
                    .size(Decimal::new(90, 0))
                    .side(polymarket_client_sdk::clob::types::Side::Buy)
                    .hash("hash-2".to_string())
                    .build(),
            ])
            .build();

        apply_price_change(&books, change).unwrap();

        let view = books.read().unwrap();
        assert_eq!(
            view.best_bid(&asset(1)),
            Some(Level::new(Decimal::new(45, 2), Decimal::new(90, 0)))
        );
        assert_eq!(
            view.best_ask(&asset(1)),
            Some(Level::new(Decimal::new(56, 2), Decimal::new(120, 0)))
        );
    }

    #[test]
    fn test_handle_price_change_without_size_keeps_existing_book() {
        let (books, ..) = build_books(vec![[asset(1), asset(2)]]).unwrap();

        let update = BookUpdate::builder()
            .asset_id(asset(1))
            .market([7; 32].into())
            .timestamp(100)
            .bids(vec![
                OrderBookLevel::builder()
                    .price(Decimal::new(44, 2))
                    .size(Decimal::new(100, 0))
                    .build(),
            ])
            .asks(vec![
                OrderBookLevel::builder()
                    .price(Decimal::new(56, 2))
                    .size(Decimal::new(120, 0))
                    .build(),
            ])
            .build();
        apply_book_update(&books, update).unwrap();

        let change = PriceChange::builder()
            .market([7; 32].into())
            .timestamp(102)
            .price_changes(vec![
                polymarket_client_sdk::clob::ws::types::response::PriceChangeBatchEntry::builder()
                    .asset_id(asset(1))
                    .price(Decimal::new(45, 2))
                    .side(polymarket_client_sdk::clob::types::Side::Buy)
                    .best_bid(Decimal::new(45, 2))
                    .best_ask(Decimal::new(56, 2))
                    .hash("hash-3".to_string())
                    .build(),
            ])
            .build();

        apply_price_change(&books, change).unwrap();

        let view = books.read().unwrap();
        assert_eq!(
            view.best_bid(&asset(1)),
            Some(Level::new(Decimal::new(44, 2), Decimal::new(100, 0)))
        );
        assert_eq!(
            view.best_ask(&asset(1)),
            Some(Level::new(Decimal::new(56, 2), Decimal::new(120, 0)))
        );
    }

    #[test]
    fn test_handle_price_change_dedupes_up_and_down_in_same_batch() {
        let (books, ..) = build_books(vec![[asset(1), asset(2)]]).unwrap();

        let change = PriceChange::builder()
            .market([7; 32].into())
            .timestamp(101)
            .price_changes(vec![
                polymarket_client_sdk::clob::ws::types::response::PriceChangeBatchEntry::builder()
                    .asset_id(asset(1))
                    .price(Decimal::new(45, 2))
                    .size(Decimal::new(90, 0))
                    .side(Side::Buy)
                    .build(),
                polymarket_client_sdk::clob::ws::types::response::PriceChangeBatchEntry::builder()
                    .asset_id(asset(2))
                    .price(Decimal::new(55, 2))
                    .size(Decimal::new(90, 0))
                    .side(Side::Sell)
                    .build(),
            ])
            .build();

        apply_price_change(&books, change).unwrap();

        let view = books.read().unwrap();
        assert_eq!(
            view.best_bid(&asset(1)),
            Some(Level::new(Decimal::new(45, 2), Decimal::new(90, 0)))
        );
        assert_eq!(
            view.best_ask(&asset(2)),
            Some(Level::new(Decimal::new(55, 2), Decimal::new(90, 0)))
        );
    }

    #[test]
    fn test_handle_price_change_non_fast_path_batch_applies_entries_sequentially() {
        let (books, ..) = build_books(vec![[asset(1), asset(2)]]).unwrap();

        let update = BookUpdate::builder()
            .asset_id(asset(1))
            .market([7; 32].into())
            .timestamp(100)
            .bids(vec![
                OrderBookLevel::builder()
                    .price(Decimal::new(44, 2))
                    .size(Decimal::new(100, 0))
                    .build(),
            ])
            .asks(vec![
                OrderBookLevel::builder()
                    .price(Decimal::new(56, 2))
                    .size(Decimal::new(120, 0))
                    .build(),
            ])
            .build();
        apply_book_update(&books, update).unwrap();

        let change = PriceChange::builder()
            .market([7; 32].into())
            .timestamp(101)
            .price_changes(vec![
                polymarket_client_sdk::clob::ws::types::response::PriceChangeBatchEntry::builder()
                    .asset_id(asset(1))
                    .price(Decimal::new(45, 2))
                    .size(Decimal::new(90, 0))
                    .side(Side::Buy)
                    .build(),
                polymarket_client_sdk::clob::ws::types::response::PriceChangeBatchEntry::builder()
                    .asset_id(asset(1))
                    .price(Decimal::new(55, 2))
                    .size(Decimal::new(80, 0))
                    .side(Side::Sell)
                    .build(),
            ])
            .build();

        apply_price_change(&books, change).unwrap();

        let view = books.read().unwrap();
        assert_eq!(
            view.best_bid(&asset(1)),
            Some(Level::new(Decimal::new(45, 2), Decimal::new(90, 0)))
        );
        assert_eq!(
            view.best_ask(&asset(1)),
            Some(Level::new(Decimal::new(55, 2), Decimal::new(80, 0)))
        );
    }

    #[test]
    fn test_handle_price_change_falls_back_when_normalized_sizes_conflict() {
        let (books, ..) = build_books(vec![[asset(1), asset(2)]]).unwrap();

        let change = PriceChange::builder()
            .market([7; 32].into())
            .timestamp(101)
            .price_changes(vec![
                polymarket_client_sdk::clob::ws::types::response::PriceChangeBatchEntry::builder()
                    .asset_id(asset(1))
                    .price(Decimal::new(45, 2))
                    .size(Decimal::new(70, 0))
                    .side(Side::Buy)
                    .build(),
                polymarket_client_sdk::clob::ws::types::response::PriceChangeBatchEntry::builder()
                    .asset_id(asset(2))
                    .price(Decimal::new(55, 2))
                    .size(Decimal::new(90, 0))
                    .side(Side::Sell)
                    .build(),
            ])
            .build();

        apply_price_change(&books, change).unwrap();

        let view = books.read().unwrap();
        assert_eq!(
            view.best_bid(&asset(1)),
            Some(Level::new(Decimal::new(45, 2), Decimal::new(90, 0)))
        );
    }

    #[test]
    fn test_client_style_reads_can_return_depth_levels() {
        let (books, ..) = build_books(vec![[asset(1), asset(2)]]).unwrap();

        let update = BookUpdate::builder()
            .asset_id(asset(1))
            .market([7; 32].into())
            .timestamp(100)
            .bids(vec![
                OrderBookLevel::builder()
                    .price(Decimal::new(44, 2))
                    .size(Decimal::new(100, 0))
                    .build(),
                OrderBookLevel::builder()
                    .price(Decimal::new(43, 2))
                    .size(Decimal::new(90, 0))
                    .build(),
            ])
            .asks(vec![
                OrderBookLevel::builder()
                    .price(Decimal::new(56, 2))
                    .size(Decimal::new(120, 0))
                    .build(),
            ])
            .build();

        apply_book_update(&books, update).unwrap();

        let view = books.read().unwrap();
        assert_eq!(
            view.get(&asset(1)).unwrap().bids(2),
            vec![
                Level::new(Decimal::new(44, 2), Decimal::new(100, 0)),
                Level::new(Decimal::new(43, 2), Decimal::new(90, 0))
            ]
        );
        assert_eq!(
            view.get(&asset(1)).unwrap().asks(1),
            vec![Level::new(Decimal::new(56, 2), Decimal::new(120, 0))]
        );
    }
}
