use crate::binance;
use crate::errors::{PolyfillError, Result};
use crate::polymarket::market_registry::MarketRegistry;
use crate::polymarket::orderbook_stream;
use crate::polymarket::rtds_stream;
use crate::polymarket::utils::crypto_market::current_slug;
use crate::types::crypto::{Interval, Symbol};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

const MIN_VALID_CHANGE_PRICE: Decimal = Decimal::from_parts(5, 0, 0, false, 2);
const MAX_CHANGE_PCT: Decimal = Decimal::from_parts(100, 0, 0, false, 0);
const PRICE_DP: u32 = 4;
const SIZE_DP: u32 = 2;
const SLOPE_WINDOW: u64 = 5;
const Z_SCORE_WINDOW: u64 = 120;
const VELOCITY_WINDOW: u64 = 5;
const SIGMA_WINDOW: u64 = 30;
const CHANGE_WINDOWS: [u64; 2] = [30, 60];

const CSV_HEADER: &str = "timestamp,binance_mid_price,chainlink_price,\
spread_binance_chainlink,spread_delta,chainlink_start_delta,\
up_bid_price,up_bid_size,up_ask_price,up_ask_size,\
down_bid_price,down_bid_size,down_ask_price,down_ask_size,\
z_score,vel_spread,up_mid_price_slope,binance_sigma,\
chainlink_change_30s_pct,chainlink_change_60s_pct,chainlink_run";

fn quantize_price(value: Decimal) -> Decimal {
    value.round_dp(PRICE_DP)
}

fn quantize_size(value: Decimal) -> Decimal {
    value.round_dp(SIZE_DP)
}

fn sqrt_decimal(value: Decimal) -> Decimal {
    if value <= Decimal::ZERO {
        return Decimal::ZERO;
    }
    let f = value.to_f64().unwrap_or(0.0).sqrt();
    Decimal::from_f64(f).unwrap_or(Decimal::ZERO)
}

struct TimedValue {
    ts: i64,
    value: Decimal,
}

struct RollingBuffer {
    data: VecDeque<TimedValue>,
    max_seconds: u64,
}

impl RollingBuffer {
    fn new(max_seconds: u64) -> Self {
        Self {
            data: VecDeque::with_capacity((max_seconds as usize) * 2),
            max_seconds,
        }
    }

    fn push(&mut self, ts: i64, value: Decimal) {
        self.data.push_back(TimedValue { ts, value });
        let cutoff = ts - (self.max_seconds as i64) * 1_000;
        while self.data.front().is_some_and(|p| p.ts < cutoff) {
            self.data.pop_front();
        }
    }

    fn latest(&self) -> Option<Decimal> {
        self.data.back().map(|p| p.value)
    }

    fn value_at(&self, now: i64, seconds_ago: u64) -> Option<Decimal> {
        let target = now - (seconds_ago as i64) * 1_000;
        self.data
            .iter()
            .filter(|p| p.ts <= target)
            .last()
            .map(|p| p.value)
    }

    fn mean_std(&self, now: i64, seconds: u64) -> (Decimal, Decimal) {
        let cutoff = now - (seconds as i64) * 1_000;
        let values: Vec<Decimal> = self
            .data
            .iter()
            .filter(|p| p.ts >= cutoff)
            .map(|p| p.value)
            .collect();
        if values.is_empty() {
            return (Decimal::ZERO, Decimal::ZERO);
        }
        let n = Decimal::from(values.len() as i64);
        let mean = values.iter().copied().sum::<Decimal>() / n;
        if values.len() < 2 {
            return (mean, Decimal::ZERO);
        }
        let variance = values
            .iter()
            .map(|v| {
                let d = *v - mean;
                d * d
            })
            .sum::<Decimal>()
            / Decimal::from((values.len() - 1) as i64);
        (mean, sqrt_decimal(variance))
    }

    fn clear(&mut self) {
        self.data.clear();
    }
}

pub struct SnapshotWriter {
    symbol: Symbol,
    interval: Interval,
    output_dir: PathBuf,

    binance: Arc<binance::Client>,
    chainlink: Arc<rtds_stream::Client>,
    orderbook: Arc<orderbook_stream::Client>,
    registry: Arc<RwLock<MarketRegistry>>,

    binance_prices: RollingBuffer,
    chainlink_prices: RollingBuffer,
    spreads: RollingBuffer,
    market_mid_prices: RollingBuffer,

    last_snapshot_spread: Decimal,
    period_start_chainlink: Option<Decimal>,
    last_chainlink_direction: i32,
    chainlink_run: i32,

    current_market: Option<(String, File)>,
}

