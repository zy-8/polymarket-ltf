//! Binance Spot 市场数据客户端。
//!
//! 当前统一维护：
//! - `bookTicker` 最新盘口；
//! - `kline` 序列缓存；
//! - 单条 `/ws` 连接上的动态订阅与自动重连。
//!
//! 缓存口径：
//! - `bookTicker` 只保留每个 symbol 的最新值；
//! - `kline` 按 `(symbol, interval)` 保留限长序列；
//! - 启动时先订阅，再用 HTTP 回填历史，再与 WS 增量按 `open_time_ms` 合并。

use std::collections::{HashMap, HashSet, VecDeque};
use std::str::FromStr;
use std::sync::{Arc, Mutex, RwLock};

use crate::errors::{PolyfillError, Result, StreamErrorKind};
use crate::types::crypto::{Interval, Symbol};
use crate::types::market::Candle;
use futures::{SinkExt, StreamExt};
use reqwest::Url;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::{Instant, sleep, sleep_until};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

/// Binance Spot WebSocket 服务地址。
pub const BINANCE_WS_BASE_URL: &str = "wss://stream.binance.com:9443";

/// Binance Spot HTTP 服务地址。
pub const BINANCE_HTTP_BASE_URL: &str = "https://api.binance.com";

/// Binance 单连接允许的最大 stream 数量。
pub const BINANCE_MAX_STREAMS_PER_CONNECTION: usize = 1024;

/// Binance 原始 WebSocket 入口。
const BINANCE_WS_PATH: &str = "ws";

/// Binance 连接接近 24 小时上限时，提前主动轮换。
const BINANCE_ROTATE_AFTER: std::time::Duration = std::time::Duration::from_secs(23 * 60 * 60);

/// 第一次重连前的等待时间。
const RECONNECT_BASE_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

/// 重连退避的最大等待时间。
const RECONNECT_MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(30);

/// 每次失败后等待时间放大的倍数。
const RECONNECT_MULTIPLIER: u32 = 2;

/// 计算中间价时使用的常量 2。
const DECIMAL_TWO: u32 = 2;

/// 启动时最少回填的历史 K 线根数。
const MIN_KLINE_SEED_BARS: usize = 256;

/// 每个 `(symbol, interval)` 的本地 K 线缓存上限。
const KLINE_CACHE_CAP: usize = 256;
/// 后台 HTTP K 线刷新在收盘后额外等待的保护时间。
const BACKGROUND_REFRESH_GRACE: std::time::Duration = std::time::Duration::from_secs(3);
/// 后台 HTTP K 线刷新失败后的重试间隔。
const BACKGROUND_REFRESH_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

type SharedBooks = Arc<RwLock<HashMap<String, BookTicker>>>;
type SharedSeries = Arc<RwLock<HashMap<(Symbol, Interval), CandleSeries>>>;
type SharedSubscriptions = Arc<RwLock<HashSet<Subscription>>>;
type SharedBackgroundSeries = Arc<RwLock<HashMap<(Symbol, BackgroundInterval), CandleSeries>>>;

/// Binance `bookTicker` 的本地结构。
#[derive(Debug, Clone, PartialEq)]
pub struct BookTicker {
    /// 订单簿更新 ID。
    pub update_id: u64,
    /// 交易对名称，内部统一保存为小写，例如 `btcusdt`。
    pub symbol: String,
    /// 买一价。
    pub bid: Decimal,
    /// 买一量。
    pub bid_qty: Decimal,
    /// 卖一价。
    pub ask: Decimal,
    /// 卖一量。
    pub ask_qty: Decimal,
}

impl BookTicker {
    /// 返回当前盘口中间价。
    pub fn mid(&self) -> Decimal {
        (self.bid + self.ask) / Decimal::from(DECIMAL_TWO)
    }
}

/// 只通过后台 HTTP 刷新维护的附加 K 线周期。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackgroundInterval {
    H1,
    H4,
}

impl BackgroundInterval {
    pub fn as_slug(self) -> &'static str {
        match self {
            Self::H1 => "1h",
            Self::H4 => "4h",
        }
    }

    fn step_secs(self) -> i64 {
        match self {
            Self::H1 => 60 * 60,
            Self::H4 => 4 * 60 * 60,
        }
    }
}

/// 单个 `(symbol, interval)` 的本地 candle 序列。
#[derive(Debug, Clone)]
struct CandleSeries {
    candles: VecDeque<Candle>,
    max_len: usize,
}

impl CandleSeries {
    fn new(max_len: usize) -> Self {
        Self {
            candles: VecDeque::with_capacity(max_len.min(32)),
            max_len,
        }
    }

    fn upsert(&mut self, candle: Candle) {
        match self.candles.back_mut() {
            Some(last) if last.open_time_ms == candle.open_time_ms => {
                *last = candle;
            }
            _ => {
                self.candles.push_back(candle);
                while self.candles.len() > self.max_len {
                    self.candles.pop_front();
                }
            }
        }
    }

    fn latest(&self) -> Option<Candle> {
        self.candles.back().cloned()
    }

    fn tail(&self, limit: usize) -> Vec<Candle> {
        let take = if limit == 0 {
            self.candles.len()
        } else {
            limit.min(self.candles.len())
        };

        self.candles
            .iter()
            .skip(self.candles.len().saturating_sub(take))
            .cloned()
            .collect()
    }
}

/// Binance 市场数据客户端。
pub struct Client {
    http: reqwest::Client,
    books: SharedBooks,
    series: SharedSeries,
    background_series: SharedBackgroundSeries,
    subscriptions: SharedSubscriptions,
    command_tx: mpsc::UnboundedSender<Command>,
    task: Mutex<Option<AbortHandle>>,
    background_tasks: Mutex<Vec<AbortHandle>>,
}

