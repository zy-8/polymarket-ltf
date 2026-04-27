//! Polymarket 市场注册表、发现与订阅调度。
//!
//! 这个模块负责三件事：
//! - 通过 Gamma API 发现 active 且可订阅 orderbook 的市场；
//! - 维护 `slug -> [up_asset_id, down_asset_id]` 的本地注册表。
//! - 基于本地注册表调度 `orderbook_stream` 的订阅切换。

use crate::errors::{PolyfillError, Result};
use crate::polymarket::orderbook_stream;
use crate::polymarket::utils::crypto_market::{current_slug, next_slug, slugs_for_hours};
use crate::types::crypto::{Interval, Symbol};
use polymarket_client_sdk_v2::gamma::Client as GammaClient;
use polymarket_client_sdk_v2::gamma::types::request::MarketsRequest;
use polymarket_client_sdk_v2::gamma::types::response::Market;
use polymarket_client_sdk_v2::types::{B256, U256};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::task::AbortHandle;
use tokio::time::{Duration, Instant, sleep, sleep_until};
use tracing::warn;

const AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const AUTO_REFRESH_RETRY_INTERVAL: Duration = Duration::from_secs(2);
const AUTO_REFRESH_MAX_RETRIES: usize = 5;
const REGISTRY_HORIZON_HOURS: u32 = 2;
const AUTO_SWITCH_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, Default, Clone)]
pub struct MarketRegistry {
    markets_by_slug: HashMap<String, MarketEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MarketEntry {
    condition_id: B256,
    assets: [U256; 2],
}

impl MarketRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, slug: impl Into<String>, market: [U256; 2]) {
        self.insert_entry(slug, B256::ZERO, market);
    }

    pub fn insert_entry(&mut self, slug: impl Into<String>, condition_id: B256, assets: [U256; 2]) {
        self.markets_by_slug.insert(
            slug.into(),
            MarketEntry {
                condition_id,
                assets,
            },
        );
    }

    pub fn get(&self, slug: &str) -> Option<[U256; 2]> {
        self.markets_by_slug.get(slug).map(|entry| entry.assets)
    }

    pub fn current_market(
        &self,
        symbols: &[Symbol],
        intervals: &[Interval],
    ) -> Result<Vec<[U256; 2]>> {
        self.collect_assets(current_slugs(symbols, intervals)?)
    }

    pub fn next_market(
        &self,
        symbols: &[Symbol],
        intervals: &[Interval],
    ) -> Result<Vec<[U256; 2]>> {
        self.collect_assets(next_slugs(symbols, intervals)?)
    }

    pub fn markets(&self, symbols: &[Symbol], intervals: &[Interval]) -> Result<Vec<[U256; 2]>> {
        self.collect_assets(expected_slugs(symbols, intervals)?)
    }

    pub fn current_market_ids(
        &self,
        symbols: &[Symbol],
        intervals: &[Interval],
    ) -> Result<Vec<B256>> {
        self.collect_condition_ids(current_slugs(symbols, intervals)?)
    }

    pub async fn refresh(
        &mut self,
        client: &GammaClient,
        symbols: &[Symbol],
        intervals: &[Interval],
    ) -> Result<usize> {
        let slugs = expected_slugs(symbols, intervals)?;
        let discovered = discover_by_slugs(client, slugs.clone()).await?;
        let count = discovered.len();

        self.replace_window(slugs, discovered);

        Ok(count)
    }

    fn replace_window(
        &mut self,
        expected_slugs: Vec<String>,
        discovered: HashMap<String, MarketEntry>,
    ) {
        for slug in &expected_slugs {
            if !discovered.contains_key(slug) {
                self.markets_by_slug.remove(slug);
            }
        }

        for (slug, market) in discovered {
            self.insert_entry(slug, market.condition_id, market.assets);
        }
    }

    fn collect_assets(&self, slugs: Vec<String>) -> Result<Vec<[U256; 2]>> {
        Ok(self
            .collect_entries(slugs)
            .into_iter()
            .map(|entry| entry.assets)
            .collect())
    }

    fn collect_condition_ids(&self, slugs: Vec<String>) -> Result<Vec<B256>> {
        Ok(self
            .collect_entries(slugs)
            .into_iter()
            .map(|entry| entry.condition_id)
            .collect())
    }

    fn collect_entries(&self, slugs: Vec<String>) -> Vec<MarketEntry> {
        slugs
            .into_iter()
            .filter_map(|slug| self.markets_by_slug.get(&slug).copied())
            .collect()
    }
}

