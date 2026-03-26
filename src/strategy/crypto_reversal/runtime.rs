use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use alloy_signer_local::PrivateKeySigner;
use anyhow::{Context, Result};
use chrono::Utc;
use polymarket_client_sdk::auth::{Normal, state::Authenticated};
use polymarket_client_sdk::clob::Client as ClobClient;
use polymarket_client_sdk::types::B256;
use tokio::task::JoinSet;
use tracing::info;

use crate::binance;
use crate::config::AppConfig;
use crate::dashboard::Handle as DashboardHandle;
use crate::polymarket::market_registry::{MarketRegistry, refresh_registry, spawn_auto_refresh};
use crate::polymarket::user_stream::{Client as UserClient, EventSink};
use crate::storage::sqlite::Store;
use crate::strategy::StrategyContext;
use crate::strategy::crypto_reversal::{constants, execute, service};
use crate::types::crypto::{Interval, Symbol};

const DASHBOARD_HISTORY_LIMIT: usize = 64;
const MARKET_PRUNE_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
struct WorkerScanState {
    current_scan_window_key: Option<String>,
    reported_candidate_keys: HashSet<String>,
    submitted_candidate_keys: HashSet<String>,
}

impl WorkerScanState {
    fn enter_window(&mut self, key: String) {
        if self.current_scan_window_key.as_ref() != Some(&key) {
            self.current_scan_window_key = Some(key);
            self.reported_candidate_keys.clear();
            self.submitted_candidate_keys.clear();
        }
    }

    fn leave_window(&mut self) {
        self.current_scan_window_key = None;
        self.reported_candidate_keys.clear();
        self.submitted_candidate_keys.clear();
    }
}

#[derive(Clone)]
struct DashboardReporter {
    handle: DashboardHandle,
}

impl DashboardReporter {
    fn new(handle: DashboardHandle) -> Self {
        Self { handle }
    }

    fn status(&self, status: &str) {
        self.handle
            .strategy_status(constants::STRATEGY_NAME, status);
    }

    async fn load(&self, store: &Store) -> Result<()> {
        self.handle.load_history(
            &store
                .load_dashboard_history(DASHBOARD_HISTORY_LIMIT)
                .await
                .map_err(anyhow::Error::from)?,
        );
        self.handle.load_strategy_attribution(
            &store
                .load_strategy_attribution()
                .await
                .map_err(anyhow::Error::from)?,
        );
        Ok(())
    }

    fn connect_user(
        &self,
        open_orders: &[crate::polymarket::types::open_orders::Order],
        positions: &[crate::polymarket::types::positions::Position],
    ) {
        self.handle.polymarket_status("connected");
        self.push_user_state(open_orders, positions);
    }

    fn connect_binance(&self) {
        self.handle.binance_status("connected");
    }

    fn user_state(
        &self,
        open_orders: &[crate::polymarket::types::open_orders::Order],
        positions: &[crate::polymarket::types::positions::Position],
    ) {
        self.push_user_state(open_orders, positions);
    }

    fn signal(&self, candidate: &service::Candidate) {
        self.handle.signal(constants::STRATEGY_NAME, candidate);
    }

    fn order_submission(&self, candidate: &service::Candidate, submission: &execute::Submission) {
        self.handle.order_submission(
            constants::STRATEGY_NAME,
            candidate,
            submission.asset_id,
            &submission.order_id,
            submission.price,
            submission.size,
        );
    }

    fn scan(&self) {
        self.handle.scan(constants::STRATEGY_NAME);
    }

    fn push_user_state(
        &self,
        open_orders: &[crate::polymarket::types::open_orders::Order],
        positions: &[crate::polymarket::types::positions::Position],
    ) {
        self.handle.user_state(open_orders, positions);
    }
}

impl EventSink for DashboardReporter {
    fn ws_status(&self, status: &str) {
        self.handle.polymarket_status(status);
    }

    fn user_state(
        &self,
        open_orders: &[crate::polymarket::types::open_orders::Order],
        positions: &[crate::polymarket::types::positions::Position],
    ) {
        self.push_user_state(open_orders, positions);
    }

