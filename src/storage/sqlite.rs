//! SQLite 策略事件存储。
//!
//! 这个模块只解决一个问题：
//! - 把订单事件、成交事件和策略归因稳定落盘；
//! - 为 dashboard 提供最小历史读取入口。
//!
//! 当前不在这里实现：
//! - 执行逻辑；
//! - 复杂查询层；
//! - ORM；
//! - 迁移框架。

use std::fs;
use std::path::Path;
use std::str::FromStr;

use polymarket_client_sdk::data::types::response::Position;
use rust_decimal::Decimal;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{ConnectOptions, Row, SqlitePool};

use crate::errors::{PolyfillError, Result};
use crate::events;

const INIT_SQL: &str = include_str!("../../schema/init.sql");
const LATEST_TRACKED_ASSETS_CTE_SQL: &str = "
    WITH latest_tracked AS (
        SELECT asset_id, strategy
        FROM (
            SELECT
                asset_id,
                strategy,
                ROW_NUMBER() OVER (
                    PARTITION BY asset_id
                    ORDER BY created_at DESC, order_id DESC
                ) AS rn
            FROM strategy
        )
        WHERE rn = 1
    )
";

#[derive(Debug, Clone, Default)]
pub struct InfoStats {
    pub trigger_count: usize,
    pub closed_count: usize,
    pub today_closed_count: usize,
    pub today_closed_pnl_usdc: String,
    pub closed_win_count: usize,
    pub closed_loss_count: usize,
    pub missed_count: usize,
    pub missed_win_count: usize,
    pub missed_loss_count: usize,
    pub strategies: Vec<StrategyInfoStats>,
}

#[derive(Debug, Clone, Default)]
pub struct StrategyInfoStats {
    pub strategy: String,
    pub trigger_count: usize,
    pub closed_count: usize,
    pub today_closed_count: usize,
    pub today_closed_pnl_usdc: String,
    pub closed_win_count: usize,
    pub closed_loss_count: usize,
    pub missed_count: usize,
    pub missed_win_count: usize,
    pub missed_loss_count: usize,
    pub settled_pnl_usdc: String,
}

#[derive(Debug, Clone)]
pub struct PositionRecord {
    pub strategy: String,
    pub proxy_wallet: String,
    pub asset: String,
    pub condition_id: String,
    pub market_slug: String,
    pub outcome: String,
    pub avg_price: String,
    pub size: Option<String>,
    pub total_bought: Option<String>,
    pub current_value: Option<String>,
    pub cash_pnl: Option<String>,
    pub realized_pnl: String,
    pub cur_price: String,
    pub end_date: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Default)]
pub struct PositionsPage {
    pub strategy: String,
    pub range: String,
    pub page: usize,
    pub page_size: usize,
    pub total: usize,
    pub total_pages: usize,
    pub rows: Vec<PositionRecord>,
}

#[derive(Debug, Clone)]
pub struct DashboardHistory {
    pub strategies: Vec<events::Strategy>,
    pub orders: Vec<events::Order>,
    pub trades: Vec<events::Trade>,
}