impl Client {
    /// 建立客户端并启动后台连接循环。
    pub async fn connect() -> Result<Self> {
        let http = reqwest::Client::new();
        let books = Arc::new(RwLock::new(HashMap::new()));
        let series = Arc::new(RwLock::new(HashMap::new()));
        let background_series = Arc::new(RwLock::new(HashMap::new()));
        let subscriptions = Arc::new(RwLock::new(HashSet::new()));
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        let task = tokio::spawn(run_loop(
            Arc::clone(&books),
            Arc::clone(&series),
            Arc::clone(&subscriptions),
            command_rx,
        ))
        .abort_handle();

        info!("已启动 Binance WebSocket 客户端");

        Ok(Self {
            http,
            books,
            series,
            background_series,
            subscriptions,
            command_tx,
            task: Mutex::new(Some(task)),
            background_tasks: Mutex::new(Vec::new()),
        })
    }

    /// 当前底层连接数量。
    pub fn connection_count(&self) -> usize {
        1
    }

    /// 订阅一组 `bookTicker`。
    pub fn subscribe_books(&self, symbols: &[Symbol]) -> Result<()> {
        let subscriptions = symbols
            .iter()
            .copied()
            .map(Subscription::Book)
            .collect::<Vec<_>>();
        self.add_subscriptions(&subscriptions)
    }

    /// 取消订阅一组 `bookTicker`。
    ///
    /// 同时会清理对应 symbol 的本地盘口缓存，避免继续读取到陈旧数据。
    pub fn unsubscribe_books(&self, symbols: &[Symbol]) -> Result<()> {
        let subscriptions = symbols
            .iter()
            .copied()
            .map(Subscription::Book)
            .collect::<Vec<_>>();
        self.remove_subscriptions(&subscriptions)?;

        let keys = symbols
            .iter()
            .map(|symbol| symbol.as_binance_symbol().to_string())
            .collect::<Vec<_>>();
        let mut guard = self
            .books
            .write()
            .map_err(|_| PolyfillError::internal_simple("Binance 价格缓存写锁已被污染"))?;
        for key in keys {
            guard.remove(&key);
        }
        Ok(())
    }

    /// 订阅一组 `kline`。
    ///
    /// 每个订阅项由 `(symbol, interval)` 唯一确定。
    ///
    /// 当前实现会先注册 WS 增量订阅，
    /// 再通过 HTTP 回填最近一段历史 K 线。
    ///
    /// 这样做的原因是：
    /// - 先订阅可以尽早接住新的增量更新；
    /// - 再回填历史时，HTTP 和 WS 数据可以按 `open_time_ms` 合并去重；
    /// - 比“先拉 HTTP，再订阅”更不容易在启动边界漏掉最新一段更新。
    pub async fn subscribe_klines(&self, items: &[(Symbol, Interval)]) -> Result<()> {
        let subscriptions = items
            .iter()
            .copied()
            .map(|(symbol, interval)| Subscription::Kline(symbol, interval))
            .collect::<Vec<_>>();
        self.add_subscriptions(&subscriptions)?;
        self.seed_klines(items, MIN_KLINE_SEED_BARS).await
    }

    /// 取消订阅一组 `kline`。
    ///
    /// 同时会清理对应序列缓存，避免策略继续读取到已停更的 candle。
    pub fn unsubscribe_klines(&self, items: &[(Symbol, Interval)]) -> Result<()> {
        let subscriptions = items
            .iter()
            .copied()
            .map(|(symbol, interval)| Subscription::Kline(symbol, interval))
            .collect::<Vec<_>>();
        self.remove_subscriptions(&subscriptions)?;

        let mut guard = self
            .series
            .write()
            .map_err(|_| PolyfillError::internal_simple("Binance kline 缓存写锁已被污染"))?;
        for item in items {
            guard.remove(item);
        }
        Ok(())
    }

    /// 返回当前订阅总数。
    pub fn subscription_count(&self) -> usize {
        self.subscriptions
            .read()
            .map(|guard| guard.len())
            .unwrap_or_default()
    }

    /// 读取某个 symbol 的最新盘口。
    pub fn get(&self, symbol: &str) -> Option<BookTicker> {
        get_book(&self.books, symbol)
    }

    /// 读取某个 symbol 的最新中间价。
    pub fn price(&self, symbol: &str) -> Option<Decimal> {
        self.get(symbol).map(|book| book.mid())
    }

