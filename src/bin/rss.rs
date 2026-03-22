use anyhow::Result;
use discord_agent::config::AppConfig;
use discord_agent::rss_server;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = AppConfig::from_env()?;
    rss_server::run(config).await
}