#[derive(Debug, Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|error| {
                PolyfillError::internal_simple(format!(
                    "创建 SQLite 目录失败 {}: {error}",
                    parent.display()
                ))
            })?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .disable_statement_logging();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|error| {
                PolyfillError::internal_simple(format!("打开 SQLite 失败: {error}"))
            })?;
        let store = Self { pool };
        store.init().await?;
        Ok(store)
    }

    pub async fn open_memory() -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(":memory:")
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .disable_statement_logging();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|error| {
                PolyfillError::internal_simple(format!("打开内存 SQLite 失败: {error}"))
            })?;
        let store = Self { pool };
        store.init().await?;
        Ok(store)
    }

    async fn init(&self) -> Result<()> {
        sqlx::query(INIT_SQL)
            .execute(&self.pool)
            .await
            .map_err(|error| {
                PolyfillError::internal_simple(format!("初始化 SQLite schema 失败: {error}"))
            })?;
        Ok(())
    }

    pub async fn insert_order(&self, event: &events::Order) -> Result<()> {
        self.execute_write(
            sqlx::query(
                "INSERT INTO orders(
                order_id, side, price, size, status, created_at
            ) VALUES(?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(order_id) DO UPDATE SET
                side = excluded.side,
                price = excluded.price,
                size = excluded.size,
                status = excluded.status,
                created_at = excluded.created_at",
            )
            .bind(&event.order_id)
            .bind(&event.side)
            .bind(event.price.to_string())
            .bind(event.size.to_string())
            .bind(&event.status)
            .bind(event.created_at),
            "写入 orders 失败",
        )
        .await
    }

    pub async fn insert_trade(&self, event: &events::Trade) -> Result<()> {
        self.execute_write(
            sqlx::query(
                "INSERT OR IGNORE INTO trades(
                id, order_id, trade_id, side, price, size, fee_bps, event_time, created_at
            ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .bind(&event.id)
            .bind(&event.order_id)
            .bind(&event.trade_id)
            .bind(&event.side)
            .bind(event.price.to_string())
            .bind(event.size.to_string())
            .bind(event.fee_bps.as_ref().map(rust_decimal::Decimal::to_string))
            .bind(event.event_time)
            .bind(event.created_at),
            "写入 trades 失败",
        )
        .await
    }

    pub async fn insert_strategy(&self, event: &events::Strategy) -> Result<()> {
        self.execute_write(
            sqlx::query(
                "INSERT OR REPLACE INTO strategy(
                order_id, asset_id, strategy, symbol, interval, market_slug, side, outcome, created_at, event
            ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )
            .bind(&event.order_id)
            .bind(&event.asset_id)
            .bind(&event.strategy)
            .bind(&event.symbol)
            .bind(&event.interval)
            .bind(&event.market_slug)
            .bind(&event.side)
            .bind(&event.outcome)
            .bind(event.created_at)
            .bind(&event.event),
            "写入 strategy 失败",
        )
        .await
    }

    pub async fn select_pending_strategy_outcomes(&self) -> Result<Vec<String>> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT market_slug
             FROM strategy
             WHERE TRIM(outcome) = ''
               AND (
                   created_at
                   + CASE interval
                       WHEN '5m' THEN 300000
                       WHEN '15m' THEN 900000
                       ELSE 0
                     END
               ) <= ?1
             ORDER BY created_at DESC",
        )
        .bind(now_ms)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            PolyfillError::internal_simple(format!("读取待补 outcome 的 strategy 失败: {error}"))
        })
    }

    pub async fn update_strategy_outcomes(
        &self,
        outcomes: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        for (market_slug, outcome) in outcomes {
            self.execute_write(
                sqlx::query(
                    "UPDATE strategy
                     SET outcome = ?1
                     WHERE market_slug = ?2
                       AND TRIM(outcome) = ''",
                )
                .bind(outcome)
                .bind(market_slug),
                "更新 strategy outcome 失败",
            )
            .await?;
        }

        Ok(())
    }

    pub async fn insert_positions(&self, positions: &[Position]) -> Result<()> {
        self.insert_positions_at(positions, chrono::Utc::now().timestamp_millis())
            .await
    }

    pub async fn insert_positions_at(
        &self,
        positions: &[Position],
        snapshot_ts: i64,
    ) -> Result<()> {
        for position in positions {
            self.execute_write(
                sqlx::query(
                    "INSERT OR REPLACE INTO positions(
                        asset, proxy_wallet, condition_id, market_slug, outcome, avg_price, size,
                        total_bought, current_value, cash_pnl, realized_pnl, cur_price,
                        end_date, timestamp
                    )
                    SELECT
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
                    WHERE EXISTS (
                        SELECT 1
                        FROM strategy
                        WHERE asset_id = ?1
                    )",
                )
                .bind(position.asset.to_string())
                .bind(position.proxy_wallet.to_string())
                .bind(position.condition_id.to_string())
                .bind(&position.slug)
                .bind(&position.outcome)
                .bind(position.avg_price.normalize().to_string())
                .bind(Some(position.size.normalize().to_string()))
                .bind(Some(position.total_bought.normalize().to_string()))
                .bind(Some(position.current_value.normalize().to_string()))
                .bind(Some(position.cash_pnl.normalize().to_string()))
                .bind(position.realized_pnl.normalize().to_string())
                .bind(position.cur_price.normalize().to_string())
                .bind(
                    position
                        .end_date
                        .map(|date| date.to_string())
                        .unwrap_or_default(),
                )
                .bind(snapshot_ts),
                "写入 positions 失败",
            )
            .await?;
        }

        Ok(())
    }

    pub async fn select_dashboard_history(&self, limit: usize) -> Result<DashboardHistory> {
        Ok(DashboardHistory {
            strategies: self
                .select_strategy_rows(Some(limit), "读取 strategy 历史失败")
                .await?,
            orders: self.select_order_history(limit).await?,
            trades: self.select_trade_history(limit).await?,
        })
    }

    pub async fn select_strategy_attribution(&self) -> Result<Vec<events::Strategy>> {
        self.select_strategy_rows(None, "读取 strategy 归因失败")
            .await
    }

    pub async fn select_info_stats(&self) -> Result<InfoStats> {
        let trigger_rows = sqlx::query(
            "SELECT strategy, COUNT(*) AS trigger_count
             FROM strategy
             GROUP BY strategy",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            PolyfillError::internal_simple(format!("读取 info trigger 统计失败: {error}"))
        })?;

        let filled_rows = sqlx::query(&format!(
            "{LATEST_TRACKED_ASSETS_CTE_SQL}
             SELECT
                 tracked.strategy,
                 COUNT(*) AS closed_count,
                 SUM(
                     CASE
                         WHEN date(
                             CASE
                                 WHEN positions.timestamp >= 1000000000000
                                     THEN positions.timestamp / 1000
                                 ELSE positions.timestamp
                             END,
                             'unixepoch'
                         ) = date('now')
                             THEN 1
                         ELSE 0
                     END
                 ) AS today_closed_count,
                 COALESCE(SUM(
                     CASE
                         WHEN date(
                             CASE
                                 WHEN positions.timestamp >= 1000000000000
                                     THEN positions.timestamp / 1000
                                 ELSE positions.timestamp
                             END,
                             'unixepoch'
                         ) = date('now')
                             THEN CAST(
                                 COALESCE(
                                     NULLIF(positions.cash_pnl, ''),
                                     NULLIF(positions.realized_pnl, ''),
                                     '0'
                                 ) AS REAL
                             )
                         ELSE 0
                     END
                 ), 0) AS today_closed_pnl_usdc
             FROM positions
             JOIN latest_tracked AS tracked
             ON tracked.asset_id = positions.asset
             GROUP BY tracked.strategy"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            PolyfillError::internal_simple(format!("读取 info filled 统计失败: {error}"))
        })?;

        let mut strategies = std::collections::BTreeMap::<String, StrategyInfoStats>::new();

        for row in trigger_rows {
            let strategy = required_text(&row, "strategy", "info.strategy")?;
            let trigger_count: i64 = required_value(&row, "trigger_count", "info.trigger_count")?;
            strategies.insert(
                strategy.clone(),
                StrategyInfoStats {
                    strategy,
                    trigger_count: trigger_count.max(0) as usize,
                    closed_count: 0,
                    today_closed_count: 0,
                    today_closed_pnl_usdc: "0".to_string(),
                    closed_win_count: 0,
                    closed_loss_count: 0,
                    missed_count: 0,
                    missed_win_count: 0,
                    missed_loss_count: 0,
                    settled_pnl_usdc: "0".to_string(),
                },
            );
        }

        for row in filled_rows {
            let strategy = required_text(&row, "strategy", "info.strategy")?;
            let closed_count: i64 = required_value(&row, "closed_count", "info.closed_count")?;
            let today_closed_count: i64 =
                required_value(&row, "today_closed_count", "info.today_closed_count")?;
            let today_closed_pnl_usdc: f64 =
                required_value(&row, "today_closed_pnl_usdc", "info.today_closed_pnl_usdc")?;
            let stats = strategies
                .entry(strategy.clone())
                .or_insert_with(|| StrategyInfoStats {
                    strategy,
                    trigger_count: 0,
                    closed_count: 0,
                    today_closed_count: 0,
                    today_closed_pnl_usdc: "0".to_string(),
                    closed_win_count: 0,
                    closed_loss_count: 0,
                    missed_count: 0,
                    missed_win_count: 0,
                    missed_loss_count: 0,
                    settled_pnl_usdc: "0".to_string(),
                });
            stats.closed_count = closed_count.max(0) as usize;
            stats.today_closed_count = today_closed_count.max(0) as usize;
            stats.today_closed_pnl_usdc = Decimal::from_f64_retain(today_closed_pnl_usdc)
                .unwrap_or(Decimal::ZERO)
                .round_dp(4)
                .normalize()
                .to_string();
        }

        let outcome_rows = sqlx::query(
            "SELECT
                 strategy.strategy,
                 SUM(CASE WHEN positions.asset IS NOT NULL THEN 1 ELSE 0 END) AS filled_total,
                 SUM(CASE WHEN positions.asset IS NOT NULL AND strategy.side = strategy.outcome THEN 1 ELSE 0 END) AS closed_win_count,
                 SUM(CASE WHEN positions.asset IS NOT NULL AND strategy.side <> strategy.outcome THEN 1 ELSE 0 END) AS closed_loss_count,
                 SUM(CASE WHEN positions.asset IS NULL THEN 1 ELSE 0 END) AS missed_count,
                 SUM(CASE WHEN positions.asset IS NULL AND strategy.side = strategy.outcome THEN 1 ELSE 0 END) AS missed_win_count,
                 SUM(CASE WHEN positions.asset IS NULL AND strategy.side <> strategy.outcome THEN 1 ELSE 0 END) AS missed_loss_count
             FROM strategy
             LEFT JOIN positions
             ON positions.asset = strategy.asset_id
             WHERE TRIM(strategy.outcome) <> ''
             GROUP BY strategy.strategy",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            PolyfillError::internal_simple(format!("读取 info outcome 统计失败: {error}"))
        })?;

        for row in outcome_rows {
            let strategy = required_text(&row, "strategy", "info.strategy")?;
            let closed_count: i64 = required_value(&row, "filled_total", "info.filled_total")?;
            let closed_win_count: i64 =
                required_value(&row, "closed_win_count", "info.closed_win_count")?;
            let closed_loss_count: i64 =
                required_value(&row, "closed_loss_count", "info.closed_loss_count")?;
            let missed_count: i64 = required_value(&row, "missed_count", "info.missed_count")?;
            let missed_win_count: i64 =
                required_value(&row, "missed_win_count", "info.missed_win_count")?;
            let missed_loss_count: i64 =
                required_value(&row, "missed_loss_count", "info.missed_loss_count")?;
            let stats = strategies
                .entry(strategy.clone())
                .or_insert_with(|| StrategyInfoStats {
                    strategy,
                    trigger_count: 0,
                    closed_count: 0,
                    today_closed_count: 0,
                    today_closed_pnl_usdc: "0".to_string(),
                    closed_win_count: 0,
                    closed_loss_count: 0,
                    missed_count: 0,
                    missed_win_count: 0,
                    missed_loss_count: 0,
                    settled_pnl_usdc: "0".to_string(),
                });
            stats.closed_win_count = closed_win_count.max(0) as usize;
            stats.closed_loss_count = closed_loss_count.max(0) as usize;
            stats.missed_count = missed_count.max(0) as usize;
            stats.missed_win_count = missed_win_count.max(0) as usize;
            stats.missed_loss_count = missed_loss_count.max(0) as usize;
            if stats.closed_count == 0 {
                stats.closed_count = closed_count.max(0) as usize;
            }
        }

        let settled_rows = sqlx::query(&format!(
            "{LATEST_TRACKED_ASSETS_CTE_SQL}
             SELECT
                 tracked.strategy,
                 COUNT(*) AS closed_count,
                 COALESCE(SUM(CAST(
                     COALESCE(
                         NULLIF(positions.cash_pnl, ''),
                         NULLIF(positions.realized_pnl, ''),
                         '0'
                     ) AS REAL
                 )), 0) AS settled_pnl_usdc
             FROM positions
             JOIN latest_tracked AS tracked
             ON tracked.asset_id = positions.asset
             GROUP BY tracked.strategy"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            PolyfillError::internal_simple(format!("读取 info settled 统计失败: {error}"))
        })?;

        for row in settled_rows {
            let strategy = required_text(&row, "strategy", "info.strategy")?;
            let closed_count: i64 = required_value(&row, "closed_count", "info.closed_count")?;
            let settled_pnl_usdc: f64 =
                required_value(&row, "settled_pnl_usdc", "info.settled_pnl_usdc")?;
            let stats = strategies
                .entry(strategy.clone())
                .or_insert_with(|| StrategyInfoStats {
                    strategy,
                    trigger_count: 0,
                    closed_count: 0,
                    today_closed_count: 0,
                    today_closed_pnl_usdc: "0".to_string(),
                    closed_win_count: 0,
                    closed_loss_count: 0,
                    missed_count: 0,
                    missed_win_count: 0,
                    missed_loss_count: 0,
                    settled_pnl_usdc: "0".to_string(),
                });
            stats.closed_count = closed_count.max(0) as usize;
            stats.settled_pnl_usdc = Decimal::from_f64_retain(settled_pnl_usdc)
                .unwrap_or(Decimal::ZERO)
                .round_dp(4)
                .normalize()
                .to_string();
        }

        let strategies: Vec<_> = strategies.into_values().collect();

        Ok(InfoStats {
            trigger_count: strategies.iter().map(|stats| stats.trigger_count).sum(),
            closed_count: strategies.iter().map(|stats| stats.closed_count).sum(),
            today_closed_count: strategies
                .iter()
                .map(|stats| stats.today_closed_count)
                .sum(),
            today_closed_pnl_usdc: strategies
                .iter()
                .fold(Decimal::ZERO, |acc, stats| {
                    acc + Decimal::from_str(&stats.today_closed_pnl_usdc).unwrap_or(Decimal::ZERO)
                })
                .round_dp(4)
                .normalize()
                .to_string(),
            closed_win_count: strategies.iter().map(|stats| stats.closed_win_count).sum(),
            closed_loss_count: strategies.iter().map(|stats| stats.closed_loss_count).sum(),
            missed_count: strategies.iter().map(|stats| stats.missed_count).sum(),
            missed_win_count: strategies.iter().map(|stats| stats.missed_win_count).sum(),
            missed_loss_count: strategies.iter().map(|stats| stats.missed_loss_count).sum(),
            strategies,
        })
    }

    pub async fn select_positions_page(
        &self,
        strategy: &str,
        range: &str,
        page: usize,
        page_size: usize,
    ) -> Result<PositionsPage> {
        let safe_page_size = page_size.clamp(1, 100);
        let safe_page = page.max(1);
        let min_timestamp = match range {
            "1d" => Some(
                chrono::Utc::now().timestamp_millis()
                    - chrono::Duration::days(1).num_milliseconds(),
            ),
            "1w" => Some(
                chrono::Utc::now().timestamp_millis()
                    - chrono::Duration::weeks(1).num_milliseconds(),
            ),
            "1m" => Some(
                chrono::Utc::now().timestamp_millis()
                    - chrono::Duration::days(30).num_milliseconds(),
            ),
            _ => None,
        };

        let total: i64 = sqlx::query_scalar(&format!(
            "{LATEST_TRACKED_ASSETS_CTE_SQL}
             SELECT COUNT(*)
             FROM positions
             JOIN latest_tracked AS tracked
             ON tracked.asset_id = positions.asset
             WHERE tracked.strategy = ?1
               AND (?2 IS NULL OR
                    CASE
                        WHEN positions.timestamp >= 1000000000000
                            THEN positions.timestamp
                        ELSE positions.timestamp * 1000
                    END >= ?2)"
        ))
        .bind(strategy)
        .bind(min_timestamp)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| {
            PolyfillError::internal_simple(format!("读取 positions 总数失败: {error}"))
        })?;

        let total = total.max(0) as usize;
        let total_pages = total.div_ceil(safe_page_size);
        let page = if total_pages == 0 {
            1
        } else {
            safe_page.min(total_pages)
        };
        let offset = ((page - 1) * safe_page_size) as i64;

        let rows = sqlx::query(&format!(
            "{LATEST_TRACKED_ASSETS_CTE_SQL}
             SELECT
                 tracked.strategy,
                 positions.proxy_wallet,
                 positions.asset,
                 positions.condition_id,
                 positions.market_slug,
                 positions.outcome,
                 positions.avg_price,
                 positions.size,
                 positions.total_bought,
                 positions.current_value,
                 positions.cash_pnl,
                 positions.realized_pnl,
                 positions.cur_price,
                 positions.end_date,
                 positions.timestamp
             FROM positions
             JOIN latest_tracked AS tracked
             ON tracked.asset_id = positions.asset
             WHERE tracked.strategy = ?1
               AND (?2 IS NULL OR
                    CASE
                        WHEN positions.timestamp >= 1000000000000
                            THEN positions.timestamp
                        ELSE positions.timestamp * 1000
                    END >= ?2)
             ORDER BY positions.timestamp DESC
             LIMIT ?3 OFFSET ?4"
        ))
        .bind(strategy)
        .bind(min_timestamp)
        .bind(safe_page_size as i64)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            PolyfillError::internal_simple(format!("读取 positions 列表失败: {error}"))
        })?
        .into_iter()
        .map(parse_position_row)
        .collect::<Result<Vec<_>>>()?;

        Ok(PositionsPage {
            strategy: strategy.to_string(),
            range: range.to_string(),
            page,
            page_size: safe_page_size,
            total,
            total_pages,
            rows,
        })
    }

    async fn execute_write<'q>(
        &self,
        query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
        error_context: &str,
    ) -> Result<()> {
        query
            .execute(&self.pool)
            .await
            .map_err(|error| PolyfillError::internal_simple(format!("{error_context}: {error}")))?;
        Ok(())
    }

    async fn select_order_history(&self, limit: usize) -> Result<Vec<events::Order>> {
        let rows = sqlx::query(
            "SELECT order_id, side, price, size, status, created_at
             FROM orders
             ORDER BY created_at DESC
             LIMIT ?1",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            PolyfillError::internal_simple(format!("读取 orders 历史失败: {error}"))
        })?;

        rows.into_iter().map(parse_order_row).collect()
    }

    async fn select_trade_history(&self, limit: usize) -> Result<Vec<events::Trade>> {
        let rows = sqlx::query(
            "SELECT id, order_id, trade_id, side, price, size, fee_bps, event_time, created_at
             FROM trades
             ORDER BY created_at DESC
             LIMIT ?1",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            PolyfillError::internal_simple(format!("读取 trades 历史失败: {error}"))
        })?;

        rows.into_iter().map(parse_trade_row).collect()
    }

    async fn select_strategy_rows(
        &self,
        limit: Option<usize>,
        error_context: &str,
    ) -> Result<Vec<events::Strategy>> {
        let rows = match limit {
            Some(limit) => sqlx::query(
                "SELECT order_id, asset_id, strategy, symbol, interval, market_slug, side, outcome, created_at, event
                     FROM strategy
                     ORDER BY created_at DESC
                     LIMIT ?1",
            )
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await,
            None => sqlx::query(
                "SELECT order_id, asset_id, strategy, symbol, interval, market_slug, side, outcome, created_at, event
                     FROM strategy
                     ORDER BY created_at DESC",
            )
            .fetch_all(&self.pool)
            .await,
        }
        .map_err(|error| PolyfillError::internal_simple(format!("{error_context}: {error}")))?;

        rows.into_iter().map(parse_strategy_row).collect()
    }
}

