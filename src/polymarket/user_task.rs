use std::time::Duration;

use polymarket_client_sdk::data::types::request::PositionsRequest;
use polymarket_client_sdk::data::types::response::Position;
use rust_decimal::Decimal;
use tokio::task::AbortHandle;
use tracing::{info, warn};

use crate::polymarket::relayer::{RelayerAction, RelayerService};
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

    context.store.insert_positions(&positions).await?;

    let outcomes = settled_outcomes_by_slug(&positions);
    if outcomes.is_empty() {
        return Ok(());
    }

    context.store.update_strategy_outcomes(&outcomes).await
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

fn settled_outcomes_by_slug(positions: &[Position]) -> std::collections::HashMap<String, String> {
    positions
        .iter()
        .filter_map(|position| {
            settled_outcome(position).map(|outcome| (position.slug.clone(), outcome))
        })
        .collect()
}

fn settled_outcome(position: &Position) -> Option<String> {
    let held_outcome = match position.outcome.trim().to_ascii_lowercase().as_str() {
        "up" => "up",
        "down" => "down",
        _ => return None,
    };

    let winning = if position.cur_price == Decimal::ONE {
        held_outcome
    } else if position.cur_price == Decimal::ZERO {
        opposite_outcome(held_outcome)
    } else {
        return None;
    };

    Some(winning.to_string())
}

fn opposite_outcome(outcome: &str) -> &'static str {
    match outcome {
        "up" => "down",
        "down" => "up",
        _ => unreachable!("unsupported outcome already filtered"),
    }
}

#[cfg(test)]
mod tests {
    use super::settled_outcome;
    use polymarket_client_sdk::data::types::response::Position;
    use rust_decimal::Decimal;

    fn position(outcome: &str, cur_price: Decimal) -> Position {
        serde_json::from_value(serde_json::json!({
            "proxyWallet": "0x0000000000000000000000000000000000000000",
            "asset": "123",
            "conditionId": format!("0x{}", "0".repeat(64)),
            "size": Decimal::new(8, 0),
            "avgPrice": Decimal::new(50, 2),
            "initialValue": Decimal::new(400, 2),
            "currentValue": Decimal::ONE,
            "cashPnl": Decimal::new(125, 2),
            "percentPnl": Decimal::ZERO,
            "totalBought": Decimal::new(8, 0),
            "realizedPnl": Decimal::new(125, 2),
            "percentRealizedPnl": Decimal::ZERO,
            "curPrice": cur_price,
            "redeemable": true,
            "mergeable": false,
            "title": "ETH test",
            "slug": "eth-updown-15m-test",
            "icon": "",
            "eventSlug": "eth",
            "eventId": "",
            "outcome": outcome,
            "outcomeIndex": 0,
            "oppositeOutcome": "No",
            "oppositeAsset": "456",
            "endDate": "2026-03-25",
            "negativeRisk": false
        }))
        .expect("position should deserialize")
    }

    #[test]
    fn settled_outcome_uses_redeemable_position_terminal_price() {
        assert_eq!(
            settled_outcome(&position("Up", Decimal::ONE)).as_deref(),
            Some("up")
        );
        assert_eq!(
            settled_outcome(&position("Up", Decimal::ZERO)).as_deref(),
            Some("down")
        );
        assert_eq!(
            settled_outcome(&position("Down", Decimal::ONE)).as_deref(),
            Some("down")
        );
        assert_eq!(
            settled_outcome(&position("Down", Decimal::ZERO)).as_deref(),
            Some("up")
        );
    }
}
