//! Rust 侧运行时策略模块。
//!
//! 当前仓库的研究与实时链路已经分层完成，
//! 因此运行时策略代码统一放在 `src/strategy/` 下，
//! 避免再把策略逻辑散落到 `examples/` 或入口文件中。

use alloy_signer::Signer as _;
use alloy_signer_local::PrivateKeySigner;
use anyhow::{Context, Result};
use polymarket_client_sdk::auth::{Credentials, Normal, state::Authenticated};
use polymarket_client_sdk::clob::types::SignatureType;
use polymarket_client_sdk::clob::{Client as ClobClient, Config as ClobConfig};
use polymarket_client_sdk::data::Client as DataClient;
use polymarket_client_sdk::gamma::Client as GammaClient;
use polymarket_client_sdk::types::Address;
use polymarket_client_sdk::{POLYGON, derive_safe_wallet};

use crate::config::AppConfig;
use crate::dashboard::Handle as DashboardHandle;
use crate::polymarket::user_task::user_task;
use crate::storage::sqlite::Store;

pub mod crypto_reversal;

#[derive(Clone)]
pub struct StrategyContext {
    pub signer: PrivateKeySigner,
    pub credentials: Credentials,
    pub clob_client: ClobClient<Authenticated<Normal>>,
    pub data_client: DataClient,
    pub gamma_client: GammaClient,
    pub safe_address: Address,
    pub store: Store,
}

pub async fn run(app: &AppConfig, dashboard: DashboardHandle) -> Result<()> {
    let context = build_context(app).await?;
    let _user_task = user_task(context.clone());

    crypto_reversal::runtime::run(app, dashboard, context).await
}

async fn build_context(app: &AppConfig) -> Result<StrategyContext> {
    let signer = load_signer(&app.trading.private_key)?;
    let store = Store::open(&app.runtime.sqlite_path).await?;
    let clob_client = ClobClient::new(&app.trading.host, ClobConfig::default())?
        .authentication_builder(&signer)
        .signature_type(SignatureType::GnosisSafe)
        .authenticate()
        .await?;
    let credentials = clob_client.create_builder_api_key().await?;
    let safe_address =
        derive_safe_wallet(signer.address(), POLYGON).context("推导 safe 地址失败")?;
    let data_client = DataClient::default();
    let gamma_client = GammaClient::default();

    Ok(StrategyContext {
        signer,
        credentials,
        clob_client,
        data_client,
        gamma_client,
        safe_address,
        store,
    })
}

fn load_signer(private_key: &str) -> Result<PrivateKeySigner> {
    Ok(private_key
        .parse::<PrivateKeySigner>()?
        .with_chain_id(Some(POLYGON)))
}
