use crate::binance;
use crate::errors::{PolyfillError, Result};
use crate::polymarket::market_registry::MarketRegistry;
use crate::polymarket::orderbook_stream;
use crate::polymarket::rtds_stream;
use crate::polymarket::utils::crypto_market::current_slug;
use crate::types::crypto::{Interval, Symbol};
use chrono::Utc;
use rust_decimal::Decimal;
use std::collections::{HashMap, VecDeque};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, RwLock};

const MIN_VALID_CHANGE_PRICE: Decimal = Decimal::from_parts(5, 0, 0, false, 2);
const MAX_CHANGE_PCT: Decimal = Decimal::from_parts(100, 0, 0, false, 0);
const PRICE_SCALE_DP: u32 = 4;
const SIZE_SCALE_DP: u32 = 2;
const SLOPE_WINDOW_SECS: u64 = 5;
const Z_SCORE_WINDOW_SECS: u64 = 120;
const VELOCITY_WINDOW_SECS: u64 = 5;
const SIGMA_WINDOW_SECS: u64 = 30;
const CHANGE_WINDOWS_SECS: [u64; 2] = [30, 60];

fn quantize_price(value: Decimal) -> Decimal {
    value.round_dp(PRICE_SCALE_DP)
}

fn quantize_size(value: Decimal) -> Decimal {
    value.round_dp(SIZE_SCALE_DP)
}

#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotWrite {
    pub ts_ms: i64,
    pub symbol: Symbol,
    pub interval: Interval,
    pub market_slug: String,
    pub binance_mid_price: Decimal,
    pub chainlink_price: Decimal,
    pub up_bid_price: Decimal,
    pub up_bid_size: Decimal,
    pub up_ask_price: Decimal,
    pub up_ask_size: Decimal,
    pub down_bid_price: Decimal,
    pub down_bid_size: Decimal,
    pub down_ask_price: Decimal,
    pub down_ask_size: Decimal,
    pub spread_binance_chainlink: Decimal,
    pub spread_delta: Decimal,
    pub chainlink_start_delta: Decimal,
    pub z_score: Decimal,
    pub vel_spread: Decimal,
    pub up_mid_price_slope: Decimal,
    pub binance_sigma: Decimal,
    pub changes: Vec<Decimal>,
    pub chainlink_run: i32,
}

#[derive(Debug, Clone)]
struct TimedValue {
    timestamp_ms: i64,
    value: Decimal,
}

#[derive(Debug, Clone)]
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

    fn push(&mut self, timestamp_ms: i64, value: Decimal) {
        self.data.push_back(TimedValue { timestamp_ms, value });
        let cutoff_ms = timestamp_ms - (self.max_seconds as i64 * 1_000);

        while let Some(front) = self.data.front() {
            if front.timestamp_ms < cutoff_ms {
                self.data.pop_front();
            } else {
                break;
            }
        }
    }

    fn latest(&self) -> Option<Decimal> {
        self.data.back().map(|point| point.value)
    }

    fn value_at(&self, now_ms: i64, seconds_ago: u64) -> Option<Decimal> {
        let target_ms = now_ms - (seconds_ago as i64 * 1_000);
        self.data
            .iter()
            .filter(|point| point.timestamp_ms <= target_ms)
            .last()
            .map(|point| point.value)
    }

    fn values_since(&self, now_ms: i64, seconds: u64) -> impl Iterator<Item = Decimal> + '_ {
        let cutoff_ms = now_ms - (seconds as i64 * 1_000);
        self.data
            .iter()
            .filter(move |point| point.timestamp_ms >= cutoff_ms)
            .map(|point| point.value)
    }

    fn mean(&self, now_ms: i64, seconds: u64) -> Decimal {
        let values: Vec<Decimal> = self.values_since(now_ms, seconds).collect();
        if values.is_empty() {
            return Decimal::ZERO;
        }

        let sum: Decimal = values.iter().copied().sum();
        sum / Decimal::from(values.len() as i64)
    }

    fn std(&self, now_ms: i64, seconds: u64) -> Decimal {
        let values: Vec<Decimal> = self.values_since(now_ms, seconds).collect();
        if values.len() < 2 {
            return Decimal::ZERO;
        }

        let mean = self.mean(now_ms, seconds);
        let variance = values
            .iter()
            .map(|value| {
                let diff = *value - mean;
                diff * diff
            })
            .sum::<Decimal>()
            / Decimal::from((values.len() - 1) as i64);

        sqrt_decimal(variance)
    }
}