pub fn spawn_auto_refresh(
    registry: Arc<RwLock<MarketRegistry>>,
    symbols: &[Symbol],
    intervals: &[Interval],
) -> AbortHandle {
    let symbols = symbols.to_vec();
    let intervals = intervals.to_vec();

    tokio::spawn(async move {
        let client = GammaClient::default();

        loop {
            let mut final_error = None;

            for _attempt in 0..AUTO_REFRESH_MAX_RETRIES {
                match refresh_registry(&registry, &client, &symbols, &intervals).await {
                    Ok(_) => {
                        final_error = None;
                        break;
                    }
                    Err(error) => {
                        final_error = Some(error);
                        sleep(AUTO_REFRESH_RETRY_INTERVAL).await;
                    }
                }
            }

            if let Some(error) = final_error {
                    warn!(
                        "Polymarket market registry 自动刷新失败: {}, retry_interval_secs={}, max_retries={}, horizon_hours={}",
                        error,
                        AUTO_REFRESH_RETRY_INTERVAL.as_secs()
                        ,
                        AUTO_REFRESH_MAX_RETRIES,
                        REGISTRY_HORIZON_HOURS
                    );
            }

            sleep(AUTO_REFRESH_INTERVAL).await;
        }
    })
    .abort_handle()
}

pub fn spawn_subscription_scheduler(
    registry: Arc<RwLock<MarketRegistry>>,
    client: Arc<orderbook_stream::Client>,
    symbols: &[Symbol],
    intervals: &[Interval],
) -> AbortHandle {
    let symbols = symbols.to_vec();
    let intervals = intervals.to_vec();

    tokio::spawn(async move {
        let mut current_markets = HashMap::new();

        loop {
            let next_markets = {
                let markets = match registry.read() {
                    Ok(guard) => guard.current_market(&symbols, &intervals),
                    Err(_) => Err(PolyfillError::internal_simple(
                        "Polymarket market registry 读锁已被污染",
                    )),
                };

                match markets {
                    Ok(markets) => to_market_map(markets),
                    Err(error) => {
                        warn!("Polymarket 订阅调度读取注册表失败: {}", error);
                        sleep(AUTO_SWITCH_GRACE).await;
                        continue;
                    }
                }
            };

            let (to_subscribe, to_unsubscribe) = diff_markets(&current_markets, &next_markets);

            if !to_unsubscribe.is_empty() {
                if let Err(error) = client.unsubscribe(to_unsubscribe).await {
                    warn!("Polymarket 自动退订失败: {}", error);
                }
            }

            if !to_subscribe.is_empty() {
                if let Err(error) = client.subscribe(to_subscribe).await {
                    warn!("Polymarket 自动订阅失败: {}", error);
                }
            }

            current_markets = next_markets;
            match next_switch_instant(&intervals) {
                Ok(instant) => sleep_until(instant).await,
                Err(error) => {
                    warn!("Polymarket 订阅调度计算下次切换时间失败: {}", error);
                    sleep(AUTO_SWITCH_GRACE).await;
                }
            }
        }
    })
    .abort_handle()
}

pub async fn refresh_registry(
    registry: &Arc<RwLock<MarketRegistry>>,
    client: &GammaClient,
    symbols: &[Symbol],
    intervals: &[Interval],
) -> Result<usize> {
    let slugs = expected_slugs(symbols, intervals)?;
    let discovered = discover_by_slugs(client, slugs.clone()).await?;
    let count = discovered.len();

    let mut guard = registry
        .write()
        .map_err(|_| PolyfillError::internal_simple("Polymarket market registry 写锁已被污染"))?;

    guard.replace_window(slugs, discovered);

    Ok(count)
}

pub async fn current_active_market(
    client: &GammaClient,
    symbol: Symbol,
    interval: Interval,
) -> Result<Option<[U256; 2]>> {
    discover_single_market(client, current_slug(symbol, interval)?).await
}

pub async fn next_active_market(
    client: &GammaClient,
    symbol: Symbol,
    interval: Interval,
) -> Result<Option<[U256; 2]>> {
    discover_single_market(client, next_slug(symbol, interval)?).await
}

pub async fn active_markets(
    client: &GammaClient,
    symbols: &[Symbol],
    intervals: &[Interval],
) -> Result<Vec<[U256; 2]>> {
    discover_market_assets(client, expected_slugs(symbols, intervals)?).await
}

async fn discover_single_market(client: &GammaClient, slug: String) -> Result<Option<[U256; 2]>> {
    Ok(discover_market_assets(client, vec![slug])
        .await?
        .into_iter()
        .next())
}