    fn error(&self, message: String) {
        self.handle.error(None, message);
    }

    fn order(&self, order: &crate::events::Order) {
        self.handle.order(order);
    }

    fn trade(&self, trade: &crate::events::Trade) {
        self.handle.trade(trade);
    }
}

pub async fn run(
    app: &AppConfig,
    dashboard: DashboardHandle,
    context: StrategyContext,
) -> Result<()> {
    ensure_parent_dir(&app.runtime.sqlite_path)?;
    let reporter = Arc::new(DashboardReporter::new(dashboard));
    reporter.status("starting");

    let StrategyContext {
        signer,
        credentials: _,
        clob_client: client,
        gamma_client,
        data_client: _,
        safe_address: _,
    } = context;

    let store = Store::open(&app.runtime.sqlite_path)
        .await
        .map_err(anyhow::Error::from)?;
    reporter.load(&store).await?;
    let user = Arc::new(
        UserClient::start_with_store(&client, Some(store.clone()), Some(reporter.clone()))
            .await
            .map_err(anyhow::Error::from)?,
    );
    reporter.connect_user(
        &user.open_orders().map_err(anyhow::Error::from)?,
        &user.positions().map_err(anyhow::Error::from)?,
    );
    let binance = Arc::new(
        binance::Client::connect()
            .await
            .map_err(anyhow::Error::from)?,
    );
    reporter.connect_binance();

    let registry = Arc::new(RwLock::new(MarketRegistry::new()));
    let initial = refresh_registry(
        &registry,
        &gamma_client,
        &app.trading.symbols,
        &app.runtime.intervals,
    )
    .await
    .map_err(anyhow::Error::from)?;
    info!(
        symbols = ?app.trading.symbols,
        intervals = ?app.runtime.intervals,
        initial_markets = initial,
        sqlite_path = %app.runtime.sqlite_path.display(),
        "crypto_reversal runtime started"
    );

    let _registry_task = spawn_auto_refresh(
        Arc::clone(&registry),
        &app.trading.symbols,
        &app.runtime.intervals,
    );
    let strategy_configs = build_strategy_configs(app);
    service::subscribe_inputs(binance.as_ref(), strategy_configs.as_slice())
        .await
        .map_err(anyhow::Error::from)?;
    reporter.status("running");

    let runtime = Arc::new(app.runtime.clone());
    let state = Arc::new(execute::State::new());
    let client = Arc::new(client);
    let signer = Arc::new(signer);
    let store = Arc::new(store);
    let mut tasks = JoinSet::new();

    tasks.spawn(run_market_prune_worker(
        Arc::clone(&registry),
        Arc::clone(&user),
        Arc::clone(&reporter),
        app.trading.symbols.clone(),
        app.runtime.intervals.clone(),
    ));

    for config in strategy_configs {
        tasks.spawn(run_config_worker(
            config,
            Arc::clone(&runtime),
            Arc::clone(&reporter),
            Arc::clone(&binance),
            Arc::clone(&registry),
            Arc::clone(&state),
            Arc::clone(&client),
            Arc::clone(&signer),
            Arc::clone(&store),
            Arc::clone(&user),
        ));
    }

    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "crypto_reversal worker task failed: {error}"
                ));
            }
        }
    }

    Ok(())
}

