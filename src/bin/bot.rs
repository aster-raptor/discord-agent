use anyhow::Result;
use discord_agent::config::AppConfig;
use discord_agent::discord_bot;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = AppConfig::from_env()?;
    discord_bot::run(config).await
}
