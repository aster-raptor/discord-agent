use anyhow::Result;
use discord_agent::config::AppConfig;
use discord_agent::discord_bot;
use discord_agent::logging::init_logging;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AppConfig::from_env()?;
    init_logging(&config.log_file_path)?;
    discord_bot::run(config).await
}
