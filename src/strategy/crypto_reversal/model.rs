//! `crypto_reversal` 的纯策略模型。
//!
//! 这个文件只做两件事：
//! - 定义策略计算直接依赖的数据结构；
//! - 根据一段 K 线序列评估“最新一根 K 线是否形成反转信号”。
//!
//! 它不负责：
//! - 拉取数据；
//! - 读取配置文件；
//! - 输出 JSON / CSV；
//! - 执行下单。
//!
//! 这样可以保证策略逻辑保持纯净，
//! 便于后续给 runtime、example、backtest 复用。

pub use crate::types::market::Candle;

/// 策略配置。
///
/// 这里保留的字段只覆盖当前信号计算和候选过滤真正需要的参数，
/// 不提前引入执行层、审计层或持久化层字段，
/// 避免把策略配置对象继续扩成“万能容器”。
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub warmup_bars: usize,
    pub rsi_period: usize,
    pub bb_period: usize,
    pub bb_stddev: f64,
    pub macd_fast: usize,
    pub macd_slow: usize,
    pub macd_signal: usize,
    pub min_width_pct: f64,
    pub long_rsi_max: f64,
    pub short_rsi_min: f64,
    pub band_pad_pct: f64,
    pub add_score: f64,
    pub max_score: f64,
}

impl Default for Config {
    fn default() -> Self {
        super::constants::default_model_config()
    }
}

impl Config {
    /// 指标序列形成有效值所需的最少 bars。
    ///
    /// 这里描述的是指标层的数学下限，不包含额外热身余量。
    pub fn indicator_bars(&self) -> usize {
        let rsi_bars = self.rsi_period.saturating_add(1);
        let bb_bars = self.bb_period;
        let macd_bars = self
            .macd_fast
            .max(self.macd_slow)
            .saturating_add(self.macd_signal);

        rsi_bars.max(bb_bars).max(macd_bars)
    }

    /// 当前策略评估最新窗口所需的最少 bars。
    ///
    /// 旧 `standalone_bot` 的逻辑要求至少 `warmup_bars + 2`，
    /// 这里保留这个口径，并同时确保不低于指标计算的数学下限。
    pub fn min_bars(&self) -> usize {
        self.indicator_bars()
            .max(self.warmup_bars.saturating_add(2))
    }
}

/// 信号方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Up,
    Down,
}

/// 最新窗口上形成的可执行信号。
///
/// 这个结构只保留后续候选构造一定会用到的信息，
/// 不保留一整套中间指标快照，避免引入冗余状态。
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    pub side: Side,
    pub signal_price: f64,
    pub score: f64,
    pub size_factor: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SignalEvaluation {
    Signal(Signal),
    Rejected(SignalRejectReason),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SignalRejectReason {
    InsufficientCandles {
        have: usize,
        need: usize,
    },
    IndicatorUnavailable,
    WidthTooNarrow {
        width_pct: f64,
        min_width_pct: f64,
    },
    EntryConditionsNotMet {
        price: f64,
        lower: f64,
        upper: f64,
        rsi: f64,
        long_rsi_max: f64,
        short_rsi_min: f64,
    },
}

impl SignalRejectReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InsufficientCandles { .. } => "insufficient_candles",
            Self::IndicatorUnavailable => "indicator_unavailable",
            Self::WidthTooNarrow { .. } => "width_too_narrow",
            Self::EntryConditionsNotMet { .. } => "entry_conditions_not_met",
        }
    }

    pub fn detail_cn(&self) -> String {
        match self {
            Self::InsufficientCandles { have, need } => {
                format!("K线数量不足，当前 {have} 根，需要至少 {need} 根")
            }
            Self::IndicatorUnavailable => "指标序列未就绪，当前窗口无法完成信号计算".to_string(),
            Self::WidthTooNarrow {
                width_pct,
                min_width_pct,
            } => format!(
                "布林带宽度不足，当前 {:.4}% ，阈值 {:.4}%",
                width_pct, min_width_pct
            ),
            Self::EntryConditionsNotMet {
                price,
                lower,
                upper,
                rsi,
                long_rsi_max,
                short_rsi_min,
            } => format!(
                "未满足反转入场条件：做多[价格<=下轨:{} RSI<阈值:{}]，做空[价格>=上轨:{} RSI>阈值:{}]；当前 price={:.6} lower={:.6} upper={:.6} rsi={:.6}",
                yes_no(*price <= *lower),
                yes_no(*rsi < *long_rsi_max),
                yes_no(*price >= *upper),
                yes_no(*rsi > *short_rsi_min),
                price,
                lower,
                upper,
                rsi
            ),
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "是" } else { "否" }
}

