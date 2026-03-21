use std::fs;
use std::path::Path;
use std::str::FromStr;

use crate::errors::{PolyfillError, Result};
use crate::types::crypto::Symbol;

const DEFAULT_ENV_PATH: &str = ".env";
const DEFAULT_CLOB_HOST: &str = "https://clob.polymarket.com";
const PRIVATE_KEY_ENV: &str = "PRIVATE_KEY";
const SYMBOLS_ENV: &str = "SYMBOLS";

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

pub fn required_env(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| PolyfillError::validation(format!("缺少环境变量 {key}")))
}

#[derive(Debug, Clone)]
pub struct TradingConfig {
    pub host: String,
    pub private_key: String,
    pub symbols: Vec<Symbol>,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub trading: TradingConfig,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        load_env_file(DEFAULT_ENV_PATH)?;

        let trading = TradingConfig {
            host: DEFAULT_CLOB_HOST.to_string(),
            private_key: required_env(PRIVATE_KEY_ENV)?,
            symbols: optional_symbols_env(SYMBOLS_ENV)?.unwrap_or_else(|| vec![Symbol::Btc]),
        };

        Ok(Self { trading })
    }
}

fn optional_symbols_env(key: &str) -> Result<Option<Vec<Symbol>>> {
    let Some(raw) = std::env::var_os(key) else {
        return Ok(None);
    };

    let raw = raw.to_string_lossy();
    let symbols = raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| Symbol::from_str(part))
        .collect::<Result<Vec<_>>>()?;

    if symbols.is_empty() {
        return Err(PolyfillError::validation(format!(
            "环境变量 {key} 不能为空，例如 btc,eth,sol"
        )));
    }

    Ok(Some(symbols))
}
