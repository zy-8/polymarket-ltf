use anyhow::Result;
use polymarket_client_sdk::gamma::Client;
use polymarket_client_sdk::gamma::types::request::MarketBySlugRequest;
use polymarket_ltf::polymarket::market_registry::{
    active_markets, current_active_market, next_active_market,
};
use polymarket_ltf::polymarket::utils::crypto_market::{current_slug, next_slug};
use polymarket_ltf::types::crypto::{Interval, Symbol};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    polymarket_ltf::logging::init();

    let mut args = std::env::args().skip(1);
    let symbol = args
        .next()
        .as_deref()
        .unwrap_or("btc")
        .parse::<Symbol>()
        .map_err(anyhow::Error::from)?;
    let interval = args
        .next()
        .as_deref()
        .unwrap_or("15m")
        .parse::<Interval>()
        .map_err(anyhow::Error::from)?;

    let current = current_slug(symbol, interval)?;
    let next = next_slug(symbol, interval)?;
    let client = Client::default();

    info!(%current, active = ?current_active_market(&client, symbol, interval).await?, "Current market");
    fetch_and_print(&client, &current).await?;

    info!(%next, active = ?next_active_market(&client, symbol, interval).await?, "Next market");
    fetch_and_print(&client, &next).await?;

    info!(active = ?active_markets(&client, &[symbol], &[interval]).await?, "Active market token ids");

    Ok(())
}

async fn fetch_and_print(client: &Client, slug: &str) -> Result<()> {
    let market = client
        .market_by_slug(&MarketBySlugRequest::builder().slug(slug).build())
        .await?;

    info!(
        id = market.id,
        question = ?market.question,
        active = ?market.active,
        closed = ?market.closed,
        clob_token_ids = ?market.clob_token_ids,
        "Gamma market"
    );

    Ok(())
}