fn sqrt_decimal(value: Decimal) -> Decimal {
    if value <= Decimal::ZERO {
        return Decimal::ZERO;
    }

    let mut guess = value / Decimal::TWO;
    for _ in 0..20 {
        let next = (guess + value / guess) / Decimal::TWO;
        if (next - guess).abs() < Decimal::new(1, 10) {
            return next;
        }
        guess = next;
    }

    guess
}

#[derive(Debug, Clone)]
struct SnapshotState {
    binance_prices: RollingBuffer,
    chainlink_prices: RollingBuffer,
    spreads: RollingBuffer,
    market_mid_prices: RollingBuffer,
    last_snapshot_spread: Decimal,
    period_start_chainlink_price: Option<Decimal>,
    last_direction: i32,
    run_count: i32,
}

impl SnapshotState {
    fn new() -> Self {
        let max_change = CHANGE_WINDOWS_SECS.iter().copied().max().unwrap_or(60);
        let max_window = *[
            Z_SCORE_WINDOW_SECS,
            VELOCITY_WINDOW_SECS,
            SIGMA_WINDOW_SECS,
            max_change,
            SLOPE_WINDOW_SECS,
        ]
        .iter()
        .max()
        .unwrap_or(&60);

        Self {
            binance_prices: RollingBuffer::new(max_window + 30),
            chainlink_prices: RollingBuffer::new(max_window + 30),
            spreads: RollingBuffer::new(max_window + 30),
            market_mid_prices: RollingBuffer::new(max_window + 30),
            last_snapshot_spread: Decimal::ZERO,
            period_start_chainlink_price: None,
            last_direction: 0,
            run_count: 0,
        }
    }

    fn reset_period(&mut self) {
        self.last_snapshot_spread = Decimal::ZERO;
        self.period_start_chainlink_price = None;
        self.last_direction = 0;
        self.run_count = 0;
    }

    fn update_binance(&mut self, timestamp_ms: i64, mid: Decimal) {
        self.binance_prices.push(timestamp_ms, quantize_price(mid));
        self.update_spread(timestamp_ms);
    }

    fn update_chainlink(&mut self, timestamp_ms: i64, price: Decimal) {
        let price = quantize_price(price);

        if let Some(last_price) = self.chainlink_prices.latest() {
            let direction = if price > last_price {
                1
            } else if price < last_price {
                -1
            } else {
                0
            };

            if direction != 0 {
                if direction == self.last_direction {
                    self.run_count += 1;
                } else {
                    self.run_count = 1;
                    self.last_direction = direction;
                }
            }
        }

        if self.period_start_chainlink_price.is_none() {
            self.period_start_chainlink_price = Some(price);
        }

        self.chainlink_prices.push(timestamp_ms, price);
        self.update_spread(timestamp_ms);
    }

    fn update_market_mid(&mut self, timestamp_ms: i64, mid: Option<Decimal>) {
        if let Some(mid) = mid {
            self.market_mid_prices.push(timestamp_ms, quantize_price(mid));
        }
    }