    /// 读取全部 symbol 的最新盘口快照。
    pub fn all(&self) -> HashMap<String, BookTicker> {
        self.books
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// 当前已经收到多少个 symbol 的有效盘口。
    pub fn len(&self) -> usize {
        self.books.read().map(|guard| guard.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 返回最近一根 candle。
    pub fn latest_candle(&self, symbol: Symbol, interval: Interval) -> Option<Candle> {
        self.series
            .read()
            .ok()?
            .get(&(symbol, interval))
            .and_then(CandleSeries::latest)
    }

    /// 返回最近一段 candles。
    pub fn candles(&self, symbol: Symbol, interval: Interval, limit: usize) -> Vec<Candle> {
        self.series
            .read()
            .ok()
            .and_then(|guard| {
                guard
                    .get(&(symbol, interval))
                    .map(|series| series.tail(limit))
            })
            .unwrap_or_default()
    }

    /// 返回最近一段后台刷新维护的 candles。
    pub fn cached_background(
        &self,
        symbol: Symbol,
        interval: BackgroundInterval,
        limit: usize,
    ) -> Vec<Candle> {
        self.background_series
            .read()
            .ok()
            .and_then(|guard| {
                guard
                    .get(&(symbol, interval))
                    .map(|series| series.tail(limit))
            })
            .unwrap_or_default()
    }

    /// 关闭后台任务。
    pub fn close(&self) {
        self.close_inner();
    }

    /// 通过 HTTP 回填一组 `(symbol, interval)` 的历史 K 线。
    pub async fn seed_klines(&self, items: &[(Symbol, Interval)], limit: usize) -> Result<()> {
        if items.is_empty() || limit == 0 {
            return Ok(());
        }

        for &(symbol, interval) in items {
            let candles = fetch_klines(&self.http, symbol, interval, limit).await?;
            self.merge_klines(symbol, interval, candles)?;
        }

        Ok(())
    }

    /// 以后台任务方式维护一组 `1h/4h` HTTP K 线缓存。
    ///
    /// 当前策略热路径只读这组缓存，不再在扫描时直接发 HTTP。
    pub async fn start_background_cache(
        &self,
        items: &[(Symbol, BackgroundInterval)],
    ) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        for &(symbol, interval) in items {
            let candles =
                fetch_klines_by_slug(&self.http, symbol, interval.as_slug(), KLINE_CACHE_CAP)
                    .await?;
            merge_series(&self.background_series, (symbol, interval), candles)?;
        }

        let mut background_tasks = self
            .background_tasks
            .lock()
            .map_err(|_| PolyfillError::internal_simple("Binance 后台刷新任务句柄锁已被污染"))?;
        for &(symbol, interval) in items {
            let http = self.http.clone();
            let background_series = Arc::clone(&self.background_series);
            background_tasks.push(
                tokio::spawn(run_background_loop(
                    http,
                    background_series,
                    symbol,
                    interval,
                    KLINE_CACHE_CAP,
                ))
                .abort_handle(),
            );
        }

        Ok(())
    }

    /// 把一组订阅项并入目标订阅集合，并通知后台连接执行同步。
    fn add_subscriptions(&self, new_items: &[Subscription]) -> Result<()> {
        if new_items.is_empty() {
            return Ok(());
        }

        let mut guard = self
            .subscriptions
            .write()
            .map_err(|_| PolyfillError::internal_simple("Binance 订阅集合写锁已被污染"))?;

        for item in new_items {
            guard.insert(*item);
        }

        validate_subscription_count(guard.len())?;
        drop(guard);

        self.command_tx
            .send(Command::Sync)
            .map_err(|_| PolyfillError::internal_simple("Binance 订阅任务已关闭"))
    }

    /// 从目标订阅集合移除一组订阅项，并通知后台连接执行同步。
    fn remove_subscriptions(&self, old_items: &[Subscription]) -> Result<()> {
        if old_items.is_empty() {
            return Ok(());
        }

        let mut guard = self
            .subscriptions
            .write()
            .map_err(|_| PolyfillError::internal_simple("Binance 订阅集合写锁已被污染"))?;

        for item in old_items {
            guard.remove(item);
        }
        drop(guard);

        self.command_tx
            .send(Command::Sync)
            .map_err(|_| PolyfillError::internal_simple("Binance 订阅任务已关闭"))
    }

    /// 关闭本地后台任务句柄。
    fn close_inner(&self) {
        let mut task = self.task.lock().unwrap_or_else(|poisoned| {
            warn!("Binance 后台任务句柄锁已被污染，继续强制关闭");
            poisoned.into_inner()
        });
        let mut background_tasks = self.background_tasks.lock().unwrap_or_else(|poisoned| {
            warn!("Binance 后台刷新任务句柄锁已被污染，继续强制关闭");
            poisoned.into_inner()
        });

        let _ = self.command_tx.send(Command::Close);

        if let Some(task) = task.take() {
            task.abort();
        }

        for task in background_tasks.drain(..) {
            task.abort();
        }
    }

    /// 把一段历史 candles 合并进本地缓存。
    ///
    /// 这个入口同时服务：
    /// - 启动期 HTTP 历史回填；
    /// - 未来可能的缺口回补。
    fn merge_klines(&self, symbol: Symbol, interval: Interval, candles: Vec<Candle>) -> Result<()> {
        if candles.is_empty() {
            return Ok(());
        }

        let mut guard = self
            .series
            .write()
            .map_err(|_| PolyfillError::internal_simple("Binance kline 缓存写锁已被污染"))?;

        let series = guard
            .entry((symbol, interval))
            .or_insert_with(|| CandleSeries::new(KLINE_CACHE_CAP));

        for candle in candles {
            series.upsert(candle);
        }

        Ok(())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close_inner();
    }
}

/// 内部订阅项。
///
/// 这是连接层内部的 canonical 表达：
/// - `Book(Symbol)` 对应一个 `@bookTicker`
/// - `Kline(Symbol, Interval)` 对应一个 `@kline_*`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Subscription {
    Book(Symbol),
    Kline(Symbol, Interval),
}

impl Subscription {
    /// 转成 Binance stream 名。
    fn stream_name(self) -> String {
        match self {
            Self::Book(symbol) => format!("{}@bookTicker", symbol.as_binance_symbol()),
            Self::Kline(symbol, interval) => {
                format!(
                    "{}@kline_{}",
                    symbol.as_binance_symbol(),
                    interval.as_slug()
                )
            }
        }
    }
}

/// 后台连接任务的内部控制消息。
#[derive(Debug)]
enum Command {
    /// 让后台连接按当前目标订阅集合重新同步。
    Sync,
    /// 终止后台连接循环。
    Close,
}

/// Binance 后台连接主循环。
///
/// 这个循环的职责只有两个：
/// - 保持与 Binance 的连接；
/// - 在连接失败后按退避策略重连。
async fn run_loop(
    books: SharedBooks,
    series: SharedSeries,
    subscriptions: SharedSubscriptions,
    mut command_rx: mpsc::UnboundedReceiver<Command>,
) {
    let mut delay = RECONNECT_BASE_DELAY;
    // 当前 client 固定使用一条 `/ws` 连接。
    let url = format!("{}/{}", BINANCE_WS_BASE_URL, BINANCE_WS_PATH);

    loop {
        match tokio_tungstenite::connect_async(&url).await {
            Ok((stream, _)) => {
                info!("Binance WebSocket 连接成功: {}", url);
                delay = RECONNECT_BASE_DELAY;

                if let Err(error) =
                    run_socket(stream, &books, &series, &subscriptions, &mut command_rx).await
                {
                    warn!("Binance WebSocket 连接中断: {}", error);
                }
            }
            Err(error) => {
                warn!("Binance WebSocket 连接失败: {}", error);
            }
        }

        sleep(delay).await;
        delay = next_delay(delay);
    }
}

/// 单条 Binance WS 连接的消息循环。
///
/// 进入这个循环后，会先把当前目标订阅集合下发到服务端，
/// 然后同时处理三类事件：
/// - 主动轮换；
/// - 本地订阅变更；
/// - 远端消息输入。
async fn run_socket(
    mut stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    books: &SharedBooks,
    series: &SharedSeries,
    subscriptions: &SharedSubscriptions,
    command_rx: &mut mpsc::UnboundedReceiver<Command>,
) -> Result<()> {
    let rotate_at = Instant::now() + BINANCE_ROTATE_AFTER;
    let mut active = HashSet::new();
    let mut request_id = 1_u64;

    // 每次重连后，都需要重新以当前目标集合为准做一次同步。
    sync_subs(&mut stream, subscriptions, &mut active, &mut request_id).await?;

    loop {
        tokio::select! {
            _ = sleep_until(rotate_at) => {
                info!("Binance WebSocket 主动轮换连接，避免触发 24 小时上限");
                return Ok(());
            }
            command = command_rx.recv() => {
                match command {
                    Some(Command::Sync) => {
                        sync_subs(&mut stream, subscriptions, &mut active, &mut request_id).await?;
                    }
                    Some(Command::Close) | None => return Ok(()),
                }
            }
            message = stream.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        if let Err(error) = handle_message(&text, books, series) {
                            warn!("Binance 消息处理失败: {}", error);
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        stream.send(Message::Pong(payload)).await.map_err(|error| {
                            PolyfillError::stream(
                                format!("响应 Binance ping 失败: {}", error),
                                StreamErrorKind::ConnectionLost,
                            )
                        })?;
                    }
                    Some(Ok(Message::Pong(_))) => continue,
                    Some(Ok(Message::Binary(_))) => continue,
                    Some(Ok(Message::Close(frame))) => {
                        info!("Binance WebSocket 被服务端关闭: {:?}", frame);
                        return Err(PolyfillError::stream(
                            "Binance WebSocket 连接已关闭",
                            StreamErrorKind::ConnectionLost,
                        ));
                    }
                    Some(Ok(Message::Frame(_))) => continue,
                    Some(Err(error)) => {
                        return Err(PolyfillError::stream(
                            format!("Binance WebSocket 错误: {}", error),
                            StreamErrorKind::ConnectionLost,
                        ));
                    }
                    None => {
                        return Err(PolyfillError::stream(
                            "Binance WebSocket 流已结束",
                            StreamErrorKind::ConnectionLost,
                        ));
                    }
                }
            }
        }
    }
}