#[derive(Debug, Clone)]
struct Series {
    rsi: Vec<Option<f64>>,
    bb_basis: Vec<Option<f64>>,
    bb_upper: Vec<Option<f64>>,
    bb_lower: Vec<Option<f64>>,
    bb_width_pct: Vec<Option<f64>>,
    macd_hist: Vec<Option<f64>>,
}

/// 基于一段 candles 评估最新窗口上的信号。
pub fn signal(config: &Config, candles: &[Candle]) -> Option<Signal> {
    match evaluate_signal(config, candles) {
        SignalEvaluation::Signal(signal) => Some(signal),
        SignalEvaluation::Rejected(_) => None,
    }
}

pub fn evaluate_signal(config: &Config, candles: &[Candle]) -> SignalEvaluation {
    if candles.len() < config.min_bars() {
        return SignalEvaluation::Rejected(SignalRejectReason::InsufficientCandles {
            have: candles.len(),
            need: config.min_bars(),
        });
    }

    let closes: Vec<f64> = candles.iter().map(|candle| candle.close).collect();
    let series = compute_series(config, &closes);
    let index = candles.len() - 1;

    let (
        Some(rsi_value),
        Some(basis),
        Some(upper),
        Some(lower),
        Some(width),
        Some(hist),
        Some(prev_hist),
    ) = (
        series.rsi[index],
        series.bb_basis[index],
        series.bb_upper[index],
        series.bb_lower[index],
        series.bb_width_pct[index],
        series.macd_hist[index],
        index.checked_sub(1).and_then(|prev| series.macd_hist[prev]),
    )
    else {
        return SignalEvaluation::Rejected(SignalRejectReason::IndicatorUnavailable);
    };

    if width < config.min_width_pct {
        return SignalEvaluation::Rejected(SignalRejectReason::WidthTooNarrow {
            width_pct: width,
            min_width_pct: config.min_width_pct,
        });
    }

    let signal_price = candles[index].close;
    let band_pad = basis * (config.band_pad_pct / 100.0);

    // `Up` 信号表示价格压到下轨附近，策略预期下一阶段向上回归；
    // `Down` 信号表示价格抬到上轨附近，策略预期下一阶段向下回归。
    let long_cond = signal_price <= (lower + band_pad) && rsi_value < config.long_rsi_max;
    let short_cond = signal_price >= (upper - band_pad) && rsi_value > config.short_rsi_min;

    if !long_cond && !short_cond {
        return SignalEvaluation::Rejected(SignalRejectReason::EntryConditionsNotMet {
            price: signal_price,
            lower,
            upper,
            rsi: rsi_value,
            long_rsi_max: config.long_rsi_max,
            short_rsi_min: config.short_rsi_min,
        });
    }

    let side = if long_cond { Side::Up } else { Side::Down };

    // 这里保留旧策略里“MACD 作为确认和加分项”的口径，
    // 但不把它做成硬过滤，避免对主信号结构引入额外分支耦合。
    let macd_confirm = if long_cond {
        hist >= prev_hist
    } else {
        hist <= prev_hist
    };

    let score = score(
        config,
        side,
        signal_price,
        basis,
        upper,
        lower,
        width,
        rsi_value,
        macd_confirm,
    );

    SignalEvaluation::Signal(Signal {
        side,
        signal_price,
        score,
        size_factor: size_factor(config, score),
    })
}

fn compute_series(config: &Config, closes: &[f64]) -> Series {
    let rsi_values = rsi(closes, config.rsi_period);
    let bb_basis = rolling_sma(closes, config.bb_period);
    let bb_std = rolling_std(closes, config.bb_period, &bb_basis);
    let ema_fast = ema(closes, config.macd_fast);
    let ema_slow = ema(closes, config.macd_slow);

    let mut bb_upper = vec![None; closes.len()];
    let mut bb_lower = vec![None; closes.len()];
    let mut bb_width_pct = vec![None; closes.len()];
    let mut macd_line = vec![None; closes.len()];
    let mut macd_hist = vec![None; closes.len()];

    for index in 0..closes.len() {
        if let (Some(fast), Some(slow)) = (ema_fast[index], ema_slow[index]) {
            macd_line[index] = Some(fast - slow);
        }

        if let (Some(basis), Some(std)) = (bb_basis[index], bb_std[index]) {
            if basis != 0.0 {
                let upper = basis + config.bb_stddev * std;
                let lower = basis - config.bb_stddev * std;
                bb_upper[index] = Some(upper);
                bb_lower[index] = Some(lower);
                bb_width_pct[index] = Some((upper - lower) / basis * 100.0);
            }
        }
    }

    let macd_seed: Vec<f64> = macd_line.iter().filter_map(|value| *value).collect();
    let macd_signal = ema(&macd_seed, config.macd_signal);
    let mut macd_signal_values = vec![None; closes.len()];
    let mut seed_index = 0usize;

    for (index, value) in macd_line.iter().enumerate() {
        if value.is_some() {
            macd_signal_values[index] = macd_signal.get(seed_index).copied().flatten();
            seed_index += 1;
        }
    }

    for index in 0..closes.len() {
        if let (Some(line), Some(signal)) = (macd_line[index], macd_signal_values[index]) {
            macd_hist[index] = Some(line - signal);
        }
    }

    Series {
        rsi: rsi_values,
        bb_basis,
        bb_upper,
        bb_lower,
        bb_width_pct,
        macd_hist,
    }
}

