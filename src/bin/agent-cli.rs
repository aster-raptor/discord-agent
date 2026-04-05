use std::env;
use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use discord_agent::codex::CodexRunner;
use discord_agent::config::AppConfig;
use discord_agent::db::Database;
use discord_agent::local_input::load_from_path;
use discord_agent::logging::init_logging;
use discord_agent::models::{TaskRecord, TaskType};
use discord_agent::notion::NotionClient;
use discord_agent::task_processor::process_task;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AppConfig::from_env()?;
    init_logging(&config.log_file_path)?;

    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(anyhow!("usage: agent-cli <submit|status|result|worker> ..."));
    };

    match command.as_str() {
        "submit" => {
            let prompt = read_flag(&mut args, "--prompt")?;
            let path = read_flag(&mut args, "--path")?;
            let prompt_template = read_optional_flag(&mut args, "--prompt-template");
            submit_task(config, &prompt, &path, prompt_template.as_deref()).await
        }
        "status" => {
            let task_id = read_flag(&mut args, "--task-id")?;
            print_status(config, &task_id)
        }
        "result" => {
            let task_id = read_flag(&mut args, "--task-id")?;
            print_result(config, &task_id)
        }
        "worker" => {
            let task_id = read_flag(&mut args, "--task-id")?;
            let prompt_template = read_optional_flag(&mut args, "--prompt-template");
            let mut config = config;
            if let Some(path) = prompt_template {
                config.research_prompt_path = path;
            }
            run_worker(config, &task_id).await
        }
        other => Err(anyhow!("unknown command: {}", other)),
    }
}

async fn submit_task(
    config: AppConfig,
    prompt: &str,
    path: &str,
    prompt_template: Option<&str>,
) -> Result<()> {
    let database = Arc::new(Database::open(&config.sqlite_path)?);
    let loaded_input = load_from_path(path)?;

    let mut task = TaskRecord::new(
        0,
        0,
        0,
        build_title(prompt),
        prompt.to_string(),
        TaskType::Research,
    );
    task.input_source_path = Some(loaded_input.source_path.clone());
    task.input_payload = Some(loaded_input.payload.clone());

    database.insert_task(&task)?;
    database.update_status(&task.id, discord_agent::models::TaskStatus::Queued, Some("task queued"))?;
    spawn_worker_process(&config, &task.id, prompt_template)?;

    println!("{}", json!({ "task_id": task.id }));
    Ok(())
}

fn print_status(config: AppConfig, task_id: &str) -> Result<()> {
    let database = Database::open(&config.sqlite_path)?;
    let task = database.get_task(task_id)?;
    println!(
        "{}",
        json!({
            "task_id": task.id,
            "status": task.status.as_str(),
            "title": task.title
        })
    );
    Ok(())
}

fn print_result(config: AppConfig, task_id: &str) -> Result<()> {
    let database = Database::open(&config.sqlite_path)?;
    let task = database.get_task(task_id)?;
    println!(
        "{}",
        json!({
            "task_id": task.id,
            "status": task.status.as_str(),
            "title": task.title,
            "public_summary": task.public_summary,
            "raw_output": task.raw_output,
            "error_text": task.error_text,
            "notion_page_id": task.notion_page_id,
            "notion_page_url": task.notion_page_url
        })
    );
    Ok(())
}

async fn run_worker(config: AppConfig, task_id: &str) -> Result<()> {
    let database = Database::open(&config.sqlite_path)?;
    let notion = NotionClient::new(&config)?;
    let codex = CodexRunner::new(config);
    process_task(&database, &notion, &codex, task_id).await
}

fn spawn_worker_process(
    config: &AppConfig,
    task_id: &str,
    prompt_template: Option<&str>,
) -> Result<()> {
    let current_exe = env::current_exe().context("failed to resolve current executable")?;
    let mut command = Command::new(current_exe);
    command
        .arg("worker")
        .arg("--task-id")
        .arg(task_id);
    apply_config_env(&mut command, config);
    if let Some(path) = prompt_template {
        command.arg("--prompt-template").arg(path);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
        .spawn()
        .context("failed to spawn background worker process")?;
    Ok(())
}

fn apply_config_env(command: &mut Command, config: &AppConfig) {
    command
        .env("DISCORD_TOKEN", &config.discord_token)
        .env(
            "DISCORD_ALLOWED_CHANNEL_IDS",
            config
                .discord_allowed_channel_ids
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
        .env("SQLITE_PATH", &config.sqlite_path)
        .env("LOG_FILE_PATH", &config.log_file_path)
        .env("RESEARCH_PROMPT_PATH", &config.research_prompt_path)
        .env("CODEX_BIN", &config.codex_bin)
        .env("CODEX_MODEL", config.codex_model.as_deref().unwrap_or(""))
        .env("WORKER_CONCURRENCY", config.worker_concurrency.to_string())
        .env("NOTION_TOKEN", config.notion_token.as_deref().unwrap_or(""))
        .env(
            "NOTION_TASK_DATABASE_ID",
            config.notion_task_database_id.as_deref().unwrap_or(""),
        )
        .env("PUBLIC_BASE_URL", &config.public_base_url);
}

fn read_flag(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    while let Some(arg) = args.next() {
        if arg == flag {
            return args
                .next()
                .ok_or_else(|| anyhow!("missing value for {}", flag));
        }
    }
    Err(anyhow!("missing required flag {}", flag))
}

fn read_optional_flag(args: &mut impl Iterator<Item = String>, flag: &str) -> Option<String> {
    while let Some(arg) = args.next() {
        if arg == flag {
            return args.next();
        }
    }
    None
}

fn build_title(prompt: &str) -> String {
    let mut title = prompt
        .lines()
        .next()
        .unwrap_or("Local analysis task")
        .trim()
        .to_string();
    if title.chars().count() > 80 {
        title = title.chars().take(80).collect();
        title.push_str("...");
    }
    if title.is_empty() {
        "Local analysis task".to_string()
    } else {
        title
    }
}

#[cfg(test)]
mod tests {
    use super::read_optional_flag;

    #[test]
    fn reads_optional_flag_when_present() {
        let mut args = vec![
            "--path".to_string(),
            "/tmp/input.json".to_string(),
            "--prompt-template".to_string(),
            "prompts/custom.txt".to_string(),
        ]
        .into_iter();

        let value = read_optional_flag(&mut args, "--prompt-template");

        assert_eq!(value.as_deref(), Some("prompts/custom.txt"));
    }

    #[test]
    fn returns_none_when_optional_flag_is_missing() {
        let mut args = vec!["--path".to_string(), "/tmp/input.json".to_string()].into_iter();

        let value = read_optional_flag(&mut args, "--prompt-template");

        assert_eq!(value, None);
    }
}
