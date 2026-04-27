use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use rust_decimal::Decimal;

use crate::errors::{PolyfillError, Result};
use crate::types::crypto::{Interval, Symbol};

const DEFAULT_ENV_PATH: &str = ".env";
const DEFAULT_CLOB_HOST: &str = "https://clob-v2.polymarket.com";
const DEFAULT_SQLITE_PATH: &str = "data/runtime/events.sqlite3";

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
/// - `.env` 内的值覆盖同名系统环境变量；
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

        let value = value.trim().trim_matches('"').trim_matches('\'');
        unsafe {
            std::env::set_var(key, value);
        }
    }

    Ok(())
}

fn env_file_value(path: impl AsRef<Path>, target_key: &str) -> Result<Option<String>> {
    let path = path.as_ref();
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(None);
    };

    let mut matched = None;

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

        if key != target_key {
            continue;
        }

        let value = value.trim().trim_matches('"').trim_matches('\'');
        matched = (!value.is_empty()).then(|| value.to_string());
    }

    Ok(matched)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn temp_env_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("polymarket-ltf-{name}-{nanos}.env"))
    }

    fn clear_env(key: &str) {
        unsafe {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn env_file_overrides_existing_env_var() {
        let _guard = env_lock();
        let path = temp_env_path("override");
        fs::write(&path, "ALLOW_ORDER_USDC=4.0\n").unwrap();
        unsafe {
            std::env::set_var(ALLOW_ORDER_USDC_ENV, "9.0");
        }

        load_env_file(&path).unwrap();

        assert_eq!(std::env::var(ALLOW_ORDER_USDC_ENV).unwrap(), "4.0");

        clear_env(ALLOW_ORDER_USDC_ENV);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn runtime_config_reads_overridden_values_from_env_file() {
        let _guard = env_lock();
        let path = temp_env_path("runtime");
        fs::write(
            &path,
            "ALLOW_ORDER_USDC=4.0\nREDUCE_ORDER_USDC=3.0\nCRYPTO_REVERSAL_ORDER_PRICE=0.42\n",
        )
        .unwrap();
        unsafe {
            std::env::set_var(ALLOW_ORDER_USDC_ENV, "9.0");
            std::env::set_var(REDUCE_ORDER_USDC_ENV, "8.0");
            std::env::set_var(CRYPTO_REVERSAL_ORDER_PRICE_ENV, "0.11");
        }

        load_env_file(&path).unwrap();
        let runtime = RuntimeConfig::from_env(&Env).unwrap();

        assert_eq!(runtime.allow_order_usdc, 4.0);
        assert_eq!(runtime.reduce_order_usdc, 3.0);
        assert_eq!(
            runtime.crypto_reversal_order_price,
            Some(Decimal::from_str("0.42").unwrap())
        );

        clear_env(ALLOW_ORDER_USDC_ENV);
        clear_env(REDUCE_ORDER_USDC_ENV);
        clear_env(CRYPTO_REVERSAL_ORDER_PRICE_ENV);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn runtime_config_requires_order_usdc_values() {
        let _guard = env_lock();
        clear_env(ALLOW_ORDER_USDC_ENV);
        clear_env(REDUCE_ORDER_USDC_ENV);

        let error = RuntimeConfig::from_env(&Env).unwrap_err();

        assert!(error.to_string().contains(ALLOW_ORDER_USDC_ENV));
    }

    #[test]
    fn crypto_reversal_order_price_reads_only_from_env_file() {
        let _guard = env_lock();
        let path = temp_env_path("fixed-price-only-env-file");
        fs::write(&path, "CRYPTO_REVERSAL_ORDER_PRICE=0.42\n").unwrap();
        unsafe {
            std::env::set_var(CRYPTO_REVERSAL_ORDER_PRICE_ENV, "0.11");
        }

        let value = Env
            .probability_price_or_zero_from_env_file(&path, CRYPTO_REVERSAL_ORDER_PRICE_ENV)
            .unwrap();

        assert_eq!(value, Some(Decimal::from_str("0.42").unwrap()));

        clear_env(CRYPTO_REVERSAL_ORDER_PRICE_ENV);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn crypto_reversal_order_price_ignores_process_env_without_env_file_value() {
        let _guard = env_lock();
        let path = temp_env_path("fixed-price-ignore-process-env");
        fs::write(&path, "ALLOW_ORDER_USDC=4.0\n").unwrap();
        unsafe {
            std::env::set_var(CRYPTO_REVERSAL_ORDER_PRICE_ENV, "0.11");
        }

        let value = Env
            .probability_price_or_zero_from_env_file(&path, CRYPTO_REVERSAL_ORDER_PRICE_ENV)
            .unwrap();

        assert_eq!(value, None);

        clear_env(CRYPTO_REVERSAL_ORDER_PRICE_ENV);
        fs::remove_file(path).unwrap();
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
            allow_order_usdc: env.required_positive_f64(ALLOW_ORDER_USDC_ENV)?,
            reduce_order_usdc: env.required_positive_f64(REDUCE_ORDER_USDC_ENV)?,
            crypto_reversal_order_price: None,
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
        let mut runtime = RuntimeConfig::from_env(&env)?;
        runtime.crypto_reversal_order_price = env
            .probability_price_or_zero_from_env_file(
                DEFAULT_ENV_PATH,
                CRYPTO_REVERSAL_ORDER_PRICE_ENV,
            )?;

        Ok(Self {
            trading: TradingConfig::from_env(&env)?,
            runtime,
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

    fn required_positive_f64(&self, key: &str) -> Result<f64> {
        let Some(value) = self.parsed::<f64>(key)? else {
            return Err(PolyfillError::validation(format!("缺少环境变量 {key}")));
        };

        if value > 0.0 {
            Ok(value)
        } else {
            Err(PolyfillError::validation(format!(
                "环境变量 {key} 必须大于 0"
            )))
        }
    }

    fn probability_price_or_zero_from_env_file(
        &self,
        path: impl AsRef<Path>,
        key: &str,
    ) -> Result<Option<Decimal>> {
        let Some(raw) = env_file_value(path, key)? else {
            return Ok(None);
        };

        let value = raw.parse::<Decimal>().map_err(|error| {
            PolyfillError::validation(format!("环境变量 {key} 解析失败: {error}"))
        })?;

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
