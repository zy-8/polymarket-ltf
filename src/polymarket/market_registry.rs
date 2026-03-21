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
use polymarket_client_sdk::gamma::Client as GammaClient;
use polymarket_client_sdk::gamma::types::request::MarketsRequest;
use polymarket_client_sdk::gamma::types::response::Market;
use polymarket_client_sdk::types::U256;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::task::AbortHandle;
use tokio::time::{Duration, Instant, sleep, sleep_until};
use tracing::warn;

const AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60);
const AUTO_REFRESH_RETRY_INTERVAL: Duration = Duration::from_secs(2);
const AUTO_REFRESH_MAX_RETRIES: usize = 5;
const REGISTRY_HORIZON_HOURS: u32 = 2;
const AUTO_SWITCH_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, Default, Clone)]
pub struct MarketRegistry {
    markets_by_slug: HashMap<String, [U256; 2]>,
}

impl MarketRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, slug: impl Into<String>, market: [U256; 2]) {
        self.markets_by_slug.insert(slug.into(), market);
    }

    pub fn get(&self, slug: &str) -> Option<[U256; 2]> {
        self.markets_by_slug.get(slug).copied()
    }

    pub fn current_market(
        &self,
        symbols: &[Symbol],
        intervals: &[Interval],
    ) -> Result<Vec<[U256; 2]>> {
        let mut markets = Vec::new();

        for slug in current_slugs(symbols, intervals)? {
            if let Some(market) = self.markets_by_slug.get(&slug).copied() {
                markets.push(market);
            }
        }

        Ok(markets)
    }

    pub fn next_market(
        &self,
        symbols: &[Symbol],
        intervals: &[Interval],
    ) -> Result<Vec<[U256; 2]>> {
        let mut markets = Vec::new();

        for slug in next_slugs(symbols, intervals)? {
            if let Some(market) = self.markets_by_slug.get(&slug).copied() {
                markets.push(market);
            }
        }

        Ok(markets)
    }

    pub fn markets(&self, symbols: &[Symbol], intervals: &[Interval]) -> Result<Vec<[U256; 2]>> {
        let mut markets = Vec::new();

        for slug in expected_slugs(symbols, intervals)? {
            if let Some(market) = self.markets_by_slug.get(&slug).copied() {
                markets.push(market);
            }
        }

        Ok(markets)
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
        discovered: HashMap<String, [U256; 2]>,
    ) {
        for slug in &expected_slugs {
            if !discovered.contains_key(slug) {
                self.markets_by_slug.remove(slug);
            }
        }

        for (slug, market) in discovered {
            self.insert(slug, market);
        }
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
    let slug = current_slug(symbol, interval)?;
    let markets = discover_by_slugs(client, vec![slug]).await?;

    Ok(markets.into_values().next())
}

pub async fn next_active_market(
    client: &GammaClient,
    symbol: Symbol,
    interval: Interval,
) -> Result<Option<[U256; 2]>> {
    let slug = next_slug(symbol, interval)?;
    let markets = discover_by_slugs(client, vec![slug]).await?;

    Ok(markets.into_values().next())
}

pub async fn active_markets(
    client: &GammaClient,
    symbols: &[Symbol],
    intervals: &[Interval],
) -> Result<Vec<[U256; 2]>> {
    let slugs = expected_slugs(symbols, intervals)?;
    let markets = discover_by_slugs(client, slugs).await?;

    Ok(markets.into_values().collect())
}

async fn discover_by_slugs(
    client: &GammaClient,
    slugs: Vec<String>,
) -> Result<HashMap<String, [U256; 2]>> {
    let markets = client
        .markets(&MarketsRequest::builder().slug(slugs).closed(false).build())
        .await
        .map_err(|e| PolyfillError::internal_simple(format!("查询 Gamma 市场失败: {e}")))?;

    Ok(markets
        .into_iter()
        .filter_map(active_market_entry)
        .collect())
}

fn active_market_entry(market: Market) -> Option<(String, [U256; 2])> {
    if market.active != Some(true) || market.closed == Some(true) {
        return None;
    }

    if market.enable_order_book == Some(false) || market.accepting_orders == Some(false) {
        return None;
    }

    let slug = market.slug?;
    let token_ids = market.clob_token_ids?;
    let [up_asset_id, down_asset_id] = token_ids.as_slice() else {
        return None;
    };

    Some((slug, [*up_asset_id, *down_asset_id]))
}

fn expected_slugs(symbols: &[Symbol], intervals: &[Interval]) -> Result<Vec<String>> {
    let mut slugs = Vec::new();

    for symbol in symbols {
        for interval in intervals {
            slugs.extend(slugs_for_hours(*symbol, *interval, REGISTRY_HORIZON_HOURS)?);
        }
    }

    Ok(slugs)
}

fn current_slugs(symbols: &[Symbol], intervals: &[Interval]) -> Result<Vec<String>> {
    let mut slugs = Vec::new();

    for symbol in symbols {
        for interval in intervals {
            slugs.push(current_slug(*symbol, *interval)?);
        }
    }

    Ok(slugs)
}

fn next_slugs(symbols: &[Symbol], intervals: &[Interval]) -> Result<Vec<String>> {
    let mut slugs = Vec::new();

    for symbol in symbols {
        for interval in intervals {
            slugs.push(next_slug(*symbol, *interval)?);
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
