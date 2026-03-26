//! 通用市场数据类型。
//!
//! 当前先提供一个仓库级可复用的 `Candle`，
//! 用来承接交易所 kline、聚合后的本地 K 线，以及策略层的 candle 输入。
//!
//! 之所以把它放到 `src/types/`，而不是继续留在策略模块里，
//! 是为了避免：
//! - Binance 数据层定义一份 candle；
//! - 策略层再定义一份 candle；
//! - 两层之间来回做一次性类型转换。

/// 通用 K 线结构。
///
/// 当前采用 `f64` 承载 OHLCV，
/// 因为它主要用于研究和策略信号计算，而不是执行价格撮合。
/// 这样可以减少从字符串或 `Decimal` 到策略指标计算之间的重复转换。
#[derive(Debug, Clone, PartialEq)]
pub struct Candle {
    pub open_time_ms: i64,
    pub close_time_ms: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub is_closed: bool,
}
