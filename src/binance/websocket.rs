//! Binance WebSocket 最简客户端。
//!
//! 这个模块只做一件事：
//! 持续订阅 Binance `bookTicker`，并在内存里维护多个交易对的最新盘口。
//!
//! 对外只保留最常用的几个接口：
//! - `Client::connect(symbols)`：启动后台连接
//! - `client.get(symbol)`：读取某个交易对的最新盘口
//! - `client.price(symbol)`：读取某个交易对的最新中间价
//! - `client.all()`：读取全部交易对的最新盘口
//! - `client.close()`：关闭后台任务
//!
//! 这里的“最新价格”不是成交价，而是 `bookTicker` 的中间价：
//! `mid = (bid + ask) / 2`
//!
//! 当前实现保留了 Binance 官方文档里真正需要处理的约束：
//! - 使用 `stream?streams=` combined stream 一次订阅多个 symbol
//! - symbol 统一转为小写
//! - 自动响应服务端 `ping`
//! - 连接在接近 24 小时时主动轮换
//! - 超过 1024 个 stream 时自动分片成多条连接

use crate::errors::{PolyfillError, Result, StreamErrorKind};
use crate::types::crypto::Symbol;
use futures::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use tokio::task::AbortHandle;
use tokio::time::{Instant, sleep, sleep_until};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

/// Binance Spot WebSocket 地址。
pub const BINANCE_WS_BASE_URL: &str = "wss://stream.binance.com:9443";

/// 单条 combined stream 连接可挂载的最大 stream 数量。
pub const BINANCE_MAX_STREAMS_PER_CONNECTION: usize = 1024;

/// combined stream 的 URL 路径前缀。
const BINANCE_COMBINED_STREAM_PATH: &str = "stream?streams=";

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

/// 多个 symbol 的最新盘口缓存。
type SharedBooks = Arc<RwLock<HashMap<String, BookTicker>>>;

/// Binance `bookTicker` 对应的业务结构。
///
/// 对应官方文档中的 payload：
///
/// ```json
/// {
///   "u": 400900217,
///   "s": "BNBUSDT",
///   "b": "25.35190000",
///   "B": "31.21000000",
///   "a": "25.36520000",
///   "A": "40.66000000"
/// }
/// ```
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
    /// 返回当前盘口的中间价。
    ///
    /// 对只关心“最新参考价格”的场景，中间价通常是最方便的表达。
    pub fn mid(&self) -> Decimal {
        (self.bid + self.ask) / Decimal::from(DECIMAL_TWO)
    }
}

/// 最简客户端。
///
/// `Client` 自己维护后台 WebSocket 任务和最新盘口缓存。
/// 外部只需要拿着这个对象去读数据，不需要自己管理连接细节。
pub struct Client {
    symbols: Vec<Symbol>,
    urls: Vec<String>,
    books: SharedBooks,
    tasks: Mutex<Vec<AbortHandle>>,
}

impl Client {
    /// 启动客户端并订阅多个 symbol 的 `bookTicker`。
    pub async fn connect(symbols: &[Symbol]) -> Result<Self> {
        if symbols.is_empty() {
            return Err(PolyfillError::validation(
                "Binance WebSocket 至少需要一个 symbol",
            ));
        }

        let symbols = symbols.to_vec();
        let urls = build_urls(&symbols)?;
        let books = Arc::new(RwLock::new(HashMap::new()));

        let tasks = urls
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, url)| {
                tokio::spawn(run_connection_loop(index, url, Arc::clone(&books))).abort_handle()
            })
            .collect::<Vec<_>>();

        info!(
            "已启动 Binance WebSocket 客户端: symbols={}, connections={}",
            symbols.len(),
            urls.len()
        );

        Ok(Self {
            symbols,
            urls,
            books,
            tasks: Mutex::new(tasks),
        })
    }

    /// 返回当前订阅的 symbol 列表。
    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    /// 返回当前建立的连接数量。
    ///
    /// 如果 symbol 数量很多，会自动拆成多条连接。
    pub fn connection_count(&self) -> usize {
        self.urls.len()
    }

    /// 读取某个 symbol 的最新盘口。
    ///
    /// 方法内部会自动把传入 symbol 规范化成小写，所以外部传 `BTCUSDT`
    /// 或 `btcusdt` 都可以。
    pub fn get(&self, symbol: &str) -> Option<BookTicker> {
        get_book(&self.books, symbol)
    }

    /// 读取某个 symbol 的最新中间价。
    pub fn price(&self, symbol: &str) -> Option<Decimal> {
        self.get(symbol).map(|book| book.mid())
    }

    /// 读取当前全部 symbol 的最新盘口。
    ///
    /// 这里返回 `HashMap` 的副本，调用简单直接。
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

    /// 关闭后台任务。
    ///
    /// 这里直接使用 `abort`，实现最简单，也足够满足当前需求。
    pub fn close(&self) {
        self.close_inner();
    }

    fn close_inner(&self) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|poisoned| {
            warn!("Binance 后台任务句柄锁已被污染，继续强制关闭");
            poisoned.into_inner()
        });

        for task in tasks.drain(..) {
            task.abort();
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close_inner();
    }
}

