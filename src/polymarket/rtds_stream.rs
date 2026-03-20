//! Polymarket RTDS Chainlink 价格流与本地缓存。
//!
//! 这个模块只负责一件事：
//! - 通过官方 Rust SDK 订阅 `subscribe_chainlink_prices`
//! - 在内存里维护各个 symbol 的最新 Chainlink 价格

use crate::errors::{PolyfillError, Result};
use crate::types::crypto::Symbol;
use polymarket_client_sdk::rtds::Client as RtdsClient;
use polymarket_client_sdk::rtds::types::response::ChainlinkPrice;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use tokio::task::AbortHandle;
use tracing::{info, warn};

pub struct Client {
    prices: Arc<RwLock<HashMap<Symbol, ChainlinkPrice>>>,
    tasks: Mutex<Vec<AbortHandle>>,
}

impl Client {
    pub fn connect(symbols: &[Symbol]) -> Result<Self> {
        if symbols.is_empty() {
            return Err(PolyfillError::validation(
                "RTDS Chainlink 订阅至少需要一个 symbol",
            ));
        }

        let client = RtdsClient::default();
        let prices = Arc::new(RwLock::new(HashMap::new()));
        let mut tasks = Vec::with_capacity(symbols.len());

        for symbol in symbols {
            let prices = Arc::clone(&prices);
            let symbol = *symbol;
            let client = client.clone();

            let task = tokio::spawn(async move {
                let stream = match client
                    .subscribe_chainlink_prices(Some(chainlink_symbol(symbol).to_owned()))
                {
                    Ok(stream) => stream,
                    Err(error) => {
                        warn!("创建 RTDS Chainlink 订阅失败: {}", error);
                        return;
                    }
                };
                let mut stream = Box::pin(stream);

                while let Some(message) = futures::StreamExt::next(&mut stream).await {
                    match message {
                        Ok(price) => {
                            if let Ok(mut guard) = prices.write() {
                                guard.insert(symbol, price);
                            } else {
                                warn!("RTDS Chainlink 价格缓存写锁已被污染");
                                break;
                            }
                        }
                        Err(error) => {
                            warn!("RTDS Chainlink 消息处理失败: {}", error);
                        }
                    }
                }

                warn!("RTDS Chainlink 流已结束: symbol={:?}", symbol);
            })
            .abort_handle();

            info!("已启动 RTDS Chainlink 订阅: symbol={:?}", symbol);
            tasks.push(task);
        }

        Ok(Self {
            prices,
            tasks: Mutex::new(tasks),
        })
    }

    pub fn latest(&self, symbol: Symbol) -> Option<ChainlinkPrice> {
        self.prices.read().ok()?.get(&symbol).cloned()
    }

    pub fn snapshot(&self) -> HashMap<Symbol, ChainlinkPrice> {
        self.prices
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn close(&self) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|poisoned| {
            warn!("RTDS Chainlink 任务锁已被污染，继续强制关闭");
            poisoned.into_inner()
        });

        for task in tasks.drain(..) {
            task.abort();
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

fn chainlink_symbol(symbol: Symbol) -> &'static str {
    symbol.as_chainlink_symbol()
}
