//! Crypto up/down 市场 slug 工具。
//!
//! 当前支持：
//! - `symbol-updown-5m-close_ts`
//! - `symbol-updown-15m-close_ts`

use crate::errors::{PolyfillError, Result};
use crate::types::crypto::{Interval, Symbol};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn current_slug(symbol: Symbol, interval: Interval) -> Result<String> {
    let close_ts = active_close_ts(now_ts()?, interval)?;
    slug(symbol, interval, close_ts)
}

pub fn next_slug(symbol: Symbol, interval: Interval) -> Result<String> {
    let close_ts = active_close_ts(now_ts()?, interval)? + interval.step_secs();
    slug(symbol, interval, close_ts)
}

pub fn slugs_for_hours(symbol: Symbol, interval: Interval, hours: u32) -> Result<Vec<String>> {
    if hours == 0 {
        return Err(PolyfillError::validation("hours 必须大于 0"));
    }

    let now_ts = now_ts()?;
    let mut close_ts = active_close_ts(now_ts, interval)?;
    let end_ts = now_ts + i64::from(hours) * 60 * 60;
    let last_close_ts = align_up(end_ts, interval);
    let mut slugs = Vec::new();

    while close_ts <= last_close_ts {
        slugs.push(slug(symbol, interval, close_ts)?);
        close_ts += interval.step_secs();
    }

    Ok(slugs)
}

fn active_close_ts(now_ts: i64, interval: Interval) -> Result<i64> {
    validate_ts(now_ts)?;

    let step = interval.step_secs();
    Ok((now_ts / step) * step)
}

fn align_up(ts: i64, interval: Interval) -> i64 {
    let step = interval.step_secs();
    ((ts + step - 1) / step) * step
}

fn slug(symbol: Symbol, interval: Interval, close_ts: i64) -> Result<String> {
    validate_ts(close_ts)?;

    let step = interval.step_secs();
    if close_ts % step != 0 {
        return Err(PolyfillError::validation(format!(
            "close_ts {close_ts} 未按 {} 对齐",
            interval.as_slug()
        )));
    }

    Ok(format!(
        "{}-updown-{}-{}",
        symbol.as_slug(),
        interval.as_slug(),
        close_ts
    ))
}

fn validate_ts(ts: i64) -> Result<()> {
    if ts <= 0 {
        return Err(PolyfillError::validation("timestamp 必须为正整数"));
    }

    Ok(())
}

fn now_ts() -> Result<i64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| PolyfillError::internal_simple(format!("系统时间错误: {e}")))?;

    i64::try_from(duration.as_secs())
        .map_err(|_| PolyfillError::internal_simple("当前时间超出 i64 范围"))
}