impl SnapshotWriter {
    pub fn new(
        symbol: Symbol,
        interval: Interval,
        output_dir: PathBuf,
        binance: Arc<binance::Client>,
        chainlink: Arc<rtds_stream::Client>,
        orderbook: Arc<orderbook_stream::Client>,
        registry: Arc<RwLock<MarketRegistry>>,
    ) -> Self {
        let max_window = *[
            Z_SCORE_WINDOW,
            VELOCITY_WINDOW,
            SIGMA_WINDOW,
            SLOPE_WINDOW,
            *CHANGE_WINDOWS.iter().max().unwrap_or(&60),
        ]
        .iter()
        .max()
        .unwrap_or(&60);
        let cap = max_window + 30;

        Self {
            symbol,
            interval,
            output_dir,
            binance,
            chainlink,
            orderbook,
            registry,
            binance_prices: RollingBuffer::new(cap),
            chainlink_prices: RollingBuffer::new(cap),
            spreads: RollingBuffer::new(cap),
            market_mid_prices: RollingBuffer::new(cap),
            last_snapshot_spread: Decimal::ZERO,
            period_start_chainlink: None,
            last_chainlink_direction: 0,
            chainlink_run: 0,
            current_market: None,
        }
    }

    /// Sample current sources and append a CSV row. Returns true if a row was written.
    pub fn tick(&mut self) -> Result<bool> {
        let market_slug = current_slug(self.symbol, self.interval)?;
        let assets = self
            .registry
            .read()
            .map_err(|_| PolyfillError::internal_simple("Polymarket market registry 读锁已被污染"))?
            .get(&market_slug);
        let Some([up_asset_id, down_asset_id]) = assets else {
            return Ok(false);
        };

        if self.current_market.as_ref().map(|(s, _)| s.as_str()) != Some(market_slug.as_str()) {
            self.reset_period();
            self.current_market = None;
        }

        let binance_book = self.binance.get(self.symbol.as_binance_symbol());
        let chainlink_price = self.chainlink.latest(self.symbol);
        let bid = self.orderbook.best_bid(&up_asset_id);
        let ask = self.orderbook.best_ask(&up_asset_id);
        let down_bid = self.orderbook.best_bid(&down_asset_id);
        let down_ask = self.orderbook.best_ask(&down_asset_id);
        let market_mid = self.orderbook.mid(&up_asset_id);
        let ts_ms = Utc::now().timestamp_millis();

        if let Some(book) = binance_book {
            self.update_binance(ts_ms, book.mid());
        }
        if let Some(price) = chainlink_price {
            self.update_chainlink(price.timestamp, price.value);
        }
        if let Some(mid) = market_mid {
            self.market_mid_prices.push(ts_ms, quantize_price(mid));
        }

        let binance_mid = self.binance_prices.latest().unwrap_or(Decimal::ZERO);
        let chainlink_p = self.chainlink_prices.latest().unwrap_or(Decimal::ZERO);
        let up_bid_price = bid.map(|l| l.price).unwrap_or(Decimal::ZERO);
        let up_ask_price = ask.map(|l| l.price).unwrap_or(Decimal::ZERO);

        if up_bid_price <= Decimal::ZERO
            && up_ask_price <= Decimal::ZERO
            && binance_mid <= Decimal::ZERO
        {
            return Ok(false);
        }

        let spread = self.spreads.latest().unwrap_or(Decimal::ZERO);
        let spread_delta = spread - self.last_snapshot_spread;
        self.last_snapshot_spread = spread;
        let chainlink_start_delta =
            chainlink_p - self.period_start_chainlink.unwrap_or(chainlink_p);

        let (z_score, vel_spread) = self.spread_stats(ts_ms, spread);
        let slope = self.up_mid_slope(ts_ms);
        let sigma = self.binance_prices.mean_std(ts_ms, SIGMA_WINDOW).1;
        let changes = self.chainlink_changes(ts_ms, chainlink_p);

        let chainlink_run = self.chainlink_run;
        let path = self
            .output_dir
            .join(self.symbol.as_slug())
            .join(self.interval.as_slug())
            .join(format!("{market_slug}.csv"));
        let file = self.ensure_file(&market_slug, &path)?;

        let timestamp = DateTime::<Utc>::from_timestamp_millis(ts_ms)
            .unwrap_or_else(Utc::now)
            .format("%Y-%m-%d %H:%M:%S%.3f");

        // Order MUST match CSV_HEADER (21 columns).
        writeln!(
            file,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            timestamp,
            quantize_price(binance_mid),
            quantize_price(chainlink_p),
            quantize_price(spread),
            quantize_price(spread_delta),
            quantize_price(chainlink_start_delta),
            quantize_price(up_bid_price),
            quantize_size(bid.map(|l| l.size).unwrap_or(Decimal::ZERO)),
            quantize_price(up_ask_price),
            quantize_size(ask.map(|l| l.size).unwrap_or(Decimal::ZERO)),
            quantize_price(down_bid.map(|l| l.price).unwrap_or(Decimal::ZERO)),
            quantize_size(down_bid.map(|l| l.size).unwrap_or(Decimal::ZERO)),
            quantize_price(down_ask.map(|l| l.price).unwrap_or(Decimal::ZERO)),
            quantize_size(down_ask.map(|l| l.size).unwrap_or(Decimal::ZERO)),
            z_score.round_dp(4),
            vel_spread.round_dp(6),
            slope.round_dp(6),
            sigma.round_dp(4),
            changes[0],
            changes[1],
            chainlink_run,
        )
        .map_err(|e| {
            PolyfillError::internal_simple(format!("写 CSV 失败 {}: {}", path.display(), e))
        })?;

        Ok(true)
    }

