use std::env;

use anyhow::{anyhow, Result};

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub discord_token: String,
    pub discord_allowed_user_ids: Vec<u64>,
    pub discord_allowed_role_ids: Vec<u64>,
    pub sqlite_path: String,
    pub codex_bin: String,
    pub codex_model: Option<String>,
    pub worker_concurrency: usize,
    pub notion_token: Option<String>,
    pub notion_task_database_id: Option<String>,
    pub public_base_url: String,
    pub rss_bind_addr: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            discord_token: env::var("DISCORD_TOKEN").unwrap_or_default(),
            discord_allowed_user_ids: parse_u64_list("DISCORD_ALLOWED_USER_IDS")?,
            discord_allowed_role_ids: parse_u64_list("DISCORD_ALLOWED_ROLE_IDS")?,
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
                .unwrap_or_else(|_| "http://localhost:8080".to_string()),
            rss_bind_addr: resolve_rss_bind_addr(),
        })
    }

    pub fn validate_for_bot(&self) -> Result<()> {
        if self.discord_token.is_empty() {
            return Err(anyhow!("DISCORD_TOKEN is required"));
        }

        if self.discord_allowed_user_ids.is_empty() && self.discord_allowed_role_ids.is_empty() {
            return Err(anyhow!(
                "either DISCORD_ALLOWED_USER_IDS or DISCORD_ALLOWED_ROLE_IDS must be set"
            ));
        }

        Ok(())
    }

    pub fn validate_for_rss(&self) -> Result<()> {
        if self.notion_token.is_none() || self.notion_task_database_id.is_none() {
            return Err(anyhow!(
                "NOTION_TOKEN and NOTION_TASK_DATABASE_ID are required for the rss service"
            ));
        }

        Ok(())
    }
}

fn resolve_rss_bind_addr() -> String {
    if let Ok(port) = env::var("PORT") {
        let trimmed = port.trim();
        if !trimmed.is_empty() {
            return format!("0.0.0.0:{}", trimmed);
        }
    }

    env::var("RSS_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string())
}

fn parse_u64_list(var_name: &str) -> Result<Vec<u64>> {
    let raw = match env::var(var_name) {
        Ok(value) => value,
        Err(_) => return Ok(Vec::new()),
    };

    let mut values = Vec::new();
    for part in raw.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }

        values.push(
            trimmed
                .parse::<u64>()
                .map_err(|_| anyhow!("{} contains an invalid u64 value: {}", var_name, trimmed))?,
        );
    }

    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::resolve_rss_bind_addr;
    use std::env;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn port_overrides_rss_bind_addr() {
        let _guard = env_lock().lock().unwrap();
        env::set_var("PORT", "9090");
        env::set_var("RSS_BIND_ADDR", "127.0.0.1:1234");

        assert_eq!(resolve_rss_bind_addr(), "0.0.0.0:9090");

        env::remove_var("PORT");
        env::remove_var("RSS_BIND_ADDR");
    }

    #[test]
    fn falls_back_to_rss_bind_addr() {
        let _guard = env_lock().lock().unwrap();
        env::remove_var("PORT");
        env::set_var("RSS_BIND_ADDR", "127.0.0.1:1234");

        assert_eq!(resolve_rss_bind_addr(), "127.0.0.1:1234");

        env::remove_var("RSS_BIND_ADDR");
    }
}