    fn build_write(
        &mut self,
        symbol: Symbol,
        interval: Interval,
        market_slug: String,
        ts_ms: i64,
        up_bid_price: Decimal,
        up_bid_size: Decimal,
        up_ask_price: Decimal,
        up_ask_size: Decimal,
        down_bid_price: Decimal,
        down_bid_size: Decimal,
        down_ask_price: Decimal,
        down_ask_size: Decimal,
    ) -> SnapshotWrite {
        let binance = self.binance_prices.latest().unwrap_or(Decimal::ZERO);
        let chainlink_price = self.chainlink_prices.latest().unwrap_or(Decimal::ZERO);
        let spread = self.spreads.latest().unwrap_or(Decimal::ZERO);
        let start_delta =
            chainlink_price - self.period_start_chainlink_price.unwrap_or(chainlink_price);
        let delta = spread - self.last_snapshot_spread;
        self.last_snapshot_spread = spread;

        let z_score = {
            let mean = self.spreads.mean(ts_ms, Z_SCORE_WINDOW_SECS);
            let std = self.spreads.std(ts_ms, Z_SCORE_WINDOW_SECS);
            if std.is_zero() {
                Decimal::ZERO
            } else {
                (spread - mean) / std
            }
        };

        let vel_spread = if VELOCITY_WINDOW_SECS == 0 {
            Decimal::ZERO
        } else {
            let spread_prev = self
                .spreads
                .value_at(ts_ms, VELOCITY_WINDOW_SECS)
                .unwrap_or(spread);
            (spread - spread_prev) / Decimal::from(VELOCITY_WINDOW_SECS as i64)
        };

        let market_mid = self.market_mid_prices.latest().unwrap_or(Decimal::ZERO);
        let market_prev = self
            .market_mid_prices
            .value_at(ts_ms, SLOPE_WINDOW_SECS)
            .unwrap_or(market_mid);
        let slope_price =
            (market_mid - market_prev) / Decimal::from(SLOPE_WINDOW_SECS as i64);

        let changes = CHANGE_WINDOWS_SECS
            .iter()
            .map(|window| {
                let old_reference = self
                    .chainlink_prices
                    .value_at(ts_ms, *window)
                    .unwrap_or(chainlink_price);
                if old_reference >= MIN_VALID_CHANGE_PRICE
                    && chainlink_price >= MIN_VALID_CHANGE_PRICE
                {
                    let raw = ((chainlink_price - old_reference) / old_reference)
                        * Decimal::new(100, 0);
                    raw.max(-MAX_CHANGE_PCT).min(MAX_CHANGE_PCT).round_dp(4)
                } else {
                    Decimal::ZERO
                }
            })
            .collect();

        SnapshotWrite {
            ts_ms,
            symbol,
            interval,
            market_slug,
            binance_mid_price: quantize_price(binance),
            chainlink_price: quantize_price(chainlink_price),
            up_bid_price: quantize_price(up_bid_price),
            up_bid_size: quantize_size(up_bid_size),
            up_ask_price: quantize_price(up_ask_price),
            up_ask_size: quantize_size(up_ask_size),
            down_bid_price: quantize_price(down_bid_price),
            down_bid_size: quantize_size(down_bid_size),
            down_ask_price: quantize_price(down_ask_price),
            down_ask_size: quantize_size(down_ask_size),
            spread_binance_chainlink: quantize_price(spread),
            spread_delta: quantize_price(delta),
            chainlink_start_delta: quantize_price(start_delta),
            z_score: z_score.round_dp(4),
            vel_spread: vel_spread.round_dp(6),
            up_mid_price_slope: slope_price.round_dp(6),
            binance_sigma: self.binance_prices.std(ts_ms, SIGMA_WINDOW_SECS).round_dp(4),
            changes,
            chainlink_run: self.run_count,
        }
    }

    fn update_spread(&mut self, timestamp_ms: i64) {
        if let (Some(binance), Some(chainlink_price)) =
            (self.binance_prices.latest(), self.chainlink_prices.latest())
        {
            self.spreads
                .push(timestamp_ms, quantize_price(binance - chainlink_price));
        }
    }
}

pub struct Snapshot {
    binance: Arc<binance::Client>,
    chainlink: Arc<rtds_stream::Client>,
    orderbook: Arc<orderbook_stream::Client>,
    registry: Arc<RwLock<MarketRegistry>>,
    states: HashMap<(Symbol, Interval), SnapshotState>,
    current_slugs: HashMap<(Symbol, Interval), String>,
}

impl Snapshot {
    pub fn new(
        binance: Arc<binance::Client>,
        chainlink: Arc<rtds_stream::Client>,
        orderbook: Arc<orderbook_stream::Client>,
        registry: Arc<RwLock<MarketRegistry>>,
    ) -> Self {
        Self {
            binance,
            chainlink,
            orderbook,
            registry,
            states: HashMap::new(),
            current_slugs: HashMap::new(),
        }
    }

    pub fn snapshot(&mut self, symbol: Symbol, interval: Interval) -> Result<Option<SnapshotWrite>> {
        let market_slug = current_slug(symbol, interval)?;
        let market = self
            .registry
            .read()
            .map_err(|_| PolyfillError::internal_simple("Polymarket market registry 读锁已被污染"))?
            .get(&market_slug);

        let Some([up_asset_id, down_asset_id]) = market else {
            return Ok(None);
        };

        let binance_book = self.binance.get(symbol.as_binance_symbol());
        let chainlink_price = self.chainlink.latest(symbol);
        let bid = self.orderbook.best_bid(&up_asset_id);
        let ask = self.orderbook.best_ask(&up_asset_id);
        let down_bid = self.orderbook.best_bid(&down_asset_id);
        let down_ask = self.orderbook.best_ask(&down_asset_id);
        let market_mid = self.orderbook.mid(&up_asset_id);

        let ts_ms = Utc::now().timestamp_millis();
        let key = (symbol, interval);
        let state = self
            .states
            .entry(key)
            .or_insert_with(SnapshotState::new);

        if self.current_slugs.get(&key) != Some(&market_slug) {
            state.reset_period();
            self.current_slugs.insert(key, market_slug.clone());
        }

        if let Some(book) = binance_book {
            state.update_binance(ts_ms, book.mid());
        }

        if let Some(price) = chainlink_price {
            state.update_chainlink(price.timestamp, price.value);
        }

        state.update_market_mid(ts_ms, market_mid);

        let up_bid_price = bid.map(|level| level.price).unwrap_or(Decimal::ZERO);
        let up_bid_size = bid.map(|level| level.size).unwrap_or(Decimal::ZERO);
        let up_ask_price = ask.map(|level| level.price).unwrap_or(Decimal::ZERO);
        let up_ask_size = ask.map(|level| level.size).unwrap_or(Decimal::ZERO);
        let down_bid_price = down_bid.map(|level| level.price).unwrap_or(Decimal::ZERO);
        let down_bid_size = down_bid.map(|level| level.size).unwrap_or(Decimal::ZERO);
        let down_ask_price = down_ask.map(|level| level.price).unwrap_or(Decimal::ZERO);
        let down_ask_size = down_ask.map(|level| level.size).unwrap_or(Decimal::ZERO);

        let record = state.build_write(
            symbol,
            interval,
            market_slug,
            ts_ms,
            up_bid_price,
            up_bid_size,
            up_ask_price,
            up_ask_size,
            down_bid_price,
            down_bid_size,
            down_ask_price,
            down_ask_size,
        );

        if record.up_ask_price <= Decimal::ZERO
            && record.up_bid_price <= Decimal::ZERO
            && record.binance_mid_price <= Decimal::ZERO
        {
            return Ok(None);
        }

        Ok(Some(record))
    }