/// 让当前连接的“实际订阅集合”追平“目标订阅集合”。
///
/// `desired` 来自 `Client` 共享状态，
/// `active` 是当前这条连接已经成功下发过的本地视图。
async fn sync_subs(
    stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    subscriptions: &SharedSubscriptions,
    active: &mut HashSet<Subscription>,
    request_id: &mut u64,
) -> Result<()> {
    let desired = subscriptions
        .read()
        .map_err(|_| PolyfillError::internal_simple("Binance 订阅集合读锁已被污染"))?
        .clone();

    validate_subscription_count(desired.len())?;

    let to_unsubscribe = active.difference(&desired).copied().collect::<Vec<_>>();
    let to_subscribe = desired.difference(active).copied().collect::<Vec<_>>();

    // 先移除多余项，再增加缺失项，保持状态单向收敛。
    if !to_unsubscribe.is_empty() {
        send_cmd(stream, "UNSUBSCRIBE", &to_unsubscribe, request_id).await?;
        for item in to_unsubscribe {
            active.remove(&item);
        }
    }

    if !to_subscribe.is_empty() {
        send_cmd(stream, "SUBSCRIBE", &to_subscribe, request_id).await?;
        for item in to_subscribe {
            active.insert(item);
        }
    }

    Ok(())
}

/// 发送一条 Binance 控制消息。
///
/// 当前仅用于：
/// - `SUBSCRIBE`
/// - `UNSUBSCRIBE`
async fn send_cmd(
    stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    method: &'static str,
    subscriptions: &[Subscription],
    request_id: &mut u64,
) -> Result<()> {
    if subscriptions.is_empty() {
        return Ok(());
    }

    // 为了让控制消息在测试和日志里保持稳定顺序，这里固定排序。
    let mut params = subscriptions
        .iter()
        .map(|subscription| subscription.stream_name())
        .collect::<Vec<_>>();
    params.sort_unstable();

    let request = Cmd {
        method,
        params,
        id: *request_id,
    };
    *request_id = request_id.saturating_add(1);

    let payload = serde_json::to_string(&request).map_err(|error| {
        PolyfillError::parse(
            format!("序列化 Binance 控制消息失败: {}", error),
            Some(Box::new(error)),
        )
    })?;

    debug!(
        method,
        stream_count = request.params.len(),
        "发送 Binance 订阅控制消息"
    );

    stream
        .send(Message::Text(payload.into()))
        .await
        .map_err(|error| {
            PolyfillError::stream(
                format!("发送 Binance 控制消息失败: {}", error),
                StreamErrorKind::ConnectionLost,
            )
        })
}

/// 校验当前订阅总数没有超出 Binance 单连接上限。
fn validate_subscription_count(count: usize) -> Result<()> {
    if count > BINANCE_MAX_STREAMS_PER_CONNECTION {
        return Err(PolyfillError::validation(format!(
            "Binance 订阅数超出上限: {} > {}",
            count, BINANCE_MAX_STREAMS_PER_CONNECTION
        )));
    }

    Ok(())
}

/// 处理一条 Binance 文本消息。
///
/// 当前消息分两类：
/// - 控制响应 / 控制错误；
/// - 市场数据事件。
fn handle_message(text: &str, books: &SharedBooks, series: &SharedSeries) -> Result<()> {
    if text.contains("\"result\"") || text.contains("\"code\"") {
        handle_control(text)?;
        return Ok(());
    }

    match parse_event(text)? {
        Event::Book(book) => update_book(books, book),
        Event::Kline(update) => update_kline(series, update),
    }
}