async fn discover_market_assets(
    client: &GammaClient,
    slugs: Vec<String>,
) -> Result<Vec<[U256; 2]>> {
    Ok(discover_by_slugs(client, slugs)
        .await?
        .into_values()
        .map(|entry| entry.assets)
        .collect())
}

async fn discover_by_slugs(
    client: &GammaClient,
    slugs: Vec<String>,
) -> Result<HashMap<String, MarketEntry>> {
    let markets = client
        .markets(&MarketsRequest::builder().slug(slugs).limit(150).build())
        .await
        .map_err(|e| PolyfillError::internal_simple(format!("查询 Gamma 市场失败: {e}")))?;

    Ok(markets
        .into_iter()
        .filter_map(active_market_entry)
        .collect())
}

fn active_market_entry(market: Market) -> Option<(String, MarketEntry)> {
    if market.active != Some(true) || market.closed == Some(true) {
        return None;
    }

    let slug = market.slug?;
    let condition_id = market.condition_id?;
    let token_ids = market.clob_token_ids?;
    let [up_asset_id, down_asset_id] = token_ids.as_slice() else {
        return None;
    };

    Some((
        slug,
        MarketEntry {
            condition_id,
            assets: [*up_asset_id, *down_asset_id],
        },
    ))
}

fn expected_slugs(symbols: &[Symbol], intervals: &[Interval]) -> Result<Vec<String>> {
    collect_slugs(symbols, intervals, |symbol, interval| {
        slugs_for_hours(symbol, interval, REGISTRY_HORIZON_HOURS)
    })
}

fn current_slugs(symbols: &[Symbol], intervals: &[Interval]) -> Result<Vec<String>> {
    collect_slugs(symbols, intervals, |symbol, interval| {
        Ok(vec![current_slug(symbol, interval)?])
    })
}

fn next_slugs(symbols: &[Symbol], intervals: &[Interval]) -> Result<Vec<String>> {
    collect_slugs(symbols, intervals, |symbol, interval| {
        Ok(vec![next_slug(symbol, interval)?])
    })
}

fn collect_slugs<F>(
    symbols: &[Symbol],
    intervals: &[Interval],
    mut expand: F,
) -> Result<Vec<String>>
where
    F: FnMut(Symbol, Interval) -> Result<Vec<String>>,
{
    let mut slugs = Vec::new();

    for symbol in symbols {
        for interval in intervals {
            slugs.extend(expand(*symbol, *interval)?);
        }
    }

    Ok(slugs)
}

fn to_market_map(markets: Vec<[U256; 2]>) -> HashMap<U256, [U256; 2]> {
    markets
        .into_iter()
        .map(|market @ [up_asset_id, _]| (up_asset_id, market))
        .collect()
}

fn diff_markets(
    current: &HashMap<U256, [U256; 2]>,
    next: &HashMap<U256, [U256; 2]>,
) -> (Vec<[U256; 2]>, Vec<U256>) {
    let mut to_subscribe = Vec::new();
    let mut to_unsubscribe = Vec::new();

    for (up_asset_id, next_market) in next {
        match current.get(up_asset_id) {
            Some(current_market) if current_market == next_market => {}
            Some(_) => {
                to_unsubscribe.push(*up_asset_id);
                to_subscribe.push(*next_market);
            }
            None => to_subscribe.push(*next_market),
        }
    }

    for up_asset_id in current.keys() {
        if !next.contains_key(up_asset_id) {
            to_unsubscribe.push(*up_asset_id);
        }
    }

    (to_subscribe, to_unsubscribe)
}

fn next_switch_instant(intervals: &[Interval]) -> Result<Instant> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| PolyfillError::internal_simple(format!("系统时间错误: {e}")))?;
    let now_secs = i64::try_from(now.as_secs())
        .map_err(|_| PolyfillError::internal_simple("当前时间超出 i64 范围"))?;

    let next_close_ts = intervals
        .iter()
        .map(|interval| next_close_ts(now_secs, *interval))
        .min()
        .ok_or_else(|| PolyfillError::validation("intervals 不能为空"))?;

    let wait_secs = (next_close_ts - now_secs).max(0) as u64;
    Ok(Instant::now() + Duration::from_secs(wait_secs) + AUTO_SWITCH_GRACE)
}

fn next_close_ts(now_secs: i64, interval: Interval) -> i64 {
    let step = match interval {
        Interval::M5 => 5 * 60,
        Interval::M15 => 15 * 60,
    };

    ((now_secs / step) + 1) * step
}