    pub fn write_csv(
        &mut self,
        symbol: Symbol,
        interval: Interval,
        output_dir: impl AsRef<Path>,
    ) -> Result<Option<SnapshotWrite>> {
        let record = self.snapshot(symbol, interval)?;
        if let Some(record) = &record {
            let path = output_dir
                .as_ref()
                .join(record.symbol.as_slug())
                .join(record.interval.as_slug())
                .join(format!("{}.csv", record.market_slug));
            append_record_row(&path, record, &CHANGE_WINDOWS_SECS)?;
        }
        Ok(record)
    }
}

fn append_record_row(
    path: &Path,
    record: &SnapshotWrite,
    change_windows: &[u64],
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            PolyfillError::internal_simple(format!(
                "Failed to create snapshot CSV parent directory {}: {}",
                parent.display(),
                error
            ))
        })?;
    }

    let has_content = path
        .metadata()
        .map(|metadata| metadata.len() > 0)
        .unwrap_or(false);

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| {
            PolyfillError::internal_simple(format!(
                "Failed to open snapshot CSV file {}: {}",
                path.display(),
                error
            ))
        })?;

    if !has_content {
        writeln!(file, "{}", csv_header(change_windows)).map_err(|error| {
            PolyfillError::internal_simple(format!(
                "Failed to write snapshot CSV header {}: {}",
                path.display(),
                error
            ))
        })?;
    }

    writeln!(file, "{}", format_record_row(record, change_windows)).map_err(|error| {
        PolyfillError::internal_simple(format!(
            "Failed to append snapshot CSV row {}: {}",
            path.display(),
            error
        ))
    })?;

    Ok(())
}

fn csv_header(change_windows: &[u64]) -> String {
    let mut header =
        "timestamp,binance_mid_price,chainlink_price,spread_binance_chainlink,spread_delta,chainlink_start_delta,up_bid_price,up_bid_size,up_ask_price,up_ask_size,down_bid_price,down_bid_size,down_ask_price,down_ask_size,z_score,vel_spread,up_mid_price_slope,binance_sigma"
            .to_string();
    for window in change_windows {
        header.push_str(&format!(",chainlink_change_{}s_pct", window));
    }
    header.push_str(",chainlink_run");
    header
}

fn format_record_row(record: &SnapshotWrite, change_windows: &[u64]) -> String {
    let timestamp = chrono::DateTime::<Utc>::from_timestamp_millis(record.ts_ms)
        .unwrap_or_else(Utc::now)
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();

    let mut line = format!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        timestamp,
        record.binance_mid_price,
        record.chainlink_price,
        record.spread_binance_chainlink,
        record.spread_delta,
        record.chainlink_start_delta,
        record.up_bid_price,
        record.up_bid_size,
        record.up_ask_price,
        record.up_ask_size,
        record.down_bid_price,
        record.down_bid_size,
        record.down_ask_price,
        record.down_ask_size,
        record.z_score,
        record.vel_spread,
        record.up_mid_price_slope,
        record.binance_sigma,
    );

    for (index, _) in change_windows.iter().enumerate() {
        let value = record.changes.get(index).cloned().unwrap_or(Decimal::ZERO);
        line.push_str(&format!(",{}", value));
    }

    line.push_str(&format!(",{}", record.chainlink_run));
    line
}