fn ema(values: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; values.len()];

    if period == 0 || values.len() < period {
        return out;
    }

    // 这里沿用常见 EMA 种子：前 `period` 个值的简单平均。
    let seed = values[..period].iter().sum::<f64>() / period as f64;
    out[period - 1] = Some(seed);

    let alpha = 2.0 / (period as f64 + 1.0);
    let mut prev = seed;

    for index in period..values.len() {
        prev = values[index] * alpha + prev * (1.0 - alpha);
        out[index] = Some(prev);
    }

    out
}

fn rsi(values: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; values.len()];

    if period == 0 || values.len() <= period {
        return out;
    }

    let mut gains = Vec::with_capacity(period);
    let mut losses = Vec::with_capacity(period);

    for index in 1..=period {
        let delta = values[index] - values[index - 1];
        gains.push(delta.max(0.0));
        losses.push((-delta).max(0.0));
    }

    let mut avg_gain = gains.iter().sum::<f64>() / period as f64;
    let mut avg_loss = losses.iter().sum::<f64>() / period as f64;

    out[period] = Some(if avg_loss == 0.0 {
        100.0
    } else {
        100.0 - (100.0 / (1.0 + avg_gain / avg_loss))
    });

    for index in period + 1..values.len() {
        let delta = values[index] - values[index - 1];
        let gain = delta.max(0.0);
        let loss = (-delta).max(0.0);

        avg_gain = ((avg_gain * (period as f64 - 1.0)) + gain) / period as f64;
        avg_loss = ((avg_loss * (period as f64 - 1.0)) + loss) / period as f64;

        out[index] = Some(if avg_loss == 0.0 {
            100.0
        } else {
            100.0 - (100.0 / (1.0 + avg_gain / avg_loss))
        });
    }

    out
}

fn rolling_sma(values: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; values.len()];

    if period == 0 || values.len() < period {
        return out;
    }

    let mut sum = values[..period].iter().sum::<f64>();
    out[period - 1] = Some(sum / period as f64);

    for index in period..values.len() {
        sum += values[index] - values[index - period];
        out[index] = Some(sum / period as f64);
    }

    out
}

fn rolling_std(values: &[f64], period: usize, means: &[Option<f64>]) -> Vec<Option<f64>> {
    let mut out = vec![None; values.len()];

    if period == 0 || values.len() < period {
        return out;
    }

    for index in period - 1..values.len() {
        let Some(mean) = means[index] else {
            continue;
        };

        // 这里直接对滑窗做方差计算。
        // 对当前 v1 的样本规模来说，这个实现足够直接清晰；
        // 如果后续它进入热路径，再单独做增量优化。
        let window = &values[index + 1 - period..=index];
        let variance = window
            .iter()
            .map(|value| {
                let diff = value - mean;
                diff * diff
            })
            .sum::<f64>()
            / period as f64;

        out[index] = Some(variance.sqrt());
    }

    out
}

fn score(
    config: &Config,
    side: Side,
    signal_price: f64,
    basis: f64,
    upper: f64,
    lower: f64,
    width: f64,
    rsi_value: f64,
    macd_confirm: bool,
) -> f64 {
    let basis = basis.max(1e-9);

    let (rsi_component, band_component) = match side {
        Side::Up => (
            (config.long_rsi_max - rsi_value).max(0.0) / config.long_rsi_max.max(1e-9),
            (lower - signal_price).max(0.0) / basis * 100.0,
        ),
        Side::Down => (
            (rsi_value - config.short_rsi_min).max(0.0) / (100.0 - config.short_rsi_min).max(1e-9),
            (signal_price - upper).max(0.0) / basis * 100.0,
        ),
    };

    let width_component = width / 10.0;
    let macd_component = if macd_confirm { 0.15 } else { 0.0 };

    rsi_component + band_component + width_component + macd_component
}

