use std::collections::HashMap;
use std::time::Duration;

use polymarket_client_sdk::data::types::request::PositionsRequest;
use polymarket_client_sdk::gamma::types::request::MarketBySlugRequest;
use polymarket_client_sdk::gamma::types::response::Market;
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

            if let Err(error) = backfill_strategy_outcomes(&context).await {
                warn!(error = %error, "strategy outcome backfill failed");
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

    context.store.insert_positions(&positions).await
}

/// 通过 Gamma API 查询已结算 market，回填 strategy 表中缺失的 outcome。
async fn backfill_strategy_outcomes(context: &StrategyContext) -> crate::errors::Result<()> {
    let slugs = context.store.pending_outcome_slugs().await?;
    if slugs.is_empty() {
        return Ok(());
    }

    let mut outcomes: HashMap<String, String> = HashMap::new();
    for slug in &slugs {
        let market = context
            .gamma_client
            .market_by_slug(&MarketBySlugRequest::builder().slug(slug).build())
            .await;

        match market {
            Ok(market) => {
                if let Some(outcome) = settled_outcome_from_market(&market) {
                    outcomes.insert(slug.clone(), outcome);
                }
            }
            Err(error) => {
                warn!(slug, %error, "查询 market 结算状态失败");
            }
        }
    }

    if outcomes.is_empty() {
        return Ok(());
    }

    info!(count = outcomes.len(), "backfill strategy outcomes from gamma");
    context.store.update_strategy_outcomes(&outcomes).await
}

/// 从 Gamma Market 的 outcomes + outcome_prices 判断赢家。
fn settled_outcome_from_market(market: &Market) -> Option<String> {
    if market.closed != Some(true) {
        return None;
    }

    let outcomes = market.outcomes.as_ref()?;
    let prices = market.outcome_prices.as_ref()?;

    outcomes
        .iter()
        .zip(prices.iter())
        .find(|(_, price)| **price == Decimal::ONE)
        .and_then(|(name, _)| match name.trim().to_ascii_lowercase().as_str() {
            "up" => Some("up".to_string()),
            "down" => Some("down".to_string()),
            _ => None,
        })
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

#[cfg(test)]
mod tests {
    use super::settled_outcome_from_market;
    use polymarket_client_sdk::gamma::types::response::Market;
    use rust_decimal::Decimal;

    fn market(outcomes: Vec<&str>, prices: Vec<Decimal>, closed: bool) -> Market {
        serde_json::from_value(serde_json::json!({
            "id": "test-market",
            "closed": closed,
            "outcomes": serde_json::to_string(&outcomes).unwrap(),
            "outcomePrices": serde_json::to_string(&prices).unwrap(),
        }))
        .expect("market should deserialize")
    }

    #[test]
    fn settled_outcome_from_closed_market() {
        let m = market(vec!["Up", "Down"], vec![Decimal::ONE, Decimal::ZERO], true);
        assert_eq!(settled_outcome_from_market(&m).as_deref(), Some("up"));

        let m = market(vec!["Up", "Down"], vec![Decimal::ZERO, Decimal::ONE], true);
        assert_eq!(settled_outcome_from_market(&m).as_deref(), Some("down"));
    }

    #[test]
    fn returns_none_for_open_market() {
        let m = market(
            vec!["Up", "Down"],
            vec![Decimal::new(55, 2), Decimal::new(45, 2)],
            false,
        );
        assert_eq!(settled_outcome_from_market(&m), None);
    }

    #[test]
    fn returns_none_when_not_yet_settled() {
        let m = market(
            vec!["Up", "Down"],
            vec![Decimal::new(55, 2), Decimal::new(45, 2)],
            true,
        );
        assert_eq!(settled_outcome_from_market(&m), None);
    }
}