async fn run_config_worker(
    config: service::Config,
    runtime: Arc<crate::config::RuntimeConfig>,
    reporter: Arc<DashboardReporter>,
    binance: Arc<binance::Client>,
    registry: Arc<RwLock<MarketRegistry>>,
    state: Arc<execute::State>,
    client: Arc<ClobClient<Authenticated<Normal>>>,
    signer: Arc<PrivateKeySigner>,
    store: Arc<Store>,
    user: Arc<UserClient>,
) -> Result<()> {
    let mut worker_state = WorkerScanState::default();

    loop {
        let now_ms = Utc::now().timestamp_millis();
        if !interval_is_active(now_ms, config.interval) {
            worker_state.leave_window();
            tokio::time::sleep(next_scan_delay(
                now_ms,
                &[config.interval],
                runtime.scan_interval,
            ))
            .await;
            continue;
        }

        reporter.scan();
        worker_state.enter_window(format_scan_window_key(now_ms, &[config.interval]));

        let evaluation = service::evaluate_from_input(&config, binance.as_ref(), &registry)?;
        let service::EvaluationOutcome::Candidate(candidate) = evaluation.outcome else {
            tokio::time::sleep(runtime.scan_interval).await;
            continue;
        };

        let candidate_key = format_candidate_key(&candidate);
        if worker_state
            .reported_candidate_keys
            .insert(candidate_key.clone())
        {
            info!(
                symbol = candidate.symbol.as_slug(),
                interval = candidate.interval.as_slug(),
                market_slug = %candidate.market_slug,
                side = ?candidate.side,
                signal_time_ms = candidate.signal_time_ms,
                score = candidate.score,
                size_factor = candidate.size_factor,
                "crypto_reversal candidate ready"
            );
            reporter.signal(&candidate);
        }

        if !worker_state
            .submitted_candidate_keys
            .contains(&candidate_key)
        {
            if let execute::Attempt::Submitted(submission) = execute::submit(
                &candidate,
                runtime.as_ref(),
                state.as_ref(),
                &registry,
                client.as_ref(),
                signer.as_ref(),
                store.as_ref(),
                user.as_ref(),
            )
            .await?
            {
                worker_state.submitted_candidate_keys.insert(candidate_key);
                reporter.order_submission(&candidate, &submission);
                info!(
                    symbol = candidate.symbol.as_slug(),
                    interval = candidate.interval.as_slug(),
                    market_slug = %candidate.market_slug,
                    side = ?candidate.side,
                    order_id = %submission.order_id,
                    status = %submission.status,
                    success = submission.success,
                    size = %submission.size,
                    trade_ids = ?submission.trade_ids,
                    "crypto_reversal order submitted"
                );
            }
        }

        tokio::time::sleep(runtime.scan_interval).await;
    }
}

async fn run_market_prune_worker(
    registry: Arc<RwLock<MarketRegistry>>,
    user: Arc<UserClient>,
    reporter: Arc<DashboardReporter>,
    symbols: Vec<Symbol>,
    intervals: Vec<Interval>,
) -> Result<()> {
    let mut current_market_ids = current_market_id_set(&registry, &symbols, &intervals)?;

    loop {
        tokio::time::sleep(MARKET_PRUNE_POLL_INTERVAL).await;

        let next_market_ids = current_market_id_set(&registry, &symbols, &intervals)?;
        let ended_market_ids = current_market_ids
            .difference(&next_market_ids)
            .copied()
            .collect::<HashSet<_>>();

        if !ended_market_ids.is_empty() {
            if let Some((open_orders, positions)) = user.prune_markets(&ended_market_ids)? {
                reporter.user_state(&open_orders, &positions);
            }
        }

        current_market_ids = next_market_ids;
    }
}

fn build_strategy_configs(app: &AppConfig) -> Vec<service::Config> {
    let model = constants::default_model_config();
    let mut configs = Vec::with_capacity(app.trading.symbols.len() * app.runtime.intervals.len());

    for &symbol in &app.trading.symbols {
        for &interval in &app.runtime.intervals {
            configs.push(service::Config {
                symbol,
                interval,
                model: model.clone(),
            });
        }
    }

    configs
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };

    std::fs::create_dir_all(parent)
        .with_context(|| format!("创建 SQLite 目录失败: {}", parent.display()))
}

fn format_candidate_key(candidate: &service::Candidate) -> String {
    format!(
        "{}:{}:{}:{:?}:{}",
        candidate.symbol.as_slug(),
        candidate.interval.as_slug(),
        candidate.market_slug,
        candidate.side,
        candidate.signal_time_ms
    )
}

fn current_market_id_set(
    registry: &Arc<RwLock<MarketRegistry>>,
    symbols: &[Symbol],
    intervals: &[Interval],
) -> Result<HashSet<B256>> {
    let guard = registry
        .read()
        .map_err(|_| anyhow::anyhow!("Polymarket market registry 读锁已被污染"))?;
    Ok(guard
        .current_market_ids(symbols, intervals)
        .map_err(anyhow::Error::from)?
        .into_iter()
        .collect())
}

