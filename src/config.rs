use std::env;

use anyhow::{anyhow, Result};

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub discord_token: String,
    pub sqlite_path: String,
    pub codex_bin: String,
    pub codex_model: Option<String>,
    pub worker_concurrency: usize,
    pub notion_token: Option<String>,
    pub notion_task_database_id: Option<String>,
    pub public_base_url: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            discord_token: env::var("DISCORD_TOKEN").unwrap_or_default(),
            sqlite_path: env::var("SQLITE_PATH")
                .unwrap_or_else(|_| "data/discord-agent.sqlite3".to_string()),
            codex_bin: env::var("CODEX_BIN").unwrap_or_else(|_| "codex".to_string()),
            codex_model: env::var("CODEX_MODEL")
                .ok()
                .filter(|value| !value.is_empty()),
            worker_concurrency: env::var("WORKER_CONCURRENCY")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(1)
                .max(1),
            notion_token: env::var("NOTION_TOKEN")
                .ok()
                .filter(|value| !value.is_empty()),
            notion_task_database_id: env::var("NOTION_TASK_DATABASE_ID")
                .ok()
                .filter(|value| !value.is_empty()),
            public_base_url: env::var("PUBLIC_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:3000".to_string()),
        })
    }

    pub fn validate_for_bot(&self) -> Result<()> {
        if self.discord_token.is_empty() {
            return Err(anyhow!("DISCORD_TOKEN is required"));
        }

        Ok(())
    }
}