fn parse_strategy_row(row: sqlx::sqlite::SqliteRow) -> Result<events::Strategy> {
    Ok(events::Strategy {
        order_id: required_text(&row, "order_id", "strategy.order_id")?,
        asset_id: required_text(&row, "asset_id", "strategy.asset_id")?,
        strategy: required_text(&row, "strategy", "strategy.strategy")?,
        symbol: required_text(&row, "symbol", "strategy.symbol")?,
        interval: required_text(&row, "interval", "strategy.interval")?,
        market_slug: required_text(&row, "market_slug", "strategy.market_slug")?,
        side: required_text(&row, "side", "strategy.side")?,
        outcome: required_text(&row, "outcome", "strategy.outcome")?,
        created_at: required_value(&row, "created_at", "strategy.created_at")?,
        event: required_text(&row, "event", "strategy.event")?,
    })
}

fn parse_order_row(row: sqlx::sqlite::SqliteRow) -> Result<events::Order> {
    Ok(events::Order {
        order_id: required_text(&row, "order_id", "orders.order_id")?,
        side: required_text(&row, "side", "orders.side")?,
        price: required_decimal(&row, "price", "orders.price")?,
        size: required_decimal(&row, "size", "orders.size")?,
        status: required_text(&row, "status", "orders.status")?,
        created_at: required_value(&row, "created_at", "orders.created_at")?,
    })
}

