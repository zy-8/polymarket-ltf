//! `crypto_reversal` 的候选构造服务。
//!
//! 这里不负责“拉取 next quote”或“决定如何调度策略”，
//! 只负责把：
//! - 一份策略配置；
//! - 一段 candles；
//! - 一个 next market；
//! 组合成可用于上层 runtime 的候选结果。
//!
//! 这样做的目的是把运行时拼装逻辑压缩到最小，
//! 并且避免把 next quote 读取、账户状态、执行状态继续耦合到策略主逻辑里。
//!
//! 当前这个模块额外提供一组轻量 helper：
//! - 从策略配置收集所需的 Binance `kline` 订阅；
//! - 基于当前缓存批量执行评估；
//!
//! 这里刻意不替策略层订阅 Binance `bookTicker`，
//! 因为当前 `crypto_reversal` 的真实输入只包括：
//! - Binance `kline`
//! - Polymarket next market
//!
//! 如果后续策略逻辑真正开始依赖 Binance 最新盘口，
//! 再由这里显式扩展订阅集合，而不是提前保留无用输入。

use std::sync::{Arc, RwLock};

use crate::binance;
use crate::errors::{PolyfillError, Result};
use crate::polymarket::market_registry::MarketRegistry;
use crate::polymarket::utils::crypto_market::next_slug;
use crate::strategy::crypto_reversal::model::{
    Candle, Config as ModelConfig, Side, SignalEvaluation, SignalRejectReason, evaluate_signal,
};
use crate::types::crypto::{Interval, Symbol};

/// 运行时评估配置。
///
/// 纯指标参数保留在 `model::Config`，
/// 这里只保留资产和周期这类运行时字段。
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub symbol: Symbol,
    pub interval: Interval,
    pub model: ModelConfig,
}

/// 由信号和 next market 组合出的候选。
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub symbol: Symbol,
    pub interval: Interval,
    pub market_slug: String,
    pub side: Side,
    pub signal_time_ms: i64,
    pub score: f64,
    pub size_factor: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    pub symbol: Symbol,
    pub interval: Interval,
    pub market_slug: String,
    pub outcome: EvaluationOutcome,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EvaluationOutcome {
    Candidate(Candidate),
    Rejected(EvaluationRejectReason),
}

#[derive(Debug, Clone, PartialEq)]
pub enum EvaluationRejectReason {
    Signal(SignalRejectReason),
    MarketMissing,
    BackgroundDataUnavailable,
    BackgroundBlocked,
}

