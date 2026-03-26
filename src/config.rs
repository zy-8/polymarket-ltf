use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use rust_decimal::Decimal;

use crate::errors::{PolyfillError, Result};
use crate::types::crypto::{Interval, Symbol};

const DEFAULT_ENV_PATH: &str = ".env";
const DEFAULT_CLOB_HOST: &str = "https://clob.polymarket.com";
const DEFAULT_SQLITE_PATH: &str = "data/runtime/events.sqlite3";
const DEFAULT_ALLOW_ORDER_USDC: f64 = 4.0;
const DEFAULT_REDUCE_ORDER_USDC: f64 = 3.0;

const PRIVATE_KEY_ENV: &str = "PRIVATE_KEY";
const HOST_ENV: &str = "CLOB_HOST";
const SYMBOLS_ENV: &str = "SYMBOLS";

const INTERVALS_ENV: &str = "INTERVALS";
const SQLITE_PATH_ENV: &str = "SQLITE_PATH";
const SCAN_INTERVAL_MS_ENV: &str = "SCAN_INTERVAL_MS";
const ALLOW_ORDER_USDC_ENV: &str = "ALLOW_ORDER_USDC";
const REDUCE_ORDER_USDC_ENV: &str = "REDUCE_ORDER_USDC";
const CRYPTO_REVERSAL_ORDER_PRICE_ENV: &str = "CRYPTO_REVERSAL_ORDER_PRICE";

/// 加载本地 `.env` 文件。
///
/// 规则保持克制：
/// - 文件不存在时直接跳过；
/// - 已存在的系统环境变量不覆盖；
/// - 只支持 `KEY=VALUE` 的简单行格式。
pub fn load_env_file(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(());
    };

    for (index, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            return Err(PolyfillError::validation(format!(
                ".env 格式错误: {}:{} 缺少 =",
                path.display(),
                index + 1
            )));
        };

        let key = key.trim();
        if key.is_empty() {
            return Err(PolyfillError::validation(format!(
                ".env 格式错误: {}:{} key 不能为空",
                path.display(),
                index + 1
            )));
        }

        if std::env::var_os(key).is_some() {
            continue;
        }

        let value = value.trim().trim_matches('"').trim_matches('\'');
        unsafe {
            std::env::set_var(key, value);
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub struct TradingConfig {
    pub host: String,
    pub private_key: String,
    pub symbols: Vec<Symbol>,
}

impl TradingConfig {
    fn from_env(env: &Env) -> Result<Self> {
        Ok(Self {
            host: env
                .string(HOST_ENV)?
                .unwrap_or_else(|| DEFAULT_CLOB_HOST.to_string()),
            private_key: env.required_string(PRIVATE_KEY_ENV)?,
            symbols: env
                .csv(SYMBOLS_ENV, Symbol::from_str)?
                .unwrap_or_else(|| vec![Symbol::Btc]),
        })
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub intervals: Vec<Interval>,
    pub sqlite_path: PathBuf,
    pub scan_interval: Duration,
    pub allow_order_usdc: f64,
    pub reduce_order_usdc: f64,
    pub crypto_reversal_order_price: Option<Decimal>,
}

impl RuntimeConfig {
    fn from_env(env: &Env) -> Result<Self> {
        Ok(Self {
            intervals: env
                .csv(INTERVALS_ENV, Interval::from_str)?
                .unwrap_or_else(|| vec![Interval::M5]),
            sqlite_path: env
                .string(SQLITE_PATH_ENV)?
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_SQLITE_PATH)),
            scan_interval: Duration::from_millis(
                env.parsed::<u64>(SCAN_INTERVAL_MS_ENV)?.unwrap_or(1_000),
            ),
            allow_order_usdc: env
                .positive_f64(ALLOW_ORDER_USDC_ENV)?
                .unwrap_or(DEFAULT_ALLOW_ORDER_USDC),
            reduce_order_usdc: env
                .positive_f64(REDUCE_ORDER_USDC_ENV)?
                .unwrap_or(DEFAULT_REDUCE_ORDER_USDC),
            crypto_reversal_order_price: env
                .probability_price_or_zero(CRYPTO_REVERSAL_ORDER_PRICE_ENV)?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub trading: TradingConfig,
    pub runtime: RuntimeConfig,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        load_env_file(DEFAULT_ENV_PATH)?;
        let env = Env;
        Ok(Self {
            trading: TradingConfig::from_env(&env)?,
            runtime: RuntimeConfig::from_env(&env)?,
        })
    }
}

struct Env;

impl Env {
    fn required_string(&self, key: &str) -> Result<String> {
        self.string(key)?
            .ok_or_else(|| PolyfillError::validation(format!("缺少环境变量 {key}")))
    }

    fn string(&self, key: &str) -> Result<Option<String>> {
        let Some(raw) = std::env::var_os(key) else {
            return Ok(None);
        };

        let value = raw.to_string_lossy().trim().to_string();
        if value.is_empty() {
            return Ok(None);
        }

        Ok(Some(value))
    }

    fn parsed<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: FromStr,
        T::Err: std::fmt::Display,
    {
        let Some(raw) = self.string(key)? else {
            return Ok(None);
        };

        raw.parse::<T>()
            .map(Some)
            .map_err(|error| PolyfillError::validation(format!("环境变量 {key} 解析失败: {error}")))
    }

    fn positive_f64(&self, key: &str) -> Result<Option<f64>> {
        let Some(value) = self.parsed::<f64>(key)? else {
            return Ok(None);
        };

        if value > 0.0 {
            Ok(Some(value))
        } else {
            Err(PolyfillError::validation(format!(
                "环境变量 {key} 必须大于 0"
            )))
        }
    }

    fn probability_price_or_zero(&self, key: &str) -> Result<Option<Decimal>> {
        let Some(value) = self.parsed::<Decimal>(key)? else {
            return Ok(None);
        };

        if value < Decimal::ZERO {
            return Err(PolyfillError::validation(format!(
                "环境变量 {key} 不能小于 0"
            )));
        }
        if value > Decimal::ONE {
            return Err(PolyfillError::validation(format!(
                "环境变量 {key} 不能大于 1"
            )));
        }
        if value.is_zero() {
            return Ok(None);
        }

        Ok(Some(value))
    }

    fn csv<T, F>(&self, key: &str, parse: F) -> Result<Option<Vec<T>>>
    where
        F: Fn(&str) -> Result<T>,
    {
        let Some(raw) = self.string(key)? else {
            return Ok(None);
        };

        let values = raw
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(parse)
            .collect::<Result<Vec<_>>>()?;

        if values.is_empty() {
            return Err(PolyfillError::validation(format!(
                "环境变量 {key} 不能为空"
            )));
        }

        Ok(Some(values))
    }
}
