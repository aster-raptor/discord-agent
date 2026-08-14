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
use tracing::warn;

struct SubmitArgs {
    prompt: String,
    path: String,
    prompt_template: Option<String>,
    previous_task_id: Option<String>,
}

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
            let submit_args = parse_submit_args(args.collect::<Vec<_>>())?;
            submit_task(
                config,
                &submit_args.prompt,
                &submit_args.path,
                submit_args.prompt_template.as_deref(),
                submit_args.previous_task_id.as_deref(),
            )
            .await
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
    previous_task_id: Option<&str>,
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
    attach_previous_task(&database, &mut task, previous_task_id);

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
            "notion_page_url": task.notion_page_url,
            "previous_task_id": task.previous_task_id,
            "previous_notion_page_url": task.previous_notion_page_url
        })
    );
    Ok(())
}

fn attach_previous_task(
    database: &Database,
    task: &mut TaskRecord,
    previous_task_id: Option<&str>,
) {
    let Some(previous_task_id) = previous_task_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    task.previous_task_id = Some(previous_task_id.to_string());
    match database.get_task(previous_task_id) {
        Ok(previous_task) => match previous_task
            .notion_page_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(url) => task.previous_notion_page_url = Some(url.to_string()),
            None => warn!(
                previous_task_id,
                "previous task has no notion page URL; continuing without a history link"
            ),
        },
        Err(error) => warn!(
            previous_task_id,
            error = %error,
            "previous task was not found; continuing without a history link"
        ),
    }
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

fn parse_submit_args(args: Vec<String>) -> Result<SubmitArgs> {
    Ok(SubmitArgs {
        prompt: find_flag_value(&args, "--prompt")?
            .ok_or_else(|| anyhow!("missing required flag --prompt"))?,
        path: find_flag_value(&args, "--path")?
            .ok_or_else(|| anyhow!("missing required flag --path"))?,
        prompt_template: find_flag_value(&args, "--prompt-template")?,
        previous_task_id: find_flag_value(&args, "--previous-task-id")?,
    })
}

fn find_flag_value(args: &[String], flag: &str) -> Result<Option<String>> {
    let Some(index) = args.iter().position(|value| value == flag) else {
        return Ok(None);
    };
    let value = args
        .get(index + 1)
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| anyhow!("missing value for {}", flag))?;
    Ok(Some(value.clone()))
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
    use super::{attach_previous_task, parse_submit_args, read_optional_flag};
    use discord_agent::db::Database;
    use discord_agent::models::{TaskRecord, TaskType};

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

    #[test]
    fn parses_submit_flags_in_any_order_with_previous_task() {
        let parsed = parse_submit_args(vec![
            "--previous-task-id".into(),
            "previous-id".into(),
            "--path".into(),
            "/tmp/input.json".into(),
            "--prompt".into(),
            "Telegram analysis".into(),
            "--prompt-template".into(),
            "prompts/telegram_summary.txt".into(),
        ])
        .unwrap();

        assert_eq!(parsed.prompt, "Telegram analysis");
        assert_eq!(parsed.path, "/tmp/input.json");
        assert_eq!(parsed.previous_task_id.as_deref(), Some("previous-id"));
        assert_eq!(
            parsed.prompt_template.as_deref(),
            Some("prompts/telegram_summary.txt")
        );
    }

    #[test]
    fn parses_submit_without_optional_previous_task() {
        let parsed = parse_submit_args(vec![
            "--prompt".into(),
            "analysis".into(),
            "--path".into(),
            "/tmp/input.json".into(),
        ])
        .unwrap();

        assert_eq!(parsed.previous_task_id, None);
        assert_eq!(parsed.prompt_template, None);
    }

    #[test]
    fn resolves_previous_notion_page_url_without_blocking_missing_tasks() {
        let database = Database::open(":memory:").unwrap();
        let mut previous = TaskRecord::new(
            0,
            0,
            0,
            "previous".into(),
            "prompt".into(),
            TaskType::Research,
        );
        previous.notion_page_url = Some("https://www.notion.so/previous".into());
        database.insert_task(&previous).unwrap();

        let mut linked = TaskRecord::new(
            0,
            0,
            0,
            "linked".into(),
            "prompt".into(),
            TaskType::Research,
        );
        attach_previous_task(&database, &mut linked, Some(&previous.id));
        assert_eq!(
            linked.previous_task_id.as_deref(),
            Some(previous.id.as_str())
        );
        assert_eq!(
            linked.previous_notion_page_url.as_deref(),
            Some("https://www.notion.so/previous")
        );

        let mut missing = TaskRecord::new(
            0,
            0,
            0,
            "missing".into(),
            "prompt".into(),
            TaskType::Research,
        );
        attach_previous_task(&database, &mut missing, Some("missing-task"));
        assert_eq!(missing.previous_task_id.as_deref(), Some("missing-task"));
        assert_eq!(missing.previous_notion_page_url, None);
    }
}
