//! Relayer 交易服务（程序化封装）。
//!
//! 参考 Polymarket Relayer 接口约定，按 `commands` 风格对外提供：
//! - 交易编码（split/redeem/transfer/multisend）；
//! - SAFE EIP-712 签名；
//! - Relayer 提交、查询与确认轮询。

use crate::commands::data::DataService;
use alloy::primitives::{address, keccak256, Address, Bytes, FixedBytes, U256};
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolCall, SolStruct};
use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use hmac::{Hmac, Mac};
use polymarket_client_sdk::auth::Credentials;
use polymarket_client_sdk::auth::ExposeSecret as _;
use polymarket_client_sdk::{contract_config, derive_safe_wallet, POLYGON};
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

const DEFAULT_RELAYER_URL: &str = "https://relayer-v2.polymarket.com";
const DEFAULT_CHAIN_ID: u64 = POLYGON;
const SAFE_TX_TYPE: &str = "SAFE";

/// Polymarket MultiSend 合约地址（Polygon）。
const MULTISEND_ADDRESS: Address = address!("40A2aCCbd92BCA938b02010E17A5b8929b49130D");

mod endpoints {
    pub const RELAY_PAYLOAD: &str = "/nonce";
    pub const SUBMIT: &str = "/submit";
    pub const TRANSACTION: &str = "/transaction";
}

sol! {
    #[derive(Debug)]
    struct SafeTx {
        address to;
        uint256 value;
        bytes data;
        uint8 operation;
        uint256 safeTxGas;
        uint256 baseGas;
        uint256 gasPrice;
        address gasToken;
        address refundReceiver;
        uint256 nonce;
    }

    interface IConditionalTokens {
        function splitPosition(
            address collateralToken,
            bytes32 parentCollectionId,
            bytes32 conditionId,
            uint256[] calldata partition,
            uint256 amount
        ) external;

        function mergePositions(
            address collateralToken,
            bytes32 parentCollectionId,
            bytes32 conditionId,
            uint256[] calldata partition,
            uint256 amount
        ) external;

        function redeemPositions(
            address collateralToken,
            bytes32 parentCollectionId,
            bytes32 conditionId,
            uint256[] calldata indexSets
        ) external;
    }

    interface IERC20 {
        function transfer(address to, uint256 amount) external returns (bool);
    }

    interface IMultiSend {
        function multiSend(bytes memory transactions) external payable;
    }
}

/// Relayer 交易状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
pub enum RelayerTransactionState {
    #[serde(rename = "STATE_NEW")]
    New,
    #[serde(rename = "STATE_EXECUTED")]
    Executed,
    #[serde(rename = "STATE_MINED")]
    Mined,
    #[serde(rename = "STATE_INVALID")]
    Invalid,
    #[serde(rename = "STATE_CONFIRMED")]
    Confirmed,
    #[serde(rename = "STATE_FAILED")]
    Failed,
}

impl RelayerTransactionState {
    /// 是否已到终态（确认成功 / 失败 / 无效）。
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Confirmed | Self::Failed | Self::Invalid)
    }
}

/// 通用交易对象（给 Relayer 批量执行）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelayerCall {
    pub to: Address,
    pub data: Bytes,
    #[serde(with = "u256_string")]
    pub value: U256,
}

impl RelayerCall {
    /// 创建 value=0 的交易。
    fn new(to: Address, data: impl Into<Bytes>) -> Self {
        Self {
            to,
            data: data.into(),
            value: U256::ZERO,
        }
    }
}

