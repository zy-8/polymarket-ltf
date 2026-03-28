#[tokio::main]
async fn main() -> anyhow::Result<()> {
    polymarket_ltf::init_process()?;
    let dashboard = polymarket_ltf::dashboard::start().await?;
    dashboard.runtime_status("starting");

    match polymarket_ltf::config::AppConfig::load().map_err(anyhow::Error::from) {
        Ok(config) => {
            dashboard.runtime_status("running");
            dashboard.register_strategy(
                polymarket_ltf::strategy::crypto_reversal::constants::STRATEGY_NAME,
            );

            let dashboard_task = dashboard.clone();
            tokio::spawn(async move {
                if let Err(error) =
                    polymarket_ltf::strategy::run(&config, dashboard_task.clone()).await
                {
                    dashboard_task.error(None, format!("strategy runtime stopped: {error}"));
                }
            });
        }
        Err(error) => {
            dashboard.error(None, format!("config load failed: {error}"));
        }
    }

    futures::future::pending::<()>().await;
    Ok(())
}
