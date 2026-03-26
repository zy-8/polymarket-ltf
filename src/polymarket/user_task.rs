use std::sync::{Arc, RwLock};
use std::time::Duration;

use polymarket_client_sdk::data::Client as DataClient;
use polymarket_client_sdk::data::types::response::ClosedPosition;
use polymarket_client_sdk::data::types::{
    ClosedPositionSortBy, SortDirection, request::ClosedPositionsRequest,
};
use polymarket_client_sdk::types::Address;
use tokio::task::AbortHandle;
use tracing::{info, warn};

use crate::errors::PolyfillError;
use crate::polymarket::relayer::{RelayerAction, RelayerService};
use crate::strategy::StrategyContext;

const AUTO_REDEEM_INTERVAL: Duration = Duration::from_secs(60);
const CLOSED_POSITIONS_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Default)]
pub struct ClosedPositionsCache {
    inner: Arc<RwLock<Vec<ClosedPosition>>>,
}

impl ClosedPositionsCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace(&self, positions: Vec<ClosedPosition>) {
        if let Ok(mut guard) = self.inner.write() {
            *guard = positions;
        }
    }

    pub fn snapshot(&self) -> Vec<ClosedPosition> {
        self.inner
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

pub fn auto_redeem_task(context: StrategyContext) -> AbortHandle {
    tokio::spawn(async move {
        let relayer = RelayerService::new(context);

        loop {
            match relayer.run(RelayerAction::Redeem).await {
                Ok(tx) if tx.transaction_id == "noop-no-redeemable-positions" => {}
                Ok(tx) => {
                    info!(
                        transaction_id = %tx.transaction_id,
                        state = ?tx.state,
                        transaction_hash = ?tx.transaction_hash,
                        "auto redeem submitted"
                    );
                }
                Err(error) => {
                    warn!(error = %error, "auto redeem failed");
                }
            }

            tokio::time::sleep(AUTO_REDEEM_INTERVAL).await;
        }
    })
    .abort_handle()
}

pub fn closed_positions_cache_task(
    context: StrategyContext,
    cache: ClosedPositionsCache,
) -> AbortHandle {
    tokio::spawn(async move {
        loop {
            match fetch_all_closed_positions(&context.data_client, context.safe_address).await {
                Ok(positions) => cache.replace(positions),
                Err(error) => {
                    warn!(
                        error = %error,
                        address = %context.safe_address,
                        "closed_positions cache refresh failed"
                    );
                }
            }

            tokio::time::sleep(CLOSED_POSITIONS_REFRESH_INTERVAL).await;
        }
    })
    .abort_handle()
}

async fn fetch_all_closed_positions(
    client: &DataClient,
    user: Address,
) -> crate::errors::Result<Vec<ClosedPosition>> {
    let request = ClosedPositionsRequest::builder()
        .user(user)
        .limit(50)
        .map_err(|error| {
            PolyfillError::validation(format!("closed_positions limit 非法: {error}"))
        })?
        .sort_by(ClosedPositionSortBy::Timestamp)
        .sort_direction(SortDirection::Desc)
        .build();

    client.closed_positions(&request).await.map_err(|error| {
        PolyfillError::internal_simple(format!("查询 closed_positions 失败 user={user}: {error}"))
    })
}
