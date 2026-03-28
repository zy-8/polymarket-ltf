use std::time::Duration;

use polymarket_client_sdk::data::types::request::PositionsRequest;
use tokio::task::AbortHandle;
use tracing::{info, warn};

use crate::polymarket::relayer::{RelayerAction, RelayerService};
use crate::polymarket::utils::http;
use crate::strategy::StrategyContext;

const AUTO_REDEEM_INTERVAL: Duration = Duration::from_secs(60);
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
];

pub fn user_task(context: StrategyContext) -> AbortHandle {
    tokio::spawn(async move {
        let relayer = RelayerService::new(context.clone());

        loop {
            if let Err(error) = sync_outcomes(&context).await {
                warn!(error = %error, "strategy outcome sync failed");
            }

            if let Err(error) = store_positions(&context).await {
                warn!(
                    error = %error,
                    address = %context.safe_address,
                    "positions sync failed"
                );
            }

            if let Err(error) = auto_redeem(&relayer).await {
                warn!(error = %error, "auto redeem failed");
            }

            tokio::time::sleep(AUTO_REDEEM_INTERVAL).await;
        }
    })
    .abort_handle()
}

async fn sync_outcomes(context: &StrategyContext) -> crate::errors::Result<()> {
    let pending = context.store.select_pending_strategy_outcomes().await?;
    if pending.is_empty() {
        return Ok(());
    }

    let outcomes = retry("sync outcomes", || http::outcomes(&pending)).await?;
    if outcomes.is_empty() {
        return Ok(());
    }

    context.store.update_strategy_outcomes(&outcomes).await
}

async fn store_positions(context: &StrategyContext) -> crate::errors::Result<()> {
    let positions = retry("store positions", || async {
        let request = PositionsRequest::builder()
            .user(context.safe_address)
            .redeemable(true)
            .build();
        context
            .data_client
            .positions(&request)
            .await
            .map_err(|error| {
                crate::errors::PolyfillError::internal_simple(format!(
                    "查询 redeemable positions 失败 user={}: {error}",
                    context.safe_address
                ))
            })
    })
    .await?;

    context.store.insert_positions(&positions).await
}

async fn auto_redeem(relayer: &RelayerService) -> crate::errors::Result<()> {
    let tx = retry("auto redeem", || async {
        relayer
            .run(RelayerAction::Redeem)
            .await
            .map_err(|error| crate::errors::PolyfillError::internal_simple(format!("{error}")))
    })
    .await?;

    if tx.transaction_id == "noop-no-redeemable-positions" {
        Ok(())
    } else {
        info!(
            transaction_id = %tx.transaction_id,
            state = ?tx.state,
            transaction_hash = ?tx.transaction_hash,
            "auto redeem submitted"
        );
        Ok(())
    }
}

async fn retry<F, Fut, T>(name: &str, mut op: F) -> crate::errors::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = crate::errors::Result<T>>,
{
    let mut attempt = 0;

    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(_error) if attempt < RETRY_DELAYS.len() => {
                let delay = RETRY_DELAYS[attempt];
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            Err(error) => {
                return Err(crate::errors::PolyfillError::internal_simple(format!(
                    "{name} failed after {} retries: {error}",
                    RETRY_DELAYS.len()
                )));
            }
        }
    }
}