/// 处理 Binance 控制消息。
///
/// 当前对控制消息的策略是“轻处理”：
/// - 成功响应只记 debug；
/// - 错误响应记 warn；
/// - 不维护更复杂的 pending request 状态机。
fn handle_control(text: &str) -> Result<()> {
    if text.contains("\"code\"") {
        let error: CmdError = serde_json::from_str(text).map_err(|parse_error| {
            PolyfillError::parse(
                format!("解析 Binance 控制错误失败: {}", parse_error),
                Some(Box::new(parse_error)),
            )
        })?;

        warn!(
            id = error.id,
            code = error.code,
            message = %error.msg,
            "Binance 控制消息返回错误"
        );
        return Ok(());
    }

    let ack: CmdAck = serde_json::from_str(text).map_err(|error| {
        PolyfillError::parse(
            format!("解析 Binance 控制响应失败: {}", error),
            Some(Box::new(error)),
        )
    })?;

    debug!(id = ack.id, "收到 Binance 控制响应");
    Ok(())
}

/// 用最新 `bookTicker` 更新本地盘口缓存。
fn update_book(books: &SharedBooks, book: BookTicker) -> Result<()> {
    let mut guard = books
        .write()
        .map_err(|_| PolyfillError::internal_simple("Binance 价格缓存写锁已被污染"))?;

    debug!("更新 Binance 最新盘口: {}", book.symbol);
    guard.insert(book.symbol.clone(), book);
    Ok(())
}

/// 用最新 `kline` 更新本地 candle 缓存。
fn update_kline(series: &SharedSeries, update: KlineEvent) -> Result<()> {
    upsert_candle(series, (update.symbol, update.interval), update.candle)
}

/// 从本地盘口缓存读取某个 symbol 的最新值。
fn get_book(books: &SharedBooks, symbol: &str) -> Option<BookTicker> {
    let symbol = norm_symbol(symbol).ok()?;
    books.read().ok()?.get(&symbol).cloned()
}

/// 统一解析 Binance 事件消息。
///
/// 当前连接使用 `/ws` + 动态订阅，
/// 但仍兼容 combined stream 形状，便于后续排查和测试。
fn parse_event(text: &str) -> Result<Event> {
    if text.contains("\"k\":") {
        return parse_kline(text);
    }

    parse_book(text)
}

/// 解析 `bookTicker` 消息。
///
/// 同时兼容：
/// - raw `/ws` 事件；
/// - combined stream 包装事件。
fn parse_book(text: &str) -> Result<Event> {
    if let Ok(wrapper) = serde_json::from_str::<StreamBook>(text) {
        return Ok(Event::Book(wrapper.data.into_book()));
    }

    let payload: RawBook = serde_json::from_str(text).map_err(|error| {
        PolyfillError::parse(
            format!("解析 Binance bookTicker payload 失败: {}", error),
            Some(Box::new(error)),
        )
    })?;

    Ok(Event::Book(payload.into_book()))
}

/// 解析 `kline` 消息。
///
/// 同时兼容：
/// - raw `/ws` 事件；
/// - combined stream 包装事件。
fn parse_kline(text: &str) -> Result<Event> {
    if let Ok(wrapper) = serde_json::from_str::<StreamKline>(text) {
        return Ok(Event::Kline(wrapper.data.try_into_kline()?));
    }

    let payload: RawKlineMsg = serde_json::from_str(text).map_err(|error| {
        PolyfillError::parse(
            format!("解析 Binance kline payload 失败: {}", error),
            Some(Box::new(error)),
        )
    })?;

    Ok(Event::Kline(payload.try_into_kline()?))
}

/// 归一化 symbol 文本，保证缓存键统一为小写 Binance 现货 symbol。
fn norm_symbol(symbol: &str) -> Result<String> {
    let symbol = symbol.trim().to_ascii_lowercase();
    if symbol.is_empty() {
        return Err(PolyfillError::validation("Binance symbol 不能为空"));
    }

    Ok(symbol)
}

/// 把 Binance symbol 解析成项目内部 `Symbol`。
///
/// 当前只支持项目主链路里已经定义的四个币种。
fn parse_asset(symbol: &str) -> Result<Symbol> {
    match symbol.trim().to_ascii_lowercase().as_str() {
        "btcusdt" => Ok(Symbol::Btc),
        "ethusdt" => Ok(Symbol::Eth),
        "solusdt" => Ok(Symbol::Sol),
        "xrpusdt" => Ok(Symbol::Xrp),
        _ => Err(PolyfillError::validation(format!(
            "unsupported Binance symbol: {symbol}"
        ))),
    }
}

/// 计算下一次重连等待时间。
fn next_delay(current_delay: std::time::Duration) -> std::time::Duration {
    let next_millis = current_delay
        .as_millis()
        .saturating_mul(RECONNECT_MULTIPLIER as u128)
        .min(RECONNECT_MAX_DELAY.as_millis());

    std::time::Duration::from_millis(next_millis as u64)
}

/// 解析 Binance `kline` 字段里的字符串数值。
fn de_str_f64<'de, D>(deserializer: D) -> std::result::Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    value.parse::<f64>().map_err(serde::de::Error::custom)
}

/// 通过 Binance Spot HTTP `klines` 接口拉取历史 K 线。
///
/// 这是启动期回填的标准入口：
/// - HTTP 负责补历史；
/// - WS 负责接增量。
async fn fetch_klines(
    http: &reqwest::Client,
    symbol: Symbol,
    interval: Interval,
    limit: usize,
) -> Result<Vec<Candle>> {
    fetch_klines_by_slug(http, symbol, interval.as_slug(), limit).await
}