fn format_scan_window_key(
    now_ms: i64,
    active_intervals: &[crate::types::crypto::Interval],
) -> String {
    let mut key = String::new();

    for (index, interval) in active_intervals.iter().copied().enumerate() {
        if index > 0 {
            key.push('|');
        }

        let step_ms = interval.step_secs() * 1_000;
        let window_index = now_ms.div_euclid(step_ms);
        key.push_str(interval.as_slug());
        key.push(':');
        key.push_str(&window_index.to_string());
    }

    key
}

fn interval_is_active(now_ms: i64, interval: crate::types::crypto::Interval) -> bool {
    let step_ms = interval.step_secs() * 1_000;
    // 这里按“当前时刻在本周期中的相位”判断是否已经进入扫描窗口，
    // 不依赖额外状态机，重启后也能立刻恢复正确调度。
    now_ms.rem_euclid(step_ms) >= scan_start_ms(interval)
}

fn next_scan_delay(
    now_ms: i64,
    intervals: &[crate::types::crypto::Interval],
    poll_interval: Duration,
) -> Duration {
    if intervals.is_empty()
        || intervals
            .iter()
            .copied()
            .any(|interval| interval_is_active(now_ms, interval))
    {
        // 已经在扫描窗口内时，退回到细粒度轮询频率。
        return poll_interval;
    }

    let wait_ms = intervals
        .iter()
        .copied()
        .map(|interval| {
            let step_ms = interval.step_secs() * 1_000;
            let phase_ms = now_ms.rem_euclid(step_ms);
            let start_ms = scan_start_ms(interval);

            if phase_ms < start_ms {
                start_ms - phase_ms
            } else {
                // 当前周期已经错过窗口起点时，等待到下一个周期的窗口开始。
                step_ms - phase_ms + start_ms
            }
        })
        .min()
        .unwrap_or(poll_interval.as_millis() as i64);

    Duration::from_millis(wait_ms.max(1) as u64)
}

fn scan_start_ms(interval: crate::types::crypto::Interval) -> i64 {
    match interval {
        crate::types::crypto::Interval::M5 => constants::M5_SCAN_START_MS,
        crate::types::crypto::Interval::M15 => constants::M15_SCAN_START_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_is_active_only_after_window_start() {
        assert!(!interval_is_active(289_999, Interval::M5));
        assert!(interval_is_active(290_000, Interval::M5));
        assert!(!interval_is_active(889_999, Interval::M15));
        assert!(interval_is_active(890_000, Interval::M15));
    }

    #[test]
    fn next_scan_delay_uses_nearest_interval_window() {
        let delay = next_scan_delay(
            100_000,
            &[Interval::M5, Interval::M15],
            Duration::from_secs(1),
        );
        assert_eq!(delay, Duration::from_secs(190));
    }

    #[test]
    fn format_scan_window_key_is_stable_within_same_window() {
        let first = format_scan_window_key(291_000, &[Interval::M5]);
        let second = format_scan_window_key(299_000, &[Interval::M5]);

        assert_eq!(first, second);
    }

    #[test]
    fn format_scan_window_key_changes_across_windows() {
        let first = format_scan_window_key(299_000, &[Interval::M5]);
        let second = format_scan_window_key(599_000, &[Interval::M5]);

        assert_ne!(first, second);
    }

    #[test]
    fn worker_scan_state_resets_when_window_changes() {
        let mut state = WorkerScanState::default();
        state.enter_window("m5:1".to_string());
        state
            .reported_candidate_keys
            .insert("candidate-a".to_string());
        state
            .submitted_candidate_keys
            .insert("candidate-a".to_string());

        state.enter_window("m5:2".to_string());

        assert!(state.reported_candidate_keys.is_empty());
        assert!(state.submitted_candidate_keys.is_empty());
        assert_eq!(state.current_scan_window_key.as_deref(), Some("m5:2"));
    }
}
