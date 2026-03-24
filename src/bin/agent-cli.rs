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
            submit_task(config, &prompt, &path).await
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
            run_worker(config, &task_id).await
        }
        other => Err(anyhow!("unknown command: {}", other)),
    }
}

async fn submit_task(config: AppConfig, prompt: &str, path: &str) -> Result<()> {
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
    spawn_worker_process(&task.id)?;

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

fn spawn_worker_process(task_id: &str) -> Result<()> {
    let current_exe = env::current_exe().context("failed to resolve current executable")?;
    let mut command = Command::new(current_exe);
    command
        .arg("worker")
        .arg("--task-id")
        .arg(task_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
        .spawn()
        .context("failed to spawn background worker process")?;
    Ok(())
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