async fn fetch_klines_by_slug(
    http: &reqwest::Client,
    symbol: Symbol,
    interval: &str,
    limit: usize,
) -> Result<Vec<Candle>> {
    let symbol = symbol.as_binance_symbol().to_ascii_uppercase();
    let limit = limit.min(1000).to_string();
    let mut url =
        Url::parse(&format!("{}/api/v3/klines", BINANCE_HTTP_BASE_URL)).map_err(|error| {
            PolyfillError::internal_simple(format!("构造 Binance klines URL 失败: {error}"))
        })?;

    url.query_pairs_mut()
        .append_pair("symbol", &symbol)
        .append_pair("interval", interval)
        .append_pair("limit", &limit);

    let response = http.get(url).send().await.map_err(|error| {
        PolyfillError::stream(
            format!("请求 Binance klines 失败: {error}"),
            StreamErrorKind::ConnectionLost,
        )
    })?;

    let response = response.error_for_status().map_err(|error| {
        PolyfillError::stream(
            format!("Binance klines HTTP 返回错误: {error}"),
            StreamErrorKind::ConnectionLost,
        )
    })?;

    let rows: Vec<HttpKline> = response.json().await.map_err(|error| {
        PolyfillError::parse(
            format!("解析 Binance klines HTTP 响应失败: {error}"),
            Some(Box::new(error)),
        )
    })?;

    Ok(rows.into_iter().map(HttpKline::into_candle).collect())
}

fn merge_series<K>(
    series: &Arc<RwLock<HashMap<K, CandleSeries>>>,
    key: K,
    candles: Vec<Candle>,
) -> Result<()>
where
    K: Eq + std::hash::Hash,
{
    if candles.is_empty() {
        return Ok(());
    }

    let mut guard = series
        .write()
        .map_err(|_| PolyfillError::internal_simple("Binance kline 缓存写锁已被污染"))?;
    let items = guard
        .entry(key)
        .or_insert_with(|| CandleSeries::new(KLINE_CACHE_CAP));

    for candle in candles {
        items.upsert(candle);
    }

    Ok(())
}

fn upsert_candle<K>(
    series: &Arc<RwLock<HashMap<K, CandleSeries>>>,
    key: K,
    candle: Candle,
) -> Result<()>
where
    K: Eq + std::hash::Hash,
{
    let mut guard = series
        .write()
        .map_err(|_| PolyfillError::internal_simple("Binance kline 缓存写锁已被污染"))?;
    let items = guard
        .entry(key)
        .or_insert_with(|| CandleSeries::new(KLINE_CACHE_CAP));
    items.upsert(candle);
    Ok(())
}

async fn run_background_loop(
    http: reqwest::Client,
    background_series: SharedBackgroundSeries,
    symbol: Symbol,
    interval: BackgroundInterval,
    limit: usize,
) {
    loop {
        match next_refresh_at(interval) {
            Ok(instant) => sleep_until(instant).await,
            Err(error) => {
                warn!(
                    symbol = symbol.as_slug(),
                    interval = interval.as_slug(),
                    error = %error,
                    "计算 Binance 后台周期下次刷新时间失败"
                );
                sleep(BACKGROUND_REFRESH_RETRY_INTERVAL).await;
            }
        }

        loop {
            match fetch_klines_by_slug(&http, symbol, interval.as_slug(), limit).await {
                Ok(candles) => {
                    if let Err(error) =
                        merge_series(&background_series, (symbol, interval), candles)
                    {
                        warn!(
                            symbol = symbol.as_slug(),
                            interval = interval.as_slug(),
                            error = %error,
                            "合并 Binance 后台周期缓存失败"
                        );
                    }
                    break;
                }
                Err(error) => {
                    warn!(
                        symbol = symbol.as_slug(),
                        interval = interval.as_slug(),
                        error = %error,
                        retry_secs = BACKGROUND_REFRESH_RETRY_INTERVAL.as_secs(),
                        "刷新 Binance 后台周期缓存失败"
                    );
                    sleep(BACKGROUND_REFRESH_RETRY_INTERVAL).await;
                }
            }
        }
    }
}

fn next_refresh_at(interval: BackgroundInterval) -> Result<Instant> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| PolyfillError::internal_simple(format!("系统时间错误: {error}")))?;
    let now_secs = i64::try_from(now.as_secs())
        .map_err(|_| PolyfillError::internal_simple("当前时间超出 i64 范围"))?;
    let step_secs = interval.step_secs();
    let next_close_ts = ((now_secs / step_secs) + 1) * step_secs;
    let wait_secs = (next_close_ts - now_secs).max(0) as u64;

    Ok(Instant::now() + std::time::Duration::from_secs(wait_secs) + BACKGROUND_REFRESH_GRACE)
}

/// Binance 控制消息请求体。
#[derive(Debug, Serialize)]
struct Cmd {
    method: &'static str,
    params: Vec<String>,
    id: u64,
}

/// Binance 控制成功响应。
#[derive(Debug, Deserialize)]
struct CmdAck {
    #[allow(dead_code)]
    result: Option<serde_json::Value>,
    id: Option<u64>,
}

/// Binance 控制错误响应。
#[derive(Debug, Deserialize)]
struct CmdError {
    code: i64,
    msg: String,
    id: Option<u64>,
}

/// 当前支持的 Binance 事件类型。
enum Event {
    Book(BookTicker),
    Kline(KlineEvent),
}

/// 解析后的 `kline` 事件。
#[derive(Debug, Clone, PartialEq)]
struct KlineEvent {
    symbol: Symbol,
    interval: Interval,
    candle: Candle,
}

/// combined stream 形状的 `bookTicker` 包装体。
#[derive(Debug, Deserialize)]
struct StreamBook {
    #[allow(dead_code)]
    stream: String,
    data: RawBook,
}

/// combined stream 形状的 `kline` 包装体。
#[derive(Debug, Deserialize)]
struct StreamKline {
    #[allow(dead_code)]
    stream: String,
    data: RawKlineMsg,
}

/// Binance 原始 `bookTicker` payload。
#[derive(Debug, Deserialize)]
struct RawBook {
    #[serde(rename = "u")]
    update_id: u64,
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "b", with = "rust_decimal::serde::str")]
    bid: Decimal,
    #[serde(rename = "B", with = "rust_decimal::serde::str")]
    bid_qty: Decimal,
    #[serde(rename = "a", with = "rust_decimal::serde::str")]
    ask: Decimal,
    #[serde(rename = "A", with = "rust_decimal::serde::str")]
    ask_qty: Decimal,
}