/// 单一入口动作定义。
#[derive(Clone, Debug)]
pub enum RelayerAction {
    Split {
        condition_id: FixedBytes<32>,
        amount: U256,
    },
    Merge {
        condition_id: FixedBytes<32>,
        amount: U256,
    },
    Redeem,
    RedeemCondition {
        condition_id: FixedBytes<32>,
    },
    Transfer {
        to: Address,
        amount: U256,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayerTransaction {
    #[serde(alias = "transactionID")]
    pub transaction_id: String,
    #[serde(default, alias = "transactionHash")]
    pub transaction_hash: Option<String>,
    pub state: RelayerTransactionState,
}

#[derive(Clone, Debug)]
pub struct RelayerConfig {
    /// Relayer API Base URL。
    pub url: String,
    /// 链 ID（默认 Polygon=137）。
    pub chain_id: u64,
}

impl Default for RelayerConfig {
    fn default() -> Self {
        Self {
            url: DEFAULT_RELAYER_URL.to_string(),
            chain_id: DEFAULT_CHAIN_ID,
        }
    }
}

#[derive(Clone, Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct SignatureParams {
    gas_price: String,
    operation: String,
    safe_txn_gas: String,
    base_gas: String,
    gas_token: Address,
    refund_receiver: Address,
}

impl SignatureParams {
    fn for_operation(operation: u8) -> Self {
        Self {
            gas_price: "0".to_string(),
            operation: operation.to_string(),
            safe_txn_gas: "0".to_string(),
            base_gas: "0".to_string(),
            gas_token: Address::ZERO,
            refund_receiver: Address::ZERO,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SafeSubmitRequest {
    from: Address,
    to: Address,
    proxy_wallet: Address,
    data: Bytes,
    nonce: String,
    signature: Bytes,
    signature_params: SignatureParams,
    #[serde(rename = "type")]
    tx_type: String,
    metadata: String,
}

#[derive(Clone, Debug, Deserialize)]
struct RelayPayloadResponse {
    nonce: String,
}

/// Relayer 业务服务。
///
/// 该服务保持：
/// - Builder API 凭据（用于签名头）；
/// - 钱包签名器（用于 SAFE Tx 签名）；
/// - HTTP 客户端与轮询配置。
pub struct RelayerService<S: alloy::signers::Signer + Send + Sync> {
    config: RelayerConfig,
    credentials: Credentials,
    http: reqwest::Client,
    signer_address: Address,
    signer: S,
}

impl<S: alloy::signers::Signer + Send + Sync> RelayerService<S> {
    /// 使用默认配置创建服务。
    pub fn new(credentials: Credentials, signer: S) -> Self {
        let signer_address = signer.address();
        Self {
            config: RelayerConfig::default(),
            credentials,
            http: reqwest::Client::new(),
            signer_address,
            signer,
        }
    }

    fn build_url(&self, path: &str) -> String {
        format!("{}{}", self.config.url.as_str().trim_end_matches('/'), path)
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.build_url(path);
        let headers = create_relayer_headers(&self.credentials, "GET", path, "")?;
        let response = self.http.get(url).headers(headers).send().await?;
        self.handle_response(response).await
    }

    async fn post<T, B>(&self, path: &str, body: &B) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
        B: Serialize,
    {
        let url = self.build_url(path);
        let body_json = serde_json::to_string(body)?;
        let headers = create_relayer_headers(&self.credentials, "POST", path, &body_json)?;
        let response = self
            .http
            .post(url)
            .headers(headers)
            .header("Content-Type", "application/json")
            .body(body_json)
            .send()
            .await?;
        self.handle_response(response).await
    }

    async fn handle_response<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T> {
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("Relayer API 错误（{}）：{}", status.as_u16(), text);
        }
        Ok(serde_json::from_str(&text)?)
    }

    async fn relay_payload(&self) -> Result<RelayPayloadResponse> {
        let path = format!(
            "{}?address={}&type=SAFE",
            endpoints::RELAY_PAYLOAD,
            self.signer_address
        );
        self.get(&path).await
    }

    /// 提交单笔或批量交易到 Relayer。
    ///
    /// - `calls.len() == 1` 时直接提交；
    /// - `calls.len() > 1` 时使用 MultiSend 打包后提交。
    async fn submit(
        &self,
        calls: Vec<RelayerCall>,
        metadata: impl Into<String>,
    ) -> Result<RelayerTransaction> {
        anyhow::ensure!(!calls.is_empty(), "calls 不能为空");
        let safe = derive_safe_wallet(self.signer_address, self.config.chain_id)
            .context("无法推导 SAFE 地址，请检查 chain_id 与 signer 地址")?;
        let payload = self.relay_payload().await?;
        let nonce: u64 = payload
            .nonce
            .parse()
            .with_context(|| format!("无效 nonce：{}", payload.nonce))?;

        let (to, data, operation) = if calls.len() == 1 {
            let call = &calls[0];
            (call.to, call.data.clone(), 0u8)
        } else {
            (MULTISEND_ADDRESS, encode_multisend(&calls)?, 1u8)
        };

        let signature = sign_safe_tx(
            &self.signer,
            &SafeTxParams {
                safe,
                to,
                value: U256::ZERO,
                data: data.clone(),
                operation,
                nonce: U256::from(nonce),
                chain_id: self.config.chain_id,
                ..Default::default()
            },
        )
        .await?;

        let request = SafeSubmitRequest {
            from: self.signer_address,
            to,
            proxy_wallet: safe,
            data,
            nonce: nonce.to_string(),
            signature,
            signature_params: SignatureParams::for_operation(operation),
            tx_type: SAFE_TX_TYPE.to_string(),
            metadata: metadata.into(),
        };

        self.post(endpoints::SUBMIT, &request).await
    }

    /// 最简入口：按动作执行 Relayer 交易。
    pub async fn run(&self, action: RelayerAction) -> Result<RelayerTransaction> {
        match action {
            RelayerAction::Split {
                condition_id,
                amount,
            } => {
                let call = split_position(condition_id, amount)?;
                let tx = self.submit(vec![call], "split").await?;
                self.confirmation(&tx.transaction_id, None, None).await
            }
            RelayerAction::Merge {
                condition_id,
                amount,
            } => {
                let call = merge_position(condition_id, amount)?;
                let tx = self.submit(vec![call], "merge").await?;
                self.confirmation(&tx.transaction_id, None, None).await
            }
            RelayerAction::Redeem => {
                let condition_ids = self.collect_redeemable_condition_ids().await?;
                if condition_ids.is_empty() {
                    return Ok(RelayerTransaction {
                        transaction_id: "noop-no-redeemable-positions".to_string(),
                        transaction_hash: None,
                        state: RelayerTransactionState::Confirmed,
                    });
                }
                let mut calls = Vec::with_capacity(condition_ids.len());
                for cid in condition_ids {
                    calls.push(redeem_position(cid)?);
                }
                let tx = self.submit(calls, "redeem").await?;
                self.confirmation(&tx.transaction_id, None, None).await
            }
            RelayerAction::RedeemCondition { condition_id } => {
                let call = redeem_position(condition_id)?;
                let tx = self.submit(vec![call], "redeem").await?;
                self.confirmation(&tx.transaction_id, None, None).await
            }
            RelayerAction::Transfer { to, amount } => {
                let call = transfer_position(to, amount)?;
                let tx = self.submit(vec![call], "transfer").await?;
                self.confirmation(&tx.transaction_id, None, None).await
            }
        }
    }

    /// 按交易 ID 查询 Relayer 交易状态。
    pub async fn get_transaction(&self, tx_id: &str) -> Result<RelayerTransaction> {
        let path = format!("{}?id={}", endpoints::TRANSACTION, tx_id);
        let transactions: Vec<RelayerTransaction> = self.get(&path).await?;
        transactions
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Transaction not found"))
    }

    /// 轮询交易状态直到终态（CONFIRMED / FAILED / INVALID）。
    ///
    /// - `max_attempts` 为空时默认 `60` 次；
    /// - `interval_secs` 为空时默认 `2` 秒。
    pub async fn confirmation(
        &self,
        tx_id: &str,
        max_attempts: Option<u32>,
        interval_secs: Option<u64>,
    ) -> Result<RelayerTransaction> {
        let max_attempts = max_attempts.unwrap_or(60);
        let interval_secs = interval_secs.unwrap_or(2);

        let mut last: Option<RelayerTransaction> = None;
        for _ in 0..max_attempts {
            let tx = self.get_transaction(tx_id).await?;
            if tx.state.is_terminal() {
                return Ok(tx);
            }
            last = Some(tx);
            tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
        }

        if let Some(tx) = last {
            anyhow::bail!(
                "确认超时: tx_id={}, state={:?}, attempts={}",
                tx.transaction_id,
                tx.state,
                max_attempts
            );
        }
        anyhow::bail!("确认超时: tx_id={tx_id}, attempts={max_attempts}");
    }

    async fn collect_redeemable_condition_ids(&self) -> Result<Vec<FixedBytes<32>>> {
        let safe = derive_safe_wallet(self.signer_address, self.config.chain_id)
            .context("无法推导 SAFE 地址，请检查 chain_id 与 signer 地址")?;
        let address = safe.to_string();
        let data = DataService::default();

        let mut offset = 0;
        let limit = 100;
        let mut condition_ids: std::collections::BTreeSet<FixedBytes<32>> =
            std::collections::BTreeSet::new();

        loop {
            let positions = data
                .redeem_positions(&address, Some(limit), Some(offset))
                .await?;
            let batch_len = positions.len();

            for p in positions {
                condition_ids.insert(p.condition_id);
            }

            if batch_len < limit as usize {
                break;
            }
            offset += limit;
        }

        Ok(condition_ids.into_iter().collect())
    }
}

/// SAFE EIP-712 签名参数。
#[derive(Clone, Debug, Default)]
struct SafeTxParams {
    safe: Address,
    to: Address,
    value: U256,
    data: Bytes,
    operation: u8,
    safe_tx_gas: U256,
    base_gas: U256,
    gas_price: U256,
    gas_token: Address,
    refund_receiver: Address,
    nonce: U256,
    chain_id: u64,
}

fn safe_tx_hash(params: &SafeTxParams) -> alloy::primitives::B256 {
    let domain = eip712_domain! {
        chain_id: params.chain_id,
        verifying_contract: params.safe,
    };

    let safe_tx = SafeTx {
        to: params.to,
        value: params.value,
        data: params.data.to_vec().into(),
        operation: params.operation,
        safeTxGas: params.safe_tx_gas,
        baseGas: params.base_gas,
        gasPrice: params.gas_price,
        gasToken: params.gas_token,
        refundReceiver: params.refund_receiver,
        nonce: params.nonce,
    };

    safe_tx.eip712_signing_hash(&domain)
}

async fn sign_safe_tx<S: alloy::signers::Signer>(
    signer: &S,
    params: &SafeTxParams,
) -> Result<Bytes> {
    let tx_hash = safe_tx_hash(params);
    let eth_signed_hash = keccak256(
        [
            b"\x19Ethereum Signed Message:\n32".as_slice(),
            tx_hash.as_slice(),
        ]
        .concat(),
    );

    let signature = signer.sign_hash(&eth_signed_hash).await?;
    let mut sig = signature.as_bytes().to_vec();
    if sig.len() == 65 {
        sig[64] += 4;
    }
    Ok(Bytes::from(sig))
}

fn create_relayer_headers(
    credentials: &Credentials,
    method: &str,
    path: &str,
    body: &str,
) -> Result<HeaderMap> {
    let timestamp = chrono::Utc::now().timestamp();
    let sign_path = path.split('?').next().unwrap_or(path);
    let message = format!("{timestamp}{method}{sign_path}{body}");

    let decoded_secret = URL_SAFE
        .decode(credentials.secret().expose_secret())
        .context("Builder secret 不是合法的 URL-safe base64")?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&decoded_secret)
        .context("无法初始化 HMAC（Builder secret 无效）")?;
    mac.update(message.as_bytes());
    let signature = URL_SAFE.encode(mac.finalize().into_bytes());

    let mut headers = HeaderMap::new();
    headers.insert(
        "POLY_BUILDER_API_KEY",
        credentials
            .key()
            .to_string()
            .parse()
            .context("无效 API key header")?,
    );
    headers.insert(
        "POLY_BUILDER_TIMESTAMP",
        timestamp
            .to_string()
            .parse()
            .context("无效 timestamp header")?,
    );
    headers.insert(
        "POLY_BUILDER_PASSPHRASE",
        credentials
            .passphrase()
            .expose_secret()
            .parse()
            .context("无效 passphrase header")?,
    );
    headers.insert(
        "POLY_BUILDER_SIGNATURE",
        signature.parse().context("无效 signature header")?,
    );

    Ok(headers)
}

fn polygon_contracts() -> Result<(Address, Address, Address)> {
    let cfg = contract_config(POLYGON, false).context("读取 Polygon 合约配置失败")?;
    Ok((cfg.collateral, cfg.conditional_tokens, cfg.exchange))
}

/// 编码 splitPosition 调用（USDC -> 条件 token）。
fn split_position(condition_id: FixedBytes<32>, amount: U256) -> Result<RelayerCall> {
    let (usdc, ctf, _) = polygon_contracts()?;
    let call = IConditionalTokens::splitPositionCall {
        collateralToken: usdc,
        parentCollectionId: FixedBytes::<32>::ZERO,
        conditionId: condition_id,
        partition: vec![U256::from(1), U256::from(2)],
        amount,
    };
    Ok(RelayerCall::new(ctf, Bytes::from(call.abi_encode())))
}

/// 编码 mergePositions 调用（条件 token -> USDC 准备态）。
fn merge_position(condition_id: FixedBytes<32>, amount: U256) -> Result<RelayerCall> {
    let (usdc, ctf, _) = polygon_contracts()?;
    let call = IConditionalTokens::mergePositionsCall {
        collateralToken: usdc,
        parentCollectionId: FixedBytes::<32>::ZERO,
        conditionId: condition_id,
        partition: vec![U256::from(1), U256::from(2)],
        amount,
    };
    Ok(RelayerCall::new(ctf, Bytes::from(call.abi_encode())))
}

/// 编码 redeemPositions 调用（条件 token -> USDC）。
fn redeem_position(condition_id: FixedBytes<32>) -> Result<RelayerCall> {
    let (usdc, ctf, _) = polygon_contracts()?;
    let call = IConditionalTokens::redeemPositionsCall {
        collateralToken: usdc,
        parentCollectionId: FixedBytes::<32>::ZERO,
        conditionId: condition_id,
        indexSets: vec![U256::from(1), U256::from(2)],
    };
    Ok(RelayerCall::new(ctf, Bytes::from(call.abi_encode())))
}

/// 编码 USDC 转账调用。
fn transfer_position(to: Address, amount: U256) -> Result<RelayerCall> {
    let (usdc, _, _) = polygon_contracts()?;
    let call = IERC20::transferCall { to, amount };
    Ok(RelayerCall::new(usdc, Bytes::from(call.abi_encode())))
}

/// 将多笔调用打包为 MultiSend payload。
fn encode_multisend(calls: &[RelayerCall]) -> Result<Bytes> {
    anyhow::ensure!(!calls.is_empty(), "calls 不能为空");

    let mut packed = Vec::new();
    for call in calls {
        packed.push(0u8);
        packed.extend_from_slice(call.to.as_slice());
        packed.extend_from_slice(&call.value.to_be_bytes::<32>());
        packed.extend_from_slice(&U256::from(call.data.len()).to_be_bytes::<32>());
        packed.extend_from_slice(&call.data);
    }

    let multisend = IMultiSend::multiSendCall {
        transactions: Bytes::from(packed),
    };
    Ok(Bytes::from(multisend.abi_encode()))
}

mod u256_string {
    use alloy::primitives::U256;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &U256, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<U256, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        U256::from_str_radix(&s, 10).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_terminal_detection() {
        assert!(RelayerTransactionState::Confirmed.is_terminal());
        assert!(RelayerTransactionState::Failed.is_terminal());
        assert!(RelayerTransactionState::Invalid.is_terminal());
        assert!(!RelayerTransactionState::New.is_terminal());
    }

    #[test]
    fn encode_multisend_empty_rejected() {
        assert!(encode_multisend(&[]).is_err());
    }
}
