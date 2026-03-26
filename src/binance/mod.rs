//! Binance 数据接入模块。
//!
//! 当前统一收敛到一个 `websocket` 模块：
//! - 维护最新 `bookTicker`；
//! - 维护最新 `kline` 序列；
//! - 统一承接连接、重连和订阅管理。

pub mod websocket;

pub use websocket::{BackgroundInterval, BookTicker, Client};