/// Binance 原始 `kline` 事件外层 payload。
#[derive(Debug, Deserialize)]
struct RawKlineMsg {
    #[allow(dead_code)]
    #[serde(rename = "e")]
    event_type: Option<String>,
    #[allow(dead_code)]
    #[serde(rename = "E")]
    event_time_ms: Option<i64>,
    #[allow(dead_code)]
    #[serde(rename = "s")]
    symbol: Option<String>,
    #[serde(rename = "k")]
    kline: RawCandle,
}

/// Binance 原始 `kline` 负载。
#[derive(Debug, Deserialize)]
struct RawCandle {
    #[serde(rename = "t")]
    open_time_ms: i64,
    #[serde(rename = "T")]
    close_time_ms: i64,
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "i")]
    interval: String,
    #[serde(rename = "o", deserialize_with = "de_str_f64")]
    open: f64,
    #[serde(rename = "c", deserialize_with = "de_str_f64")]
    close: f64,
    #[serde(rename = "h", deserialize_with = "de_str_f64")]
    high: f64,
    #[serde(rename = "l", deserialize_with = "de_str_f64")]
    low: f64,
    #[serde(rename = "v", deserialize_with = "de_str_f64")]
    volume: f64,
    #[serde(rename = "x")]
    is_closed: bool,
}

/// Binance HTTP `klines` 返回的一行数组。
///
/// Binance 把 K 线行编码成数组，而不是对象；
/// 这里使用 tuple struct 直接映射对应位置。
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct HttpKline(
    i64,
    #[serde(deserialize_with = "de_str_f64")] f64,
    #[serde(deserialize_with = "de_str_f64")] f64,
    #[serde(deserialize_with = "de_str_f64")] f64,
    #[serde(deserialize_with = "de_str_f64")] f64,
    #[serde(deserialize_with = "de_str_f64")] f64,
    i64,
    serde_json::Value,
    serde_json::Value,
    serde_json::Value,
    serde_json::Value,
    serde_json::Value,
);

impl RawBook {
    /// 转成本地 `BookTicker`。
    fn into_book(self) -> BookTicker {
        BookTicker {
            update_id: self.update_id,
            symbol: self.symbol.to_ascii_lowercase(),
            bid: self.bid,
            bid_qty: self.bid_qty,
            ask: self.ask,
            ask_qty: self.ask_qty,
        }
    }
}

impl RawKlineMsg {
    /// 转成本地解析后的 `kline` 事件。
    fn try_into_kline(self) -> Result<KlineEvent> {
        let symbol = parse_asset(&self.kline.symbol)?;
        let interval = Interval::from_str(&self.kline.interval)?;

        Ok(KlineEvent {
            symbol,
            interval,
            candle: Candle {
                open_time_ms: self.kline.open_time_ms,
                close_time_ms: self.kline.close_time_ms,
                open: self.kline.open,
                high: self.kline.high,
                low: self.kline.low,
                close: self.kline.close,
                volume: self.kline.volume,
                is_closed: self.kline.is_closed,
            },
        })
    }
}

impl HttpKline {
    /// 转成本地 `Candle`。
    fn into_candle(self) -> Candle {
        Candle {
            open_time_ms: self.0,
            open: self.1,
            high: self.2,
            low: self.3,
            close: self.4,
            volume: self.5,
            close_time_ms: self.6,
            is_closed: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subscription_stream_name_for_book() {
        assert_eq!(
            Subscription::Book(Symbol::Btc).stream_name(),
            "btcusdt@bookTicker"
        );
    }

    #[test]
    fn test_subscription_stream_name_for_kline() {
        assert_eq!(
            Subscription::Kline(Symbol::Eth, Interval::M15).stream_name(),
            "ethusdt@kline_15m"
        );
    }

    #[test]
    fn test_validate_subscription_count_rejects_over_limit() {
        let result = validate_subscription_count(BINANCE_MAX_STREAMS_PER_CONNECTION + 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_combined_book_ticker_message() {
        let text = r#"{
            "stream":"btcusdt@bookTicker",
            "data":{"u":400900217,"s":"BTCUSDT","b":"64000.10","B":"1.2500","a":"64000.20","A":"0.7500"}
        }"#;

        let Event::Book(book) = parse_event(text).unwrap() else {
            panic!("expected book message");
        };

        assert_eq!(book.symbol, "btcusdt");
        assert_eq!(book.update_id, 400900217);
        assert_eq!(book.bid, Decimal::new(6400010, 2));
        assert_eq!(book.bid_qty, Decimal::new(12500, 4));
        assert_eq!(book.ask, Decimal::new(6400020, 2));
        assert_eq!(book.ask_qty, Decimal::new(7500, 4));
    }

    #[test]
    fn test_parse_raw_book_ticker_message() {
        let text = r#"{
            "u":400900217,
            "s":"ETHUSDT",
            "b":"3200.10",
            "B":"4.5",
            "a":"3200.20",
            "A":"2.5"
        }"#;

        let Event::Book(book) = parse_event(text).unwrap() else {
            panic!("expected book message");
        };

        assert_eq!(book.symbol, "ethusdt");
        assert_eq!(book.update_id, 400900217);
        assert_eq!(book.bid, Decimal::new(320010, 2));
        assert_eq!(book.ask, Decimal::new(320020, 2));
    }

    #[test]
    fn test_parse_combined_kline_message() {
        let text = r#"{
            "stream":"btcusdt@kline_5m",
            "data":{
                "e":"kline",
                "E":1735689600000,
                "s":"BTCUSDT",
                "k":{
                    "t":1735689300000,
                    "T":1735689599999,
                    "s":"BTCUSDT",
                    "i":"5m",
                    "o":"64000.0",
                    "c":"64010.5",
                    "h":"64012.0",
                    "l":"63990.0",
                    "v":"12.34",
                    "x":false
                }
            }
        }"#;

        let Event::Kline(parsed) = parse_event(text).unwrap() else {
            panic!("expected kline message");
        };

        assert_eq!(parsed.symbol, Symbol::Btc);
        assert_eq!(parsed.interval, Interval::M5);
        assert_eq!(parsed.candle.close, 64010.5);
        assert!(!parsed.candle.is_closed);
    }

    #[test]
    fn test_parse_http_kline_row() {
        let text = r#"[
            1735689300000,
            "64000.0",
            "64012.0",
            "63990.0",
            "64010.5",
            "12.34",
            1735689599999,
            "0",
            0,
            "0",
            "0",
            "0"
        ]"#;

        let row: HttpKline = serde_json::from_str(text).unwrap();
        let candle = row.into_candle();

        assert_eq!(candle.open_time_ms, 1735689300000);
        assert_eq!(candle.close_time_ms, 1735689599999);
        assert_eq!(candle.open, 64000.0);
        assert_eq!(candle.close, 64010.5);
        assert!(candle.is_closed);
    }