    fn reset_period(&mut self) {
        self.last_snapshot_spread = Decimal::ZERO;
        self.period_start_chainlink = None;
        self.last_chainlink_direction = 0;
        self.chainlink_run = 0;
        self.market_mid_prices.clear();
    }

    fn update_binance(&mut self, ts: i64, mid: Decimal) {
        self.binance_prices.push(ts, quantize_price(mid));
        self.refresh_spread(ts);
    }

    fn update_chainlink(&mut self, ts: i64, price: Decimal) {
        let price = quantize_price(price);
        if let Some(last) = self.chainlink_prices.latest() {
            let direction = match price.cmp(&last) {
                std::cmp::Ordering::Greater => 1,
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
            };
            if direction != 0 {
                if direction == self.last_chainlink_direction {
                    self.chainlink_run += 1;
                } else {
                    self.chainlink_run = 1;
                    self.last_chainlink_direction = direction;
                }
            }
        }
        if self.period_start_chainlink.is_none() {
            self.period_start_chainlink = Some(price);
        }
        self.chainlink_prices.push(ts, price);
        self.refresh_spread(ts);
    }

    fn refresh_spread(&mut self, ts: i64) {
        if let (Some(b), Some(c)) = (self.binance_prices.latest(), self.chainlink_prices.latest())
        {
            self.spreads.push(ts, quantize_price(b - c));
        }
    }

    fn spread_stats(&self, ts: i64, spread: Decimal) -> (Decimal, Decimal) {
        let (mean, std) = self.spreads.mean_std(ts, Z_SCORE_WINDOW);
        let z = if std.is_zero() {
            Decimal::ZERO
        } else {
            (spread - mean) / std
        };
        let prev = self
            .spreads
            .value_at(ts, VELOCITY_WINDOW)
            .unwrap_or(spread);
        let vel = if VELOCITY_WINDOW == 0 {
            Decimal::ZERO
        } else {
            (spread - prev) / Decimal::from(VELOCITY_WINDOW as i64)
        };
        (z, vel)
    }

    fn up_mid_slope(&self, ts: i64) -> Decimal {
        let mid = self.market_mid_prices.latest().unwrap_or(Decimal::ZERO);
        let prev = self
            .market_mid_prices
            .value_at(ts, SLOPE_WINDOW)
            .unwrap_or(mid);
        if SLOPE_WINDOW == 0 {
            Decimal::ZERO
        } else {
            (mid - prev) / Decimal::from(SLOPE_WINDOW as i64)
        }
    }

    fn chainlink_changes(&self, ts: i64, current: Decimal) -> [Decimal; 2] {
        let mut out = [Decimal::ZERO; 2];
        for (i, window) in CHANGE_WINDOWS.iter().enumerate() {
            let old = self
                .chainlink_prices
                .value_at(ts, *window)
                .unwrap_or(current);
            if old >= MIN_VALID_CHANGE_PRICE && current >= MIN_VALID_CHANGE_PRICE {
                let raw = ((current - old) / old) * Decimal::new(100, 0);
                out[i] = raw.max(-MAX_CHANGE_PCT).min(MAX_CHANGE_PCT).round_dp(4);
            }
        }
        out
    }

    fn ensure_file(&mut self, slug: &str, path: &Path) -> Result<&mut File> {
        if self.current_market.is_none() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    PolyfillError::internal_simple(format!(
                        "创建 CSV 目录失败 {}: {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
            let need_header = !path.metadata().is_ok_and(|m| m.len() > 0);
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| {
                    PolyfillError::internal_simple(format!(
                        "打开 CSV 失败 {}: {}",
                        path.display(),
                        e
                    ))
                })?;
            if need_header {
                writeln!(file, "{CSV_HEADER}").map_err(|e| {
                    PolyfillError::internal_simple(format!(
                        "写 CSV header 失败 {}: {}",
                        path.display(),
                        e
                    ))
                })?;
            }
            self.current_market = Some((slug.to_owned(), file));
        }
        Ok(&mut self.current_market.as_mut().unwrap().1)
    }
}
