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

use std::path::Path;
use std::str::FromStr;

use rust_decimal::Decimal;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{ConnectOptions, Row, SqlitePool};

use crate::errors::{PolyfillError, Result};
use crate::events;

const INIT_SQL: &str = include_str!("../../schema/init.sql");

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
                id, order_id, trade_id, asset_id, side, price, size, fee_bps, event_time, created_at
            ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )
            .bind(&event.id)
            .bind(&event.order_id)
            .bind(&event.trade_id)
            .bind(&event.asset_id)
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
                order_id, strategy, symbol, interval, market_slug, side, created_at, event
            ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .bind(&event.order_id)
            .bind(&event.strategy)
            .bind(&event.symbol)
            .bind(&event.interval)
            .bind(&event.market_slug)
            .bind(&event.side)
            .bind(event.created_at)
            .bind(&event.event),
            "写入 strategy 失败",
        )
        .await
    }

    pub async fn load_dashboard_history(&self, limit: usize) -> Result<DashboardHistory> {
        Ok(DashboardHistory {
            strategies: self.load_strategy_history(limit).await?,
            orders: self.load_order_history(limit).await?,
            trades: self.load_trade_history(limit).await?,
        })
    }

    pub async fn load_strategy_attribution(&self) -> Result<Vec<events::Strategy>> {
        self.load_strategy_rows(None, "读取 strategy 归因失败")
            .await
    }

    async fn load_strategy_history(&self, limit: usize) -> Result<Vec<events::Strategy>> {
        self.load_strategy_rows(Some(limit), "读取 strategy 历史失败")
            .await
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

    async fn load_order_history(&self, limit: usize) -> Result<Vec<events::Order>> {
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

    async fn load_trade_history(&self, limit: usize) -> Result<Vec<events::Trade>> {
        let rows = sqlx::query(
            "SELECT id, order_id, trade_id, asset_id, side, price, size, fee_bps, event_time, created_at
             FROM trades
             ORDER BY created_at DESC
             LIMIT ?1",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| PolyfillError::internal_simple(format!("读取 trades 历史失败: {error}")))?;

        rows.into_iter().map(parse_trade_row).collect()
    }
}

impl Store {
    async fn load_strategy_rows(
        &self,
        limit: Option<usize>,
        error_context: &str,
    ) -> Result<Vec<events::Strategy>> {
        let rows = match limit {
            Some(limit) => sqlx::query(
                "SELECT order_id, strategy, symbol, interval, market_slug, side, created_at, event
                     FROM strategy
                     ORDER BY created_at DESC
                     LIMIT ?1",
            )
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await,
            None => sqlx::query(
                "SELECT order_id, strategy, symbol, interval, market_slug, side, created_at, event
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
        order_id: row
            .try_get("order_id")
            .map_err(|error| parse_error("strategy.order_id", error))?,
        strategy: row
            .try_get("strategy")
            .map_err(|error| parse_error("strategy.strategy", error))?,
        symbol: row
            .try_get("symbol")
            .map_err(|error| parse_error("strategy.symbol", error))?,
        interval: row
            .try_get("interval")
            .map_err(|error| parse_error("strategy.interval", error))?,
        market_slug: row
            .try_get("market_slug")
            .map_err(|error| parse_error("strategy.market_slug", error))?,
        side: row
            .try_get("side")
            .map_err(|error| parse_error("strategy.side", error))?,
        created_at: row
            .try_get("created_at")
            .map_err(|error| parse_error("strategy.created_at", error))?,
        event: row
            .try_get("event")
            .map_err(|error| parse_error("strategy.event", error))?,
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
        asset_id: required_text(&row, "asset_id", "trades.asset_id")?,
        side: required_text(&row, "side", "trades.side")?,
        price: required_decimal(&row, "price", "trades.price")?,
        size: required_decimal(&row, "size", "trades.size")?,
        fee_bps: optional_decimal(&row, "fee_bps", "trades.fee_bps")?,
        event_time: optional_value(&row, "event_time", "trades.event_time")?,
        created_at: required_value(&row, "created_at", "trades.created_at")?,
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
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('orders', 'trades', 'strategy')")
            .fetch_one(&store.pool)
            .await
            .expect("table count should read");
        assert_eq!(count, 3);
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
                asset_id: "asset-1".to_string(),
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
                strategy: "crypto_reversal".to_string(),
                symbol: "eth".to_string(),
                interval: "5m".to_string(),
                market_slug: "eth-updown-5m-test".to_string(),
                side: "up".to_string(),
                created_at: 21,
                event: "{\"signal_time_ms\":20}".to_string(),
            })
            .await
            .expect("strategy insert should work");

        let row = sqlx::query(
            "SELECT strategy, symbol, interval, market_slug, side, event FROM strategy WHERE order_id = 'order-1'",
        )
        .fetch_one(&store.pool)
        .await
        .expect("strategy row should read");

        let strategy: Option<String> = row.try_get(0).expect("strategy should read");
        let symbol: String = row.try_get(1).expect("symbol should read");
        let interval: String = row.try_get(2).expect("interval should read");
        let market_slug: String = row.try_get(3).expect("market_slug should read");
        let side: String = row.try_get(4).expect("side should read");
        let event: String = row.try_get(5).expect("event should read");

        assert_eq!(strategy.as_deref(), Some("crypto_reversal"));
        assert_eq!(symbol, "eth");
        assert_eq!(interval, "5m");
        assert_eq!(market_slug, "eth-updown-5m-test");
        assert_eq!(side, "up");
        assert_eq!(event, "{\"signal_time_ms\":20}");
    }

    #[tokio::test]
    async fn load_dashboard_history_returns_rows() {
        let store = Store::open_memory()
            .await
            .expect("memory sqlite should open");

        store
            .insert_strategy(&events::Strategy {
                order_id: "order-1".to_string(),
                strategy: "crypto_reversal".to_string(),
                symbol: "eth".to_string(),
                interval: "5m".to_string(),
                market_slug: "eth-updown-5m-test".to_string(),
                side: "up".to_string(),
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
                asset_id: "asset-1".to_string(),
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
            .load_dashboard_history(8)
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
}