/// 单条 WebSocket 连接的主循环。
///
/// 这个循环负责两件事：
/// - 建立连接
/// - 连接断开后按指数退避自动重连
async fn run_connection_loop(connection_index: usize, url: String, books: SharedBooks) {
    let mut delay = RECONNECT_BASE_DELAY;

    loop {
        match tokio_tungstenite::connect_async(&url).await {
            Ok((stream, _)) => {
                info!("Binance shard {} 连接成功: {}", connection_index, url);
                delay = RECONNECT_BASE_DELAY;

                if let Err(error) = run_stream(connection_index, stream, &books).await {
                    warn!("Binance shard {} 连接中断: {}", connection_index, error);
                }
            }
            Err(error) => {
                warn!("Binance shard {} 连接失败: {}", connection_index, error);
            }
        }

        sleep(delay).await;
        delay = next_reconnect_delay(delay);
    }
}

/// 单条已经连上的 WebSocket 的消息循环。
async fn run_stream(
    connection_index: usize,
    mut stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    books: &SharedBooks,
) -> Result<()> {
    let rotate_at = Instant::now() + BINANCE_ROTATE_AFTER;

    loop {
        tokio::select! {
            _ = sleep_until(rotate_at) => {
                info!(
                    "Binance shard {} 主动轮换连接，避免触发 24 小时上限",
                    connection_index
                );
                return Ok(());
            }
            message = stream.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        if let Err(error) = handle_text(&text, books) {
                            warn!("Binance shard {} 解析消息失败: {}", connection_index, error);
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
                        info!("Binance shard {} 被服务端关闭: {:?}", connection_index, frame);
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

fn handle_text(text: &str, books: &SharedBooks) -> Result<()> {
    let book = parse_book_ticker(text)?;
    apply_book(books, book)
}

/// 把一条最新盘口写入缓存。
fn apply_book(books: &SharedBooks, book: BookTicker) -> Result<()> {
    let mut guard = books
        .write()
        .map_err(|_| PolyfillError::internal_simple("Binance 价格缓存写锁已被污染"))?;

    debug!("更新 Binance 最新盘口: {}", book.symbol);
    guard.insert(book.symbol.clone(), book);
    Ok(())
}

/// 从缓存中读取某个 symbol 的最新盘口。
fn get_book(books: &SharedBooks, symbol: &str) -> Option<BookTicker> {
    let symbol = normalize_symbol(symbol).ok()?;
    books.read().ok()?.get(&symbol).cloned()
}

/// 解析 Binance `bookTicker` 消息。
///
/// 支持两种格式：
/// - combined stream：`{"stream":"...","data":{...}}`
/// - raw payload：`{"u":...,"s":...,"b":...,"B":...,"a":...,"A":...}`
fn parse_book_ticker(text: &str) -> Result<BookTicker> {
    if let Ok(wrapper) = serde_json::from_str::<CombinedMessage>(text) {
        return Ok(wrapper.data.into_book());
    }

    let payload: RawBookTicker = serde_json::from_str(text).map_err(|error| {
        PolyfillError::parse(
            format!("解析 Binance bookTicker payload 失败: {}", error),
            Some(Box::new(error)),
        )
    })?;

    Ok(payload.into_book())
}

/// 根据订阅的 symbol 构建一组 WebSocket URL。
fn build_urls(symbols: &[Symbol]) -> Result<Vec<String>> {
    if symbols.is_empty() {
        return Err(PolyfillError::validation(
            "Binance WebSocket 至少需要一个 symbol",
        ));
    }

    Ok(symbols
        .chunks(BINANCE_MAX_STREAMS_PER_CONNECTION)
        .map(build_url)
        .collect())
}

/// 为一组 symbol 构造 combined stream URL。
fn build_url(symbols: &[Symbol]) -> String {
    let streams = symbols
        .iter()
        .map(|symbol| format!("{}@bookTicker", symbol.as_binance_symbol()))
        .collect::<Vec<_>>()
        .join("/");

    format!(
        "{}/{}{}",
        BINANCE_WS_BASE_URL, BINANCE_COMBINED_STREAM_PATH, streams
    )
}

/// 标准化单个 symbol。
fn normalize_symbol(symbol: &str) -> Result<String> {
    let symbol = symbol.trim().to_ascii_lowercase();
    if symbol.is_empty() {
        return Err(PolyfillError::validation("Binance symbol 不能为空"));
    }

    Ok(symbol)
}

/// 根据固定退避规则计算下次重连等待时间。
fn next_reconnect_delay(current_delay: std::time::Duration) -> std::time::Duration {
    let next_millis = current_delay
        .as_millis()
        .saturating_mul(RECONNECT_MULTIPLIER as u128)
        .min(RECONNECT_MAX_DELAY.as_millis());

    std::time::Duration::from_millis(next_millis as u64)
}

#[derive(Debug, Deserialize)]
struct CombinedMessage {
    #[allow(dead_code)]
    stream: String,
    data: RawBookTicker,
}

#[derive(Debug, Deserialize)]
struct RawBookTicker {
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

impl RawBookTicker {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_urls_from_symbols() {
        let urls = build_urls(&[Symbol::Btc, Symbol::Eth]).unwrap();
        assert_eq!(
            urls,
            vec![
                "wss://stream.binance.com:9443/stream?streams=btcusdt@bookTicker/ethusdt@bookTicker"
                    .to_string()
            ]
        );
    }

    #[test]
    fn test_build_urls_shards_large_symbol_sets() {
        let symbols = vec![Symbol::Btc; BINANCE_MAX_STREAMS_PER_CONNECTION + 1];
        let urls = build_urls(&symbols).unwrap();

        assert_eq!(urls.len(), 2);
        assert!(urls[0].contains("btcusdt@bookTicker"));
        assert!(urls[1].contains("btcusdt@bookTicker"));
    }

    #[test]
    fn test_parse_combined_book_ticker_message() {
        let text = r#"{
            "stream":"btcusdt@bookTicker",
            "data":{"u":400900217,"s":"BTCUSDT","b":"64000.10","B":"1.2500","a":"64000.20","A":"0.7500"}
        }"#;

        let book = parse_book_ticker(text).unwrap();

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

        let book = parse_book_ticker(text).unwrap();

        assert_eq!(book.symbol, "ethusdt");
        assert_eq!(book.update_id, 400900217);
        assert_eq!(book.bid, Decimal::new(320010, 2));
        assert_eq!(book.ask, Decimal::new(320020, 2));
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

        apply_book(&books, book.clone()).unwrap();

        let latest = get_book(&books, "SOLUSDT").unwrap();
        assert_eq!(latest, book);
        assert_eq!(latest.mid(), Decimal::new(12350, 2));
    }

    #[test]
    fn test_all_books_can_be_cloned_from_cache() {
        let books = Arc::new(RwLock::new(HashMap::new()));

        apply_book(
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

        apply_book(
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
    fn test_next_reconnect_delay_caps_at_max_delay() {
        assert_eq!(
            next_reconnect_delay(std::time::Duration::from_secs(1)),
            std::time::Duration::from_secs(2)
        );
        assert_eq!(
            next_reconnect_delay(std::time::Duration::from_secs(30)),
            std::time::Duration::from_secs(30)
        );
    }
}