fn parse_trade_row(row: sqlx::sqlite::SqliteRow) -> Result<events::Trade> {
    Ok(events::Trade {
        id: required_text(&row, "id", "trades.id")?,
        order_id: optional_value(&row, "order_id", "trades.order_id")?,
        trade_id: required_text(&row, "trade_id", "trades.trade_id")?,
        side: required_text(&row, "side", "trades.side")?,
        price: required_decimal(&row, "price", "trades.price")?,
        size: required_decimal(&row, "size", "trades.size")?,
        fee_bps: optional_decimal(&row, "fee_bps", "trades.fee_bps")?,
        event_time: optional_value(&row, "event_time", "trades.event_time")?,
        created_at: required_value(&row, "created_at", "trades.created_at")?,
    })
}

fn parse_position_row(row: sqlx::sqlite::SqliteRow) -> Result<PositionRecord> {
    Ok(PositionRecord {
        strategy: required_text(&row, "strategy", "positions.strategy")?,
        proxy_wallet: required_text(&row, "proxy_wallet", "positions.proxy_wallet")?,
        asset: required_text(&row, "asset", "positions.asset")?,
        condition_id: required_text(&row, "condition_id", "positions.condition_id")?,
        market_slug: required_text(&row, "market_slug", "positions.market_slug")?,
        outcome: required_text(&row, "outcome", "positions.outcome")?,
        avg_price: required_text(&row, "avg_price", "positions.avg_price")?,
        size: optional_value(&row, "size", "positions.size")?,
        total_bought: optional_value(&row, "total_bought", "positions.total_bought")?,
        current_value: optional_value(&row, "current_value", "positions.current_value")?,
        cash_pnl: optional_value(&row, "cash_pnl", "positions.cash_pnl")?,
        realized_pnl: required_text(&row, "realized_pnl", "positions.realized_pnl")?,
        cur_price: required_text(&row, "cur_price", "positions.cur_price")?,
        end_date: required_text(&row, "end_date", "positions.end_date")?,
        timestamp: required_value(&row, "timestamp", "positions.timestamp")?,
    })
}