impl EvaluationRejectReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Signal(reason) => reason.as_str(),
            Self::MarketMissing => "market_missing",
            Self::BackgroundDataUnavailable => "background_data_unavailable",
            Self::BackgroundBlocked => "background_blocked",
        }
    }

    pub fn detail_cn(&self) -> String {
        match self {
            Self::Signal(reason) => reason.detail_cn(),
            Self::MarketMissing => "下一期 market 尚未进入 registry，当前不能下单".to_string(),
            Self::BackgroundDataUnavailable => "背景周期数据不足，无法完成背景过滤".to_string(),
            Self::BackgroundBlocked => "背景周期过滤阻止本次入场".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackgroundAction {
    Allow,
    Reduce,
    Block,
}

#[derive(Debug, Clone, PartialEq)]
struct Inputs {
    klines: Vec<(Symbol, Interval)>,
    background: Vec<(Symbol, binance::BackgroundInterval)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct BackgroundProfile {
    fast_lookback: usize,
    slow_lookback: usize,
    block_fast_pct: f64,
    block_slow_pct: f64,
    reduce_fast_pct: f64,
    reduce_slow_pct: f64,
    reduce_factor: f64,
}

const BACKGROUND_LOOKBACK_15M: usize = 8;
const BACKGROUND_LOOKBACK_1H: usize = 6;
const BACKGROUND_BLOCK_15M_PCT: f64 = 0.006;
const BACKGROUND_BLOCK_1H_PCT: f64 = 0.010;
const BACKGROUND_REDUCE_15M_PCT: f64 = 0.003;
const BACKGROUND_REDUCE_1H_PCT: f64 = 0.005;
const BACKGROUND_REDUCE_FACTOR: f64 = 0.5;
const BACKGROUND_BLOCK_1H_PCT_15M: f64 = 0.008;
const BACKGROUND_BLOCK_4H_PCT_15M: f64 = 0.012;
const BACKGROUND_REDUCE_1H_PCT_15M: f64 = 0.004;
const BACKGROUND_REDUCE_4H_PCT_15M: f64 = 0.006;
const BACKGROUND_REDUCE_FACTOR_15M: f64 = 0.75;

/// 根据最新价格序列和下一期 market 输出候选。
///
/// 这里不再使用当前盘口或本地 orderbook。
/// 这个阶段只负责：
/// - 纯信号评估；
/// - 解析下一期 market slug；
/// - 把候选留给执行层去定价。
pub fn candidate(
    config: &Config,
    candles: &[Candle],
    market_slug: Option<&str>,
) -> Option<Candidate> {
    let SignalEvaluation::Signal(signal) = evaluate_signal(&config.model, candles) else {
        return None;
    };
    let market_slug = market_slug?;
    let signal_time_ms = candles.last()?.close_time_ms;

    Some(Candidate {
        symbol: config.symbol,
        interval: config.interval,
        market_slug: market_slug.to_string(),
        side: signal.side,
        signal_time_ms,
        score: signal.score,
        size_factor: signal.size_factor,
    })
}

pub fn evaluate(config: &Config, candles: &[Candle], market_slug: Option<&str>) -> Evaluation {
    let market_slug = market_slug
        .map(str::to_string)
        .unwrap_or_else(|| next_slug(config.symbol, config.interval).unwrap_or_default());

    let outcome = match evaluate_signal(&config.model, candles) {
        SignalEvaluation::Signal(signal) => match candles.last() {
            Some(last_candle) => match market_slug.is_empty() {
                true => EvaluationOutcome::Rejected(EvaluationRejectReason::MarketMissing),
                false => EvaluationOutcome::Candidate(Candidate {
                    symbol: config.symbol,
                    interval: config.interval,
                    market_slug: market_slug.clone(),
                    side: signal.side,
                    signal_time_ms: last_candle.close_time_ms,
                    score: signal.score,
                    size_factor: signal.size_factor,
                }),
            },
            None => EvaluationOutcome::Rejected(EvaluationRejectReason::Signal(
                SignalRejectReason::InsufficientCandles {
                    have: 0,
                    need: config.model.min_bars(),
                },
            )),
        },
        SignalEvaluation::Rejected(reason) => {
            EvaluationOutcome::Rejected(EvaluationRejectReason::Signal(reason))
        }
    };

    Evaluation {
        symbol: config.symbol,
        interval: config.interval,
        market_slug,
        outcome,
    }
}

/// 基于 registry 判断下一期 market 是否可做，并完成一次候选评估。
///
/// 这里不读取盘口，只检查下一期 market 是否已经进入 registry 且可下单。
pub fn from_registry(
    config: &Config,
    candles: &[Candle],
    registry: &Arc<RwLock<MarketRegistry>>,
) -> Result<Option<Candidate>> {
    let market_slug = next_slug(config.symbol, config.interval)?;
    let exists = registry
        .read()
        .map_err(|_| PolyfillError::internal_simple("Polymarket market registry 读锁已被污染"))?
        .get(&market_slug)
        .is_some();

    if !exists {
        return Ok(None);
    }

    Ok(candidate(config, candles, Some(&market_slug)))
}

pub fn evaluate_from_registry(
    config: &Config,
    candles: &[Candle],
    registry: &Arc<RwLock<MarketRegistry>>,
) -> Result<Evaluation> {
    let market_slug = next_slug(config.symbol, config.interval)?;
    let exists = registry
        .read()
        .map_err(|_| PolyfillError::internal_simple("Polymarket market registry 读锁已被污染"))?
        .get(&market_slug)
        .is_some();

    if !exists {
        return Ok(Evaluation {
            symbol: config.symbol,
            interval: config.interval,
            market_slug,
            outcome: EvaluationOutcome::Rejected(EvaluationRejectReason::MarketMissing),
        });
    }

    Ok(evaluate(config, candles, Some(&market_slug)))
}

pub fn evaluate_from_input(
    config: &Config,
    binance: &binance::Client,
    registry: &Arc<RwLock<MarketRegistry>>,
) -> Result<Evaluation> {
    let candles = binance.candles(config.symbol, config.interval, config.model.min_bars());
    let mut evaluation = evaluate_from_registry(config, &candles, registry)?;

    let EvaluationOutcome::Candidate(candidate) = &mut evaluation.outcome else {
        return Ok(evaluation);
    };

    match background_action(config, binance, candidate.side) {
        Some(BackgroundAction::Allow) => {}
        Some(BackgroundAction::Reduce) => {
            candidate.size_factor *= background_profile(config.interval).reduce_factor;
        }
        Some(BackgroundAction::Block) => {
            evaluation.outcome =
                EvaluationOutcome::Rejected(EvaluationRejectReason::BackgroundBlocked);
        }
        None => {
            evaluation.outcome =
                EvaluationOutcome::Rejected(EvaluationRejectReason::BackgroundDataUnavailable);
        }
    }

    Ok(evaluation)
}

/// 为一组策略配置订阅运行时需要的 Binance `kline` 输入。
///
/// 当前策略只依赖 `(symbol, interval)` 对应的 candle 序列，
/// 因此这里只订阅 `kline`，不额外订阅 `bookTicker`。
///
/// 这里先对 `(symbol, interval)` 做去重，
/// 避免同一组配置里出现重复资产或重复周期时重复下发控制消息。
pub async fn subscribe_inputs(binance: &binance::Client, configs: &[Config]) -> Result<()> {
    let inputs = build_inputs(configs);
    binance.subscribe_klines(&inputs.klines).await?;
    binance.start_background_cache(&inputs.background).await
}

/// 收集当前策略组真正需要的运行时输入计划。
///
/// 这里保持实现最简单：
/// - 按配置遍历一次；
/// - 主周期与背景周期一起收集；
/// - 保留首次出现的输入项。
fn build_inputs(configs: &[Config]) -> Inputs {
    let mut inputs = Inputs {
        klines: Vec::with_capacity(configs.len()),
        background: Vec::with_capacity(configs.len()),
    };

    for config in configs {
        push_unique(&mut inputs.klines, (config.symbol, config.interval));
        if config.interval == Interval::M5 {
            push_unique(&mut inputs.klines, (config.symbol, Interval::M15));
        }
        push_unique(
            &mut inputs.background,
            (config.symbol, binance::BackgroundInterval::H1),
        );
        if config.interval == Interval::M15 {
            push_unique(
                &mut inputs.background,
                (config.symbol, binance::BackgroundInterval::H4),
            );
        }
    }

    inputs
}

fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}

fn background_profile(interval: Interval) -> BackgroundProfile {
    if interval == Interval::M15 {
        BackgroundProfile {
            fast_lookback: 8,
            slow_lookback: 6,
            block_fast_pct: BACKGROUND_BLOCK_1H_PCT_15M,
            block_slow_pct: BACKGROUND_BLOCK_4H_PCT_15M,
            reduce_fast_pct: BACKGROUND_REDUCE_1H_PCT_15M,
            reduce_slow_pct: BACKGROUND_REDUCE_4H_PCT_15M,
            reduce_factor: BACKGROUND_REDUCE_FACTOR_15M,
        }
    } else {
        BackgroundProfile {
            fast_lookback: BACKGROUND_LOOKBACK_15M,
            slow_lookback: BACKGROUND_LOOKBACK_1H,
            block_fast_pct: BACKGROUND_BLOCK_15M_PCT,
            block_slow_pct: BACKGROUND_BLOCK_1H_PCT,
            reduce_fast_pct: BACKGROUND_REDUCE_15M_PCT,
            reduce_slow_pct: BACKGROUND_REDUCE_1H_PCT,
            reduce_factor: BACKGROUND_REDUCE_FACTOR,
        }
    }
}

fn background_action(
    config: &Config,
    binance: &binance::Client,
    side: Side,
) -> Option<BackgroundAction> {
    let profile = background_profile(config.interval);
    let fast = match config.interval {
        Interval::M5 => {
            let closed_m15 = closed_cached_candles(binance, config.symbol, Interval::M15, 128);
            match tail(&closed_m15, profile.fast_lookback) {
                Some(candles) => candles.to_vec(),
                None => return None,
            }
        }
        Interval::M15 => match tail(
            &closed_cached_background(
                binance,
                config.symbol,
                binance::BackgroundInterval::H1,
                profile.fast_lookback + 4,
            ),
            profile.fast_lookback,
        ) {
            Some(candles) => candles.to_vec(),
            None => return None,
        },
    };

    let slow = match config.interval {
        Interval::M5 => match tail(
            &closed_cached_background(
                binance,
                config.symbol,
                binance::BackgroundInterval::H1,
                profile.slow_lookback + 4,
            ),
            profile.slow_lookback,
        ) {
            Some(candles) => candles.to_vec(),
            None => return None,
        },
        Interval::M15 => match tail(
            &closed_cached_background(
                binance,
                config.symbol,
                binance::BackgroundInterval::H4,
                profile.slow_lookback + 4,
            ),
            profile.slow_lookback,
        ) {
            Some(candles) => candles.to_vec(),
            None => return None,
        },
    };

    evaluate_regime_filter(side, &fast, &slow, profile)
}

fn evaluate_regime_filter(
    side: Side,
    fast: &[Candle],
    slow: &[Candle],
    profile: BackgroundProfile,
) -> Option<BackgroundAction> {
    let change_fast = percent_change(fast.first()?.close, fast.last()?.close);
    let change_slow = percent_change(slow.first()?.close, slow.last()?.close);

    Some(match side {
        Side::Up => {
            if change_fast <= -profile.block_fast_pct && change_slow <= -profile.block_slow_pct {
                BackgroundAction::Block
            } else if change_fast <= -profile.reduce_fast_pct
                || change_slow <= -profile.reduce_slow_pct
            {
                BackgroundAction::Reduce
            } else {
                BackgroundAction::Allow
            }
        }
        Side::Down => {
            if change_fast >= profile.block_fast_pct && change_slow >= profile.block_slow_pct {
                BackgroundAction::Block
            } else if change_fast >= profile.reduce_fast_pct
                || change_slow >= profile.reduce_slow_pct
            {
                BackgroundAction::Reduce
            } else {
                BackgroundAction::Allow
            }
        }
    })
}

fn tail<T>(items: &[T], count: usize) -> Option<&[T]> {
    if items.len() < count {
        return None;
    }

    Some(&items[items.len() - count..])
}

fn closed_cached_candles(
    binance: &binance::Client,
    symbol: Symbol,
    interval: Interval,
    limit: usize,
) -> Vec<Candle> {
    binance
        .candles(symbol, interval, limit)
        .into_iter()
        .filter(|candle| candle.is_closed)
        .collect()
}

fn closed_cached_background(
    binance: &binance::Client,
    symbol: Symbol,
    interval: binance::BackgroundInterval,
    limit: usize,
) -> Vec<Candle> {
    binance
        .cached_background(symbol, interval, limit)
        .into_iter()
        .filter(|candle| candle.is_closed)
        .collect()
}

fn percent_change(first: f64, last: f64) -> f64 {
    if first == 0.0 {
        0.0
    } else {
        (last - first) / first
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use super::*;
    use crate::polymarket::utils::crypto_market::next_slug;
    use crate::strategy::crypto_reversal::model::Candle;
    use polymarket_client_sdk::types::U256;

    fn config() -> Config {
        Config {
            symbol: Symbol::Eth,
            interval: Interval::M5,
            model: ModelConfig {
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
            },
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
    fn candidate_uses_next_market_slug_for_up_signal() {
        let config = config();
        let candles = candles(&[100.0, 101.0, 100.0, 99.0, 98.0, 90.0]);
        let market_slug = next_slug(Symbol::Eth, Interval::M5).expect("slug should resolve");

        let Some(result) = candidate(&config, &candles, Some(&market_slug)) else {
            panic!("expected candidate");
        };
        assert_eq!(result.market_slug, market_slug);
        assert_eq!(result.side, Side::Up);
    }

    #[test]
    fn candidate_returns_none_when_market_is_missing() {
        let config = config();
        let candles = candles(&[100.0, 99.0, 100.0, 101.0, 102.0, 110.0]);

        assert_eq!(candidate(&config, &candles, None), None);
    }

    #[test]
    fn inputs_dedup_runtime_inputs() {
        let configs = vec![
            Config {
                symbol: Symbol::Eth,
                interval: Interval::M5,
                ..config()
            },
            Config {
                symbol: Symbol::Eth,
                interval: Interval::M5,
                ..config()
            },
            Config {
                symbol: Symbol::Eth,
                interval: Interval::M15,
                ..config()
            },
            Config {
                symbol: Symbol::Btc,
                interval: Interval::M5,
                ..config()
            },
        ];

        let inputs = build_inputs(&configs);
        assert_eq!(
            inputs.klines,
            vec![
                (Symbol::Eth, Interval::M5),
                (Symbol::Eth, Interval::M15),
                (Symbol::Btc, Interval::M5),
                (Symbol::Btc, Interval::M15),
            ]
        );
        assert_eq!(
            inputs.background,
            vec![
                (Symbol::Eth, binance::BackgroundInterval::H1),
                (Symbol::Eth, binance::BackgroundInterval::H4),
                (Symbol::Btc, binance::BackgroundInterval::H1),
            ]
        );
    }

    #[test]
    fn regime_filter_blocks_strong_adverse_up_trend() {
        let profile = background_profile(Interval::M5);
        let fast = candles(&[100.0, 99.0, 98.5, 98.0, 97.5, 97.0, 96.5, 96.0]);
        let slow = candles(&[100.0, 99.0, 98.0, 97.0, 96.0, 95.0]);

        assert_eq!(
            evaluate_regime_filter(Side::Up, &fast, &slow, profile),
            Some(BackgroundAction::Block)
        );
    }

    #[test]
    fn regime_filter_reduces_soft_adverse_up_trend() {
        let profile = background_profile(Interval::M5);
        let fast = candles(&[100.0, 99.9, 99.8, 99.7, 99.6, 99.55, 99.5, 99.49]);
        let slow = candles(&[100.0, 99.95, 99.9, 99.85, 99.8, 99.75]);

        assert_eq!(
            evaluate_regime_filter(Side::Up, &fast, &slow, profile),
            Some(BackgroundAction::Reduce)
        );
    }

    #[tokio::test]
    async fn from_registry_returns_none_when_market_is_missing() {
        let registry = Arc::new(RwLock::new(MarketRegistry::new()));
        let candles = candles(&[100.0, 101.0, 100.0, 99.0, 98.0, 90.0]);

        let candidate =
            from_registry(&config(), &candles, &registry).expect("lookup should succeed");

        assert!(candidate.is_none());
    }

    #[tokio::test]
    async fn from_registry_resolves_next_market() {
        let registry = Arc::new(RwLock::new(MarketRegistry::new()));
        registry
            .write()
            .expect("registry lock should be writable")
            .insert(
                next_slug(Symbol::Eth, Interval::M5).expect("slug should resolve"),
                [U256::from(1u64), U256::from(2u64)],
            );
        let candles = candles(&[100.0, 101.0, 100.0, 99.0, 98.0, 90.0]);

        let candidate = from_registry(&config(), &candles, &registry)
            .expect("lookup should succeed")
            .expect("market should exist");

        assert!(candidate.market_slug.starts_with("eth-updown-5m-"));
    }
}