fn size_factor(config: &Config, score: f64) -> f64 {
    if score >= config.max_score {
        2.0
    } else if score >= config.add_score {
        1.5
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(left: f64, right: f64, tolerance: f64) {
        let diff = (left - right).abs();
        assert!(
            diff <= tolerance,
            "left={left}, right={right}, diff={diff}, tolerance={tolerance}"
        );
    }

    fn base_config() -> Config {
        Config {
            warmup_bars: 4,
            rsi_period: 2,
            bb_period: 3,
            bb_stddev: 1.0,
            macd_fast: 2,
            macd_slow: 3,
            macd_signal: 2,
            min_width_pct: 0.0,
            long_rsi_max: 40.0,
            short_rsi_min: 60.0,
            band_pad_pct: 0.0,
            add_score: 0.2,
            max_score: 0.5,
        }
    }

    fn candles(closes: &[f64]) -> Vec<Candle> {
        closes
            .iter()
            .enumerate()
            .map(|(index, close)| Candle {
                open_time_ms: index as i64 * 300_000,
                close_time_ms: index as i64 * 300_000 + 299_999,
                open: *close,
                high: *close,
                low: *close,
                close: *close,
                volume: 0.0,
                is_closed: true,
            })
            .collect()
    }

    #[test]
    fn rsi_returns_none_when_input_is_too_short() {
        let values = [100.0, 101.0];
        let result = rsi(&values, 2);

        assert_eq!(result, vec![None, None]);
    }

    #[test]
    fn rsi_matches_simple_rising_case() {
        let values = [100.0, 101.0, 102.0, 103.0];
        let result = rsi(&values, 2);

        assert_eq!(result[0], None);
        assert_eq!(result[1], None);
        assert_eq!(result[2], Some(100.0));
        assert_eq!(result[3], Some(100.0));
    }

    #[test]
    fn rolling_sma_and_std_work_on_small_window() {
        let values = [1.0, 2.0, 3.0];
        let means = rolling_sma(&values, 3);
        let stds = rolling_std(&values, 3, &means);

        assert_eq!(means[0], None);
        assert_eq!(means[1], None);
        approx_eq(means[2].unwrap(), 2.0, 1e-12);
        approx_eq(stds[2].unwrap(), (2.0_f64 / 3.0).sqrt(), 1e-12);
    }

    #[test]
    fn indicator_bars_matches_standard_defaults() {
        let config = Config {
            warmup_bars: 50,
            rsi_period: 14,
            bb_period: 20,
            bb_stddev: 2.0,
            macd_fast: 12,
            macd_slow: 26,
            macd_signal: 9,
            min_width_pct: 0.0,
            long_rsi_max: 35.0,
            short_rsi_min: 65.0,
            band_pad_pct: 0.0,
            add_score: 0.2,
            max_score: 0.5,
        };

        assert_eq!(config.indicator_bars(), 35);
        assert_eq!(config.min_bars(), 52);
    }

    #[test]
    fn signal_returns_none_when_candles_are_insufficient() {
        let config = base_config();
        let result = signal(&config, &candles(&[100.0, 99.0, 98.0]));

        assert_eq!(result, None);
    }

    #[test]
    fn signal_detects_up_reversal_on_sharp_selloff() {
        let config = base_config();
        let input = candles(&[100.0, 101.0, 100.0, 99.0, 98.0, 90.0]);
        let Some(result) = signal(&config, &input) else {
            panic!("expected up signal");
        };

        assert_eq!(result.side, Side::Up);
        assert!(result.score > 0.0);
        assert!(result.size_factor >= 1.0);
        approx_eq(result.signal_price, 90.0, 1e-12);
    }

    #[test]
    fn signal_detects_down_reversal_on_sharp_spike() {
        let config = base_config();
        let input = candles(&[100.0, 99.0, 100.0, 101.0, 102.0, 110.0]);
        let Some(result) = signal(&config, &input) else {
            panic!("expected down signal");
        };

        assert_eq!(result.side, Side::Down);
        assert!(result.score > 0.0);
        assert!(result.size_factor >= 1.0);
        approx_eq(result.signal_price, 110.0, 1e-12);
    }

    #[test]
    fn size_factor_matches_old_threshold_steps() {
        let config = Config {
            warmup_bars: 50,
            rsi_period: 14,
            bb_period: 20,
            bb_stddev: 2.0,
            macd_fast: 12,
            macd_slow: 26,
            macd_signal: 9,
            min_width_pct: 0.01,
            long_rsi_max: 35.0,
            short_rsi_min: 65.0,
            band_pad_pct: 0.002,
            add_score: 0.1,
            max_score: 0.2,
        };

        approx_eq(size_factor(&config, 0.05), 1.0, 1e-12);
        approx_eq(size_factor(&config, 0.10), 1.5, 1e-12);
        approx_eq(size_factor(&config, 0.20), 2.0, 1e-12);
    }
}
