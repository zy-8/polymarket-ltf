use std::str::FromStr;
use std::sync::LazyLock;

use polymarket_client_sdk_v2::clob::types::OrderType;
use rust_decimal::Decimal;

use crate::strategy::crypto_reversal::model;

// 5m 周期在每个 300 秒窗口的第 290 秒开始进入扫描窗口。
pub const M5_SCAN_START_MS: i64 = 290_000;
// 15m 周期在每个 900 秒窗口的第 890 秒开始进入扫描窗口。
pub const M15_SCAN_START_MS: i64 = 890_000;
// 5m 挂单在开盘后 30 秒仍然 0 成交时触发最终撤单检查。
pub const M5_CANCEL_AFTER_OPEN_MS: i64 = 30_000;
// 15m 挂单在开盘后 120 秒仍然 0 成交时触发最终撤单检查。
pub const M15_CANCEL_AFTER_OPEN_MS: i64 = 120_000;

// Polymarket 下单允许的最小 shares；低于这个值会被抬到最小门槛。
pub const POLY_MIN_ORDER_SIZE_SHARES: f64 = 5.0;
// Polymarket 下单允许的最小名义金额；会和最小 shares 一起约束最终下单量。
pub const POLY_MIN_ORDER_NOTIONAL_USDC: f64 = 1.0;
// 当前策略统一使用 GTC 限价单。
pub const ORDER_TYPE: OrderType = OrderType::GTC;
// 写入事件和日志时使用的策略名。
pub const STRATEGY_NAME: &str = "crypto_reversal";

// 小于这个持仓阈值的残仓会被视为 dust，不阻塞新单。
pub static POSITION_DUST_THRESHOLD: LazyLock<Decimal> = LazyLock::new(|| Decimal::new(1, 4));
// 报价高于这个价格上限时，策略直接放弃入场。
pub static MAX_ENTRY_PRICE: LazyLock<Decimal> =
    LazyLock::new(|| Decimal::from_str("0.54").expect("max entry price should parse"));

pub fn default_model_config() -> model::Config {
    model::Config {
        // 信号计算要求的热身 bars 数量。
        warmup_bars: 100,
        // RSI 指标周期。
        rsi_period: 14,
        // Bollinger Basis / Std 的滚动窗口。
        bb_period: 30,
        // Bollinger 带宽使用的标准差倍数。
        bb_stddev: 2.0,
        // MACD 快线 EMA 周期。
        macd_fast: 12,
        // MACD 慢线 EMA 周期。
        macd_slow: 26,
        // MACD 信号线 EMA 周期。
        macd_signal: 9,
        // 布林带宽度低于这个百分比时，不认为当前有足够波动。
        min_width_pct: 0.2,
        // 做多反转时 RSI 必须低于这个阈值。
        long_rsi_max: 40.0,
        // 做空反转时 RSI 必须高于这个阈值。
        short_rsi_min: 60.0,
        // 价格触碰上下轨时额外允许的边界缓冲百分比。
        band_pad_pct: 0.0,
        // score 达到这个阈值后，仓位倍率进入加仓档。
        add_score: 0.32,
        // score 达到这个阈值后，仓位倍率进入最高档。
        max_score: 0.5,
    }
}