fn parse_decimal(raw: String, field: &str) -> Result<Decimal> {
    Decimal::from_str(&raw).map_err(|error| {
        PolyfillError::internal_simple(format!("{field} 解析 Decimal 失败: {error}"))
    })
}

fn required_decimal(row: &sqlx::sqlite::SqliteRow, column: &str, field: &str) -> Result<Decimal> {
    parse_decimal(required_text(row, column, field)?, field)
}

fn optional_decimal(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
    field: &str,
) -> Result<Option<Decimal>> {
    optional_value::<String>(row, column, field)?
        .map(|raw| parse_decimal(raw, field))
        .transpose()
}

fn required_text(row: &sqlx::sqlite::SqliteRow, column: &str, field: &str) -> Result<String> {
    required_value(row, column, field)
}

fn required_value<'r, T>(row: &'r sqlx::sqlite::SqliteRow, column: &str, field: &str) -> Result<T>
where
    T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    row.try_get(column)
        .map_err(|error| parse_error(field, error))
}

fn optional_value<'r, T>(
    row: &'r sqlx::sqlite::SqliteRow,
    column: &str,
    field: &str,
) -> Result<Option<T>>
where
    T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    row.try_get(column)
        .map_err(|error| parse_error(field, error))
}

fn parse_error(field: &str, error: sqlx::Error) -> PolyfillError {
    PolyfillError::internal_simple(format!("读取 {field} 失败: {error}"))
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;
    use sqlx::Row;

    use super::*;

    #[tokio::test]
    async fn store_runs_init_sql() {
        let store = Store::open_memory()
            .await
            .expect("memory sqlite should open");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('orders', 'trades', 'strategy', 'positions')")
            .fetch_one(&store.pool)
            .await
            .expect("table count should read");
        assert_eq!(count, 4);
    }

    #[tokio::test]
    async fn store_persists_order_and_trade() {
        let store = Store::open_memory()
            .await
            .expect("memory sqlite should open");

        store
            .insert_order(&events::Order {
                order_id: "order-1".to_string(),
                side: "buy".to_string(),
                price: Decimal::new(52, 2),
                size: Decimal::new(10, 0),
                status: "live".to_string(),
                created_at: 1_700_000_000_200,
            })
            .await
            .expect("order insert should work");

        store
            .insert_trade(&events::Trade {
                id: "trade-event-1".to_string(),
                order_id: Some("order-1".to_string()),
                trade_id: "trade-1".to_string(),
                side: "buy".to_string(),
                price: Decimal::new(52, 2),
                size: Decimal::new(10, 0),
                fee_bps: Some(Decimal::new(25, 0)),
                event_time: Some(1_700_000_000_300),
                created_at: 1_700_000_000_300,
            })
            .await
            .expect("trade insert should work");

        let order_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM orders")
            .fetch_one(&store.pool)
            .await
            .expect("order count should read");
        let trade_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&store.pool)
            .await
            .expect("trade count should read");

        assert_eq!(order_count, 1);
        assert_eq!(trade_count, 1);
    }

    #[tokio::test]
    async fn strategy_row_keeps_order_context() {
        let store = Store::open_memory()
            .await
            .expect("memory sqlite should open");

        store
            .insert_strategy(&events::Strategy {
                order_id: "order-1".to_string(),
                asset_id: "123".to_string(),
                strategy: "crypto_reversal".to_string(),
                symbol: "eth".to_string(),
                interval: "5m".to_string(),
                market_slug: "eth-updown-5m-test".to_string(),
                side: "up".to_string(),
                outcome: "up".to_string(),
                created_at: 21,
                event: "{\"signal_time_ms\":20}".to_string(),
            })
            .await
            .expect("strategy insert should work");

        let row = sqlx::query(
            "SELECT asset_id, strategy, symbol, interval, market_slug, side, outcome, event FROM strategy WHERE order_id = 'order-1'",
        )
        .fetch_one(&store.pool)
        .await
        .expect("strategy row should read");

        let asset_id: String = row.try_get(0).expect("asset_id should read");
        let strategy: Option<String> = row.try_get(1).expect("strategy should read");
        let symbol: String = row.try_get(2).expect("symbol should read");
        let interval: String = row.try_get(3).expect("interval should read");
        let market_slug: String = row.try_get(4).expect("market_slug should read");
        let side: String = row.try_get(5).expect("side should read");
        let outcome: String = row.try_get(6).expect("outcome should read");
        let event: String = row.try_get(7).expect("event should read");

        assert_eq!(asset_id, "123");
        assert_eq!(strategy.as_deref(), Some("crypto_reversal"));
        assert_eq!(symbol, "eth");
        assert_eq!(interval, "5m");
        assert_eq!(market_slug, "eth-updown-5m-test");
        assert_eq!(side, "up");
        assert_eq!(outcome, "up");
        assert_eq!(event, "{\"signal_time_ms\":20}");
    }

    #[tokio::test]
    async fn select_dashboard_history_returns_rows() {
        let store = Store::open_memory()
            .await
            .expect("memory sqlite should open");

        store
            .insert_strategy(&events::Strategy {
                order_id: "order-1".to_string(),
                asset_id: "asset-1".to_string(),
                strategy: "crypto_reversal".to_string(),
                symbol: "eth".to_string(),
                interval: "5m".to_string(),
                market_slug: "eth-updown-5m-test".to_string(),
                side: "up".to_string(),
                outcome: String::new(),
                created_at: 100,
                event: "{\"signal_time_ms\":20}".to_string(),
            })
            .await
            .expect("strategy insert should work");
        store
            .insert_order(&events::Order {
                order_id: "order-1".to_string(),
                side: "buy".to_string(),
                price: Decimal::new(51, 2),
                size: Decimal::new(8, 0),
                status: "live".to_string(),
                created_at: 101,
            })
            .await
            .expect("order insert should work");
        store
            .insert_trade(&events::Trade {
                id: "trade-event-1".to_string(),
                order_id: Some("order-1".to_string()),
                trade_id: "trade-1".to_string(),
                side: "buy".to_string(),
                price: Decimal::new(51, 2),
                size: Decimal::new(8, 0),
                fee_bps: None,
                event_time: Some(102),
                created_at: 102,
            })
            .await
            .expect("trade insert should work");

        let history = store
            .select_dashboard_history(8)
            .await
            .expect("history should load");

        assert_eq!(history.strategies.len(), 1);
        assert_eq!(history.orders.len(), 1);
        assert_eq!(history.trades.len(), 1);
        assert_eq!(history.strategies[0].order_id, "order-1");
        assert_eq!(history.orders[0].status, "live");
        assert_eq!(history.trades[0].trade_id, "trade-1");
    }

    #[tokio::test]
    async fn store_updates_order_to_latest_status() {
        let store = Store::open_memory()
            .await
            .expect("memory sqlite should open");

        store
            .insert_order(&events::Order {
                order_id: "order-1".to_string(),
                side: "buy".to_string(),
                price: Decimal::new(51, 2),
                size: Decimal::new(8, 0),
                status: "live".to_string(),
                created_at: 100,
            })
            .await
            .expect("live order insert should work");
        store
            .insert_order(&events::Order {
                order_id: "order-1".to_string(),
                side: "buy".to_string(),
                price: Decimal::new(51, 2),
                size: Decimal::new(8, 0),
                status: "matched".to_string(),
                created_at: 101,
            })
            .await
            .expect("matched order insert should work");

        let order_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM orders WHERE order_id = 'order-1'")
                .fetch_one(&store.pool)
                .await
                .expect("order count should read");
        let status: String =
            sqlx::query_scalar("SELECT status FROM orders WHERE order_id = 'order-1'")
                .fetch_one(&store.pool)
                .await
                .expect("order status should read");

        assert_eq!(order_count, 1);
        assert_eq!(status, "matched");
    }

    #[tokio::test]
    async fn insert_positions_only_inserts_tracked_assets_once() {
        let store = Store::open_memory()
            .await
            .expect("memory sqlite should open");
        store
            .insert_strategy(&events::Strategy {
                order_id: "order-1".to_string(),
                asset_id: "123".to_string(),
                strategy: "crypto_reversal".to_string(),
                symbol: "eth".to_string(),
                interval: "5m".to_string(),
                market_slug: "eth-updown-5m-old".to_string(),
                side: "up".to_string(),
                outcome: String::new(),
                created_at: 21,
                event: "{\"signal_time_ms\":20}".to_string(),
            })
            .await
            .expect("strategy insert should work");
        let position: Position = serde_json::from_value(serde_json::json!({
            "proxyWallet": "0x0000000000000000000000000000000000000000",
            "asset": "123",
            "conditionId": format!("0x{}", "0".repeat(64)),
            "size": Decimal::new(8, 0),
            "avgPrice": Decimal::new(43, 2),
            "initialValue": Decimal::new(344, 2),
            "currentValue": Decimal::ONE,
            "cashPnl": Decimal::new(57, 2),
            "percentPnl": Decimal::ZERO,
            "totalBought": Decimal::new(8, 0),
            "realizedPnl": Decimal::new(57, 2),
            "percentRealizedPnl": Decimal::ZERO,
            "curPrice": Decimal::ONE,
            "redeemable": true,
            "mergeable": false,
            "title": "ETH test",
            "slug": "eth-updown-5m-old",
            "icon": "",
            "eventSlug": "eth",
            "eventId": "",
            "outcome": "Yes",
            "outcomeIndex": 0,
            "oppositeOutcome": "No",
            "oppositeAsset": "456",
            "endDate": "2026-03-25",
            "negativeRisk": false,
        }))
        .expect("position should deserialize");

        store
            .insert_positions_at(&[position.clone(), position], 1710003600000_i64)
            .await
            .expect("positions insert should work");

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM positions WHERE asset = '123'")
            .fetch_one(&store.pool)
            .await
            .expect("positions count should read");

        assert_eq!(count, 1);

        let unknown: Position = serde_json::from_value(serde_json::json!({
            "proxyWallet": "0x0000000000000000000000000000000000000000",
            "asset": "999",
            "conditionId": format!("0x{}", "0".repeat(64)),
            "size": Decimal::new(8, 0),
            "avgPrice": Decimal::new(43, 2),
            "initialValue": Decimal::new(344, 2),
            "currentValue": Decimal::ONE,
            "cashPnl": Decimal::new(57, 2),
            "percentPnl": Decimal::ZERO,
            "totalBought": Decimal::new(8, 0),
            "realizedPnl": Decimal::new(57, 2),
            "percentRealizedPnl": Decimal::ZERO,
            "curPrice": Decimal::ONE,
            "redeemable": true,
            "mergeable": false,
            "title": "ETH test",
            "slug": "eth-updown-5m-old",
            "icon": "",
            "eventSlug": "eth",
            "eventId": "",
            "outcome": "Yes",
            "outcomeIndex": 0,
            "oppositeOutcome": "No",
            "oppositeAsset": "456",
            "endDate": "2026-03-25",
            "negativeRisk": false,
        }))
        .expect("unknown position should deserialize");

        store
            .insert_positions_at(&[unknown], 1710003600000_i64)
            .await
            .expect("unknown positions insert should work");

        let unknown_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM positions WHERE asset = '999'")
                .fetch_one(&store.pool)
                .await
                .expect("unknown positions count should read");

        assert_eq!(unknown_count, 0);
    }

    #[tokio::test]
    async fn select_info_stats_returns_trigger_and_closed_counts() {
        let store = Store::open_memory()
            .await
            .expect("memory sqlite should open");

        store
            .insert_strategy(&events::Strategy {
                order_id: "order-1".to_string(),
                asset_id: "asset-1".to_string(),
                strategy: "crypto_reversal".to_string(),
                symbol: "eth".to_string(),
                interval: "5m".to_string(),
                market_slug: "eth-updown-5m-test".to_string(),
                side: "up".to_string(),
                outcome: String::new(),
                created_at: 100,
                event: "{}".to_string(),
            })
            .await
            .expect("first strategy insert should work");
        store
            .insert_strategy(&events::Strategy {
                order_id: "order-2".to_string(),
                asset_id: "456".to_string(),
                strategy: "crypto_reversal".to_string(),
                symbol: "eth".to_string(),
                interval: "15m".to_string(),
                market_slug: "eth-updown-15m-test".to_string(),
                side: "down".to_string(),
                outcome: String::new(),
                created_at: 101,
                event: "{}".to_string(),
            })
            .await
            .expect("second strategy insert should work");

        let timestamp = chrono::Utc::now().timestamp_millis();
        let position: Position = serde_json::from_value(serde_json::json!({
            "proxyWallet": "0x0000000000000000000000000000000000000000",
            "asset": "456",
            "conditionId": format!("0x{}", "0".repeat(64)),
            "size": Decimal::new(8, 0),
            "avgPrice": Decimal::new(43, 2),
            "initialValue": Decimal::new(344, 2),
            "currentValue": Decimal::ONE,
            "cashPnl": Decimal::new(57, 2),
            "percentPnl": Decimal::ZERO,
            "totalBought": Decimal::new(8, 0),
            "realizedPnl": Decimal::new(57, 2),
            "percentRealizedPnl": Decimal::ZERO,
            "curPrice": Decimal::ONE,
            "redeemable": true,
            "mergeable": false,
            "title": "ETH test",
            "slug": "eth-updown-15m-test",
            "icon": "",
            "eventSlug": "eth",
            "eventId": "",
            "outcome": "Yes",
            "outcomeIndex": 0,
            "oppositeOutcome": "No",
            "oppositeAsset": "456",
            "endDate": chrono::Utc::now().date_naive().to_string(),
            "negativeRisk": false,
        }))
        .expect("position should deserialize");

        store
            .insert_positions_at(&[position], timestamp)
            .await
            .expect("position insert should work");

        let stats = store
            .select_info_stats()
            .await
            .expect("info stats should load");

        assert_eq!(stats.trigger_count, 2);
        assert_eq!(stats.closed_count, 1);
        assert_eq!(stats.today_closed_count, 1);
        assert_eq!(stats.strategies.len(), 1);
        assert_eq!(stats.strategies[0].strategy, "crypto_reversal");
        assert_eq!(stats.strategies[0].trigger_count, 2);
        assert_eq!(stats.strategies[0].closed_count, 1);
        assert_eq!(stats.strategies[0].today_closed_count, 1);
    }

    #[tokio::test]
    async fn select_pending_strategy_outcomes_only_returns_settled_markets() {
        let store = Store::open_memory()
            .await
            .expect("memory sqlite should open");
        let now_ms = chrono::Utc::now().timestamp_millis();

        store
            .insert_strategy(&events::Strategy {
                order_id: "order-5m-ready".to_string(),
                asset_id: "asset-5m-ready".to_string(),
                strategy: "crypto_reversal".to_string(),
                symbol: "eth".to_string(),
                interval: "5m".to_string(),
                market_slug: "eth-updown-5m-ready".to_string(),
                side: "up".to_string(),
                outcome: String::new(),
                created_at: now_ms - 600_000,
                event: "{}".to_string(),
            })
            .await
            .expect("ready strategy insert should work");

        store
            .insert_strategy(&events::Strategy {
                order_id: "order-15m-early".to_string(),
                asset_id: "asset-15m-early".to_string(),
                strategy: "crypto_reversal".to_string(),
                symbol: "eth".to_string(),
                interval: "15m".to_string(),
                market_slug: "eth-updown-15m-early".to_string(),
                side: "down".to_string(),
                outcome: String::new(),
                created_at: now_ms - 120_000,
                event: "{}".to_string(),
            })
            .await
            .expect("early strategy insert should work");

        let pending = store
            .select_pending_strategy_outcomes()
            .await
            .expect("pending outcomes should load");

        assert!(pending.iter().any(|slug| slug == "eth-updown-5m-ready"));
        assert!(!pending.iter().any(|slug| slug == "eth-updown-15m-early"));
    }

    #[tokio::test]
    async fn select_info_stats_does_not_double_count_settled_pnl_for_reused_asset() {
        let store = Store::open_memory()
            .await
            .expect("memory sqlite should open");

        store
            .insert_strategy(&events::Strategy {
                order_id: "order-1".to_string(),
                asset_id: "123".to_string(),
                strategy: "old_strategy".to_string(),
                symbol: "eth".to_string(),
                interval: "5m".to_string(),
                market_slug: "eth-updown-5m-old".to_string(),
                side: "up".to_string(),
                outcome: "up".to_string(),
                created_at: 100,
                event: "{}".to_string(),
            })
            .await
            .expect("old strategy insert should work");
        store
            .insert_strategy(&events::Strategy {
                order_id: "order-2".to_string(),
                asset_id: "123".to_string(),
                strategy: "new_strategy".to_string(),
                symbol: "eth".to_string(),
                interval: "15m".to_string(),
                market_slug: "eth-updown-15m-new".to_string(),
                side: "down".to_string(),
                outcome: "down".to_string(),
                created_at: 200,
                event: "{}".to_string(),
            })
            .await
            .expect("new strategy insert should work");

        let position: Position = serde_json::from_value(serde_json::json!({
            "proxyWallet": "0x0000000000000000000000000000000000000000",
            "asset": "123",
            "conditionId": format!("0x{}", "0".repeat(64)),
            "size": Decimal::new(8, 0),
            "avgPrice": Decimal::new(50, 2),
            "initialValue": Decimal::new(400, 2),
            "currentValue": Decimal::ONE,
            "cashPnl": Decimal::new(125, 2),
            "percentPnl": Decimal::ZERO,
            "totalBought": Decimal::new(8, 0),
            "realizedPnl": Decimal::new(125, 2),
            "percentRealizedPnl": Decimal::ZERO,
            "curPrice": Decimal::ONE,
            "redeemable": true,
            "mergeable": false,
            "title": "ETH test",
            "slug": "eth-updown-15m-new",
            "icon": "",
            "eventSlug": "eth",
            "eventId": "",
            "outcome": "Yes",
            "outcomeIndex": 0,
            "oppositeOutcome": "No",
            "oppositeAsset": "456",
            "endDate": "2026-03-25",
            "negativeRisk": false,
        }))
        .expect("position should deserialize");

        store
            .insert_positions_at(&[position], chrono::Utc::now().timestamp_millis())
            .await
            .expect("position insert should work");

        let stats = store
            .select_info_stats()
            .await
            .expect("info stats should load");

        let old = stats
            .strategies
            .iter()
            .find(|item| item.strategy == "old_strategy")
            .expect("old strategy stats should exist");
        let new = stats
            .strategies
            .iter()
            .find(|item| item.strategy == "new_strategy")
            .expect("new strategy stats should exist");

        assert_eq!(old.closed_count, 1);
        assert_eq!(old.settled_pnl_usdc, "0");
        assert_eq!(new.closed_count, 1);
        assert_eq!(new.settled_pnl_usdc, "1.25");
    }
}
