use std::collections::HashMap;

use reqwest::Client;
use serde::Deserialize;

use crate::errors::{PolyfillError, Result};

const DEFAULT_BASE_URL: &str = "https://polymarket.com";

#[derive(Debug, Deserialize)]
struct PastResultsEnvelope {
    status: String,
    data: PastResultsData,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PastResultsData {
    #[serde(default)]
    outcomes_by_slug: HashMap<String, String>,
}

pub async fn outcomes(slugs: &[String]) -> Result<HashMap<String, String>> {
    outcomes_at(DEFAULT_BASE_URL, slugs).await
}

pub async fn outcomes_at(base_url: &str, slugs: &[String]) -> Result<HashMap<String, String>> {
    if slugs.is_empty() {
        return Ok(HashMap::new());
    }

    let response = Client::new()
        .post(format!(
            "{}/api/past-results",
            base_url.trim_end_matches('/')
        ))
        .json(&serde_json::json!({
            "includeOutcomesBySlug": true,
            "outcomesOnly": true,
            "pastEventSlugs": slugs,
        }))
        .send()
        .await
        .map_err(|error| {
            PolyfillError::internal_simple(format!("请求 polymarket past-results 失败: {error}"))
        })?;

    let response = response.error_for_status().map_err(|error| {
        PolyfillError::internal_simple(format!("polymarket past-results 返回失败状态: {error}"))
    })?;

    let payload: PastResultsEnvelope = response.json().await.map_err(|error| {
        PolyfillError::parse(
            format!("解析 polymarket past-results 响应失败: {error}"),
            Some(Box::new(error)),
        )
    })?;

    if payload.status != "success" {
        return Err(PolyfillError::internal_simple(format!(
            "polymarket past-results 返回非 success 状态: {}",
            payload.status
        )));
    }

    Ok(payload.data.outcomes_by_slug)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::PastResultsEnvelope;

    #[test]
    fn request_body_matches_expected_payload() {
        let slugs = ["eth-updown-5m-1", "eth-updown-5m-2"];
        let value = serde_json::json!({
            "includeOutcomesBySlug": true,
            "outcomesOnly": true,
            "pastEventSlugs": slugs,
        });

        assert_eq!(
            value,
            serde_json::json!({
                "includeOutcomesBySlug": true,
                "outcomesOnly": true,
                "pastEventSlugs": ["eth-updown-5m-1", "eth-updown-5m-2"]
            })
        );
    }

    #[test]
    fn response_deserializes_outcomes_by_slug() {
        let payload: PastResultsEnvelope = serde_json::from_value(serde_json::json!({
            "status": "success",
            "data": {
                "results": [],
                "outcomesBySlug": {
                    "eth-updown-5m-1": "up",
                    "eth-updown-5m-2": "down"
                }
            }
        }))
        .expect("response should deserialize");

        let expected = HashMap::from([
            ("eth-updown-5m-1".to_string(), "up".to_string()),
            ("eth-updown-5m-2".to_string(), "down".to_string()),
        ]);
        assert_eq!(payload.data.outcomes_by_slug, expected);
    }
}
