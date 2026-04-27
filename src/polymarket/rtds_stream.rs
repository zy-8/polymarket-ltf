//! Polymarket RTDS Chainlink 价格流与本地缓存。
//!
//! 单个 task 订阅所有 symbol 的 chainlink 价格，按 `price.symbol` 路由到对应 key。

use crate::errors::{PolyfillError, Result};
use crate::types::crypto::Symbol;
use polymarket_client_sdk_v2::rtds::Client as RtdsClient;
use polymarket_client_sdk_v2::rtds::types::response::ChainlinkPrice;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use tokio::task::AbortHandle;
use tracing::{info, warn};

pub struct Client {
    prices: Arc<RwLock<HashMap<Symbol, ChainlinkPrice>>>,
    task: Mutex<Option<AbortHandle>>,
}

impl Client {
    pub fn connect(symbols: &[Symbol]) -> Result<Self> {
        if symbols.is_empty() {
            return Err(PolyfillError::validation(
                "RTDS Chainlink 订阅至少需要一个 symbol",
            ));
        }

        let routes: HashMap<&'static str, Symbol> = symbols
            .iter()
            .map(|s| (s.as_chainlink_symbol(), *s))
            .collect();
        let prices = Arc::new(RwLock::new(HashMap::new()));
        let prices_task = Arc::clone(&prices);
        let client = RtdsClient::default();

        let task = tokio::spawn(async move {
            let stream = match client.subscribe_chainlink_prices(None) {
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
                        let Some(&symbol) = routes.get(price.symbol.as_str()) else {
                            continue;
                        };
                        match prices_task.write() {
                            Ok(mut guard) => {
                                guard.insert(symbol, price);
                            }
                            Err(_) => {
                                warn!("RTDS Chainlink 价格缓存写锁已被污染");
                                break;
                            }
                        }
                    }
                    Err(error) => warn!("RTDS Chainlink 消息处理失败: {}", error),
                }
            }

            warn!("RTDS Chainlink 流已结束");
        })
        .abort_handle();

        info!("已启动 RTDS Chainlink 订阅: symbols={:?}", symbols);

        Ok(Self {
            prices,
            task: Mutex::new(Some(task)),
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
        let mut guard = self.task.lock().unwrap_or_else(|poisoned| {
            warn!("RTDS Chainlink 任务锁已被污染，继续强制关闭");
            poisoned.into_inner()
        });
        if let Some(task) = guard.take() {
            task.abort();
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}
