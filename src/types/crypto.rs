use std::str::FromStr;

use crate::errors::{PolyfillError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Symbol {
    Btc,
    Eth,
    Sol,
    Xrp,
}

impl Symbol {
    pub fn as_slug(self) -> &'static str {
        match self {
            Self::Btc => "btc",
            Self::Eth => "eth",
            Self::Sol => "sol",
            Self::Xrp => "xrp",
        }
    }

    pub fn as_chainlink_symbol(self) -> &'static str {
        match self {
            Self::Btc => "btc/usd",
            Self::Eth => "eth/usd",
            Self::Sol => "sol/usd",
            Self::Xrp => "xrp/usd",
        }
    }

    pub fn as_binance_symbol(self) -> &'static str {
        match self {
            Self::Btc => "btcusdt",
            Self::Eth => "ethusdt",
            Self::Sol => "solusdt",
            Self::Xrp => "xrpusdt",
        }
    }
}

impl FromStr for Symbol {
    type Err = PolyfillError;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "btc" => Ok(Self::Btc),
            "eth" => Ok(Self::Eth),
            "sol" => Ok(Self::Sol),
            "xrp" => Ok(Self::Xrp),
            _ => Err(PolyfillError::validation(format!(
                "unsupported symbol: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Interval {
    M5,
    M15,
}

impl Interval {
    pub fn as_slug(self) -> &'static str {
        match self {
            Self::M5 => "5m",
            Self::M15 => "15m",
        }
    }

    pub fn step_secs(self) -> i64 {
        match self {
            Self::M5 => 5 * 60,
            Self::M15 => 15 * 60,
        }
    }
}

impl FromStr for Interval {
    type Err = PolyfillError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "5m" => Ok(Self::M5),
            "15m" => Ok(Self::M15),
            _ => Err(PolyfillError::validation(format!(
                "unsupported interval: {value}"
            ))),
        }
    }
}