    #[test]
    fn test_handle_control_ack_message() {
        handle_control(r#"{"result":null,"id":7}"#).unwrap();
    }

    #[test]
    fn test_apply_and_get_book() {
        let books = Arc::new(RwLock::new(HashMap::new()));
        let book = BookTicker {
            update_id: 42,
            symbol: "solusdt".to_string(),
            bid: Decimal::new(12345, 2),
            bid_qty: Decimal::new(2000, 3),
            ask: Decimal::new(12355, 2),
            ask_qty: Decimal::new(1500, 3),
        };

        update_book(&books, book.clone()).unwrap();

        let latest = get_book(&books, "SOLUSDT").unwrap();
        assert_eq!(latest, book);
        assert_eq!(latest.mid(), Decimal::new(12350, 2));
    }

    #[test]
    fn test_all_books_can_be_cloned_from_cache() {
        let books = Arc::new(RwLock::new(HashMap::new()));

        update_book(
            &books,
            BookTicker {
                update_id: 1,
                symbol: "btcusdt".to_string(),
                bid: Decimal::new(10000, 2),
                bid_qty: Decimal::ONE,
                ask: Decimal::new(10020, 2),
                ask_qty: Decimal::ONE,
            },
        )
        .unwrap();

        update_book(
            &books,
            BookTicker {
                update_id: 2,
                symbol: "ethusdt".to_string(),
                bid: Decimal::new(2000, 0),
                bid_qty: Decimal::ONE,
                ask: Decimal::new(2002, 0),
                ask_qty: Decimal::ONE,
            },
        )
        .unwrap();

        let all = books.read().unwrap().clone();

        assert_eq!(all.len(), 2);
        assert!(all.contains_key("btcusdt"));
        assert!(all.contains_key("ethusdt"));
    }

    #[test]
    fn test_update_kline_updates_existing_open_bucket() {
        let series = Arc::new(RwLock::new(HashMap::new()));

        update_kline(
            &series,
            KlineEvent {
                symbol: Symbol::Eth,
                interval: Interval::M5,
                candle: Candle {
                    open_time_ms: 1,
                    close_time_ms: 2,
                    open: 100.0,
                    high: 101.0,
                    low: 99.0,
                    close: 100.5,
                    volume: 1.0,
                    is_closed: false,
                },
            },
        )
        .unwrap();

        update_kline(
            &series,
            KlineEvent {
                symbol: Symbol::Eth,
                interval: Interval::M5,
                candle: Candle {
                    open_time_ms: 1,
                    close_time_ms: 2,
                    open: 100.0,
                    high: 102.0,
                    low: 98.0,
                    close: 101.5,
                    volume: 2.0,
                    is_closed: true,
                },
            },
        )
        .unwrap();

        let latest = series
            .read()
            .unwrap()
            .get(&(Symbol::Eth, Interval::M5))
            .unwrap()
            .latest()
            .unwrap();

        assert_eq!(latest.open_time_ms, 1);
        assert_eq!(latest.close, 101.5);
        assert!(latest.is_closed);
    }

    #[test]
    fn test_candle_series_keeps_tail_values_only() {
        let mut series = CandleSeries::new(2);
        series.upsert(Candle {
            open_time_ms: 1,
            close_time_ms: 2,
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            volume: 1.0,
            is_closed: true,
        });
        series.upsert(Candle {
            open_time_ms: 3,
            close_time_ms: 4,
            open: 2.0,
            high: 2.0,
            low: 2.0,
            close: 2.0,
            volume: 1.0,
            is_closed: true,
        });
        series.upsert(Candle {
            open_time_ms: 5,
            close_time_ms: 6,
            open: 3.0,
            high: 3.0,
            low: 3.0,
            close: 3.0,
            volume: 1.0,
            is_closed: true,
        });

        let tail = series.tail(0);
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].open_time_ms, 3);
        assert_eq!(tail[1].open_time_ms, 5);
    }

    #[test]
    fn test_merge_series_updates_background_tail() {
        let series = Arc::new(RwLock::new(HashMap::new()));
        merge_series(
            &series,
            (Symbol::Eth, BackgroundInterval::H1),
            vec![
                Candle {
                    open_time_ms: 1,
                    close_time_ms: 2,
                    open: 100.0,
                    high: 101.0,
                    low: 99.0,
                    close: 100.5,
                    volume: 1.0,
                    is_closed: true,
                },
                Candle {
                    open_time_ms: 3,
                    close_time_ms: 4,
                    open: 101.0,
                    high: 102.0,
                    low: 100.0,
                    close: 101.5,
                    volume: 1.0,
                    is_closed: true,
                },
            ],
        )
        .unwrap();

        let tail = series
            .read()
            .unwrap()
            .get(&(Symbol::Eth, BackgroundInterval::H1))
            .unwrap()
            .tail(2);

        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].open_time_ms, 1);
        assert_eq!(tail[1].open_time_ms, 3);
        assert_eq!(tail[1].close, 101.5);
    }
}
