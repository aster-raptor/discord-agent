use std::sync::Arc;

use anyhow::{Context as _, Result};
use serenity::async_trait;
use serenity::http::Http;
use serenity::model::channel::{Channel, ChannelType, Message};
use serenity::model::gateway::Ready;
use serenity::model::id::ChannelId;
use serenity::prelude::*;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{error, info, warn};
use url::Url;

use crate::codex::{CodexOutput, CodexRunner};
use crate::config::AppConfig;
use crate::db::Database;
use crate::models::{TaskJob, TaskRecord, TaskStatus, TaskType};
use crate::notion::NotionClient;

pub async fn run(config: AppConfig) -> Result<()> {
    config.validate_for_bot()?;

    let database = Arc::new(Database::open(&config.sqlite_path)?);
    let notion = Arc::new(NotionClient::new(&config)?);
    let codex = Arc::new(CodexRunner::new(config.clone()));
    let progress = Arc::new(DiscordProgressReporter::default());
    let (tx, rx) = mpsc::channel::<TaskJob>(1024);
    let rx = Arc::new(Mutex::new(rx));

    for worker_id in 0..config.worker_concurrency {
        let worker_state = WorkerState {
            config: config.clone(),
            database: Arc::clone(&database),
            notion: Arc::clone(&notion),
            codex: Arc::clone(&codex),
            progress: Arc::clone(&progress),
            rx: Arc::clone(&rx),
        };
        tokio::spawn(async move {
            if let Err(error) = worker_loop(worker_id, worker_state).await {
                error!("worker {} stopped: {:?}", worker_id, error);
            }
        });
    }

    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILD_MEMBERS;

    let handler = Handler {
        state: Arc::new(BotState {
            config,
            database,
            progress,
            tx,
        }),
    };

    let mut client = Client::builder(&handler.state.config.discord_token, intents)
        .event_handler(handler)
        .await
        .context("failed to build discord client")?;

    client.start().await.context("discord client exited")?;
    Ok(())
}

struct Handler {
    state: Arc<BotState>,
}

struct BotState {
    config: AppConfig,
    database: Arc<Database>,
    progress: Arc<DiscordProgressReporter>,
    tx: mpsc::Sender<TaskJob>,
}

struct WorkerState {
    config: AppConfig,
    database: Arc<Database>,
    notion: Arc<NotionClient>,
    codex: Arc<CodexRunner>,
    progress: Arc<DiscordProgressReporter>,
    rx: Arc<Mutex<mpsc::Receiver<TaskJob>>>,
}

#[derive(Default)]
struct DiscordProgressReporter {
    http: RwLock<Option<Arc<Http>>>,
}

impl DiscordProgressReporter {
    async fn set_http(&self, http: Arc<Http>) {
        let mut guard = self.http.write().await;
        *guard = Some(http);
    }

    async fn send(&self, channel_id: u64, message: &str) {
        let http = {
            let guard = self.http.read().await;
            guard.clone()
        };

        if let Some(http) = http {
            if let Err(error) = ChannelId(channel_id).say(&http, message).await {
                warn!(
                    "failed to send progress message to {}: {:?}",
                    channel_id, error
                );
            }
        }
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        self.state.progress.set_http(ctx.http.clone()).await;
        info!("connected as {}", ready.user.name);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        let guild_id = match msg.guild_id {
            Some(guild_id) => guild_id,
            None => return,
        };

        if !is_thread_message(&ctx, &msg).await {
            return;
        }

        match is_allowed(&ctx, &msg, &self.state.config).await {
            Ok(true) => {}
            Ok(false) => {
                let _ = msg
                    .reply(
                        &ctx.http,
                        "This bot is restricted to the configured allowlist.",
                    )
                    .await;
                return;
            }
            Err(error) => {
                error!("failed to authorize message: {:?}", error);
                return;
            }
        }

        let task_type = infer_task_type(&msg.content);
        if matches!(task_type, TaskType::Coding) {
            let _ = msg
                .reply(
                    &ctx.http,
                    "Coding tasks are wired for future approval flow, but v1 currently executes research tasks only.",
                )
                .await;
            return;
        }

        let title = build_title(&msg.content);
        let prompt = build_prompt_from_message(&msg.content);
        let mut task = TaskRecord::new(
            msg.channel_id.0,
            msg.channel_id.0,
            guild_id.0,
            msg.author.id.0,
            msg.id.0,
            title,
            prompt,
            task_type,
        );

        if let Err(error) = self.state.database.insert_task(&task) {
            error!("failed to insert task: {:?}", error);
            let _ = msg.reply(&ctx.http, "Failed to persist task.").await;
            return;
        }

        if let Err(error) =
            self.state
                .database
                .update_status(&task.id, TaskStatus::Queued, Some("task queued"))
        {
            error!("failed to queue task status: {:?}", error);
        }
        task.status = TaskStatus::Queued;

        if let Err(error) = self
            .state
            .tx
            .send(TaskJob {
                task_id: task.id.clone(),
            })
            .await
        {
            error!("failed to enqueue task: {:?}", error);
            let _ = msg.reply(&ctx.http, "Failed to enqueue task.").await;
            return;
        }

        let queue_message = format!(
            "Accepted task `{}`.\nStatus: queued\nTask ID: `{}`",
            task.title, task.id
        );
        let _ = msg.reply(&ctx.http, queue_message).await;
    }
}

async fn worker_loop(worker_id: usize, state: WorkerState) -> Result<()> {
    loop {
        let job = {
            let mut receiver = state.rx.lock().await;
            receiver.recv().await
        };

        let job = match job {
            Some(job) => job,
            None => return Ok(()),
        };

        let task = match state.database.get_task(&job.task_id) {
            Ok(task) => task,
            Err(error) => {
                error!(
                    "worker {} failed to load task {}: {:?}",
                    worker_id, job.task_id, error
                );
                continue;
            }
        };

        state
            .progress
            .send(
                task.channel_id,
                &format!("Task `{}` is now running.", task.id),
            )
            .await;

        if let Err(error) = state.database.mark_running(&task.id) {
            error!("failed to mark task running: {:?}", error);
        }

        let output = state.codex.run_research(&task).await;
        match handle_task_completion(&state, &task, output).await {
            Ok(()) => {}
            Err(error) => {
                error!(
                    "worker {} failed to finish task {}: {:?}",
                    worker_id, task.id, error
                );
            }
        }
    }
}

async fn handle_task_completion(
    state: &WorkerState,
    task: &TaskRecord,
    output: Result<CodexOutput>,
) -> Result<()> {
    match output {
        Ok(output) => {
            if !output.success {
                let raw_output = render_raw_output(&output);
                state.database.fail_task(
                    &task.id,
                    "codex exited with a non-zero status",
                    Some(&raw_output),
                )?;
                state
                    .progress
                    .send(
                        task.channel_id,
                        &format!(
                            "Task `{}` failed.\n{}",
                            task.id,
                            truncate_for_discord(&raw_output)
                        ),
                    )
                    .await;
                return Ok(());
            }

            state
                .progress
                .send(
                    task.channel_id,
                    &format!("Task `{}` is summarizing.", task.id),
                )
                .await;
            state.database.mark_summarizing(&task.id)?;

            let mut task = state.database.get_task(&task.id)?;
            task.raw_output = Some(render_raw_output(&output));
            task.public_summary = Some(build_public_summary(&output.stdout));
            task.updated_at = chrono::Utc::now().to_rfc3339();
            task.completed_at = Some(task.updated_at.clone());
            task.publish = true;

            let notion_page_id = state.notion.publish_task(&task).await?;
            state.database.complete_task(
                &task.id,
                task.public_summary.as_deref().unwrap_or(""),
                task.raw_output.as_deref().unwrap_or(""),
                notion_page_id.as_deref(),
                true,
            )?;

            let completion_message = format!(
                "Task `{}` completed.\nPublic summary:\n{}",
                task.id,
                task.public_summary.as_deref().unwrap_or("No summary.")
            );
            state
                .progress
                .send(task.channel_id, &completion_message)
                .await;
        }
        Err(error) => {
            let message = error.to_string();
            state.database.fail_task(&task.id, &message, None)?;
            state
                .progress
                .send(
                    task.channel_id,
                    &format!("Task `{}` failed.\n{}", task.id, message),
                )
                .await;
        }
    }

    Ok(())
}

fn render_raw_output(output: &CodexOutput) -> String {
    if output.stderr.trim().is_empty() {
        output.stdout.clone()
    } else {
        format!(
            "STDOUT\n{}\n\nSTDERR\n{}",
            output.stdout.trim(),
            output.stderr.trim()
        )
    }
}

fn build_public_summary(output: &str) -> String {
    let mut summary = output.trim().to_string();
    if summary.chars().count() > 1200 {
        summary = summary.chars().take(1200).collect();
        summary.push_str("\n...");
    }
    summary
}

fn truncate_for_discord(value: &str) -> String {
    let mut result = value.chars().take(1500).collect::<String>();
    if value.chars().count() > 1500 {
        result.push_str("\n...");
    }
    result
}

async fn is_thread_message(ctx: &Context, msg: &Message) -> bool {
    match msg.channel(&ctx.http).await {
        Ok(Channel::Guild(channel)) => matches!(
            channel.kind,
            ChannelType::PublicThread | ChannelType::PrivateThread | ChannelType::NewsThread
        ),
        _ => false,
    }
}

async fn is_allowed(ctx: &Context, msg: &Message, config: &AppConfig) -> Result<bool> {
    if config.discord_allowed_user_ids.contains(&msg.author.id.0) {
        return Ok(true);
    }

    let member = match &msg.member {
        Some(member) => member.clone(),
        None => msg
            .guild_id
            .unwrap()
            .member(&ctx.http, msg.author.id)
            .await
            .context("failed to fetch guild member")?,
    };

    for role_id in member.roles {
        if config.discord_allowed_role_ids.contains(&role_id.0) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn infer_task_type(content: &str) -> TaskType {
    let lowered = content.to_ascii_lowercase();
    if lowered.contains("codex exec")
        || lowered.contains("コードを書いて")
        || lowered.contains("fix ")
        || lowered.contains("refactor")
    {
        return TaskType::Coding;
    }
    TaskType::Research
}

fn build_title(content: &str) -> String {
    let mut title = content
        .lines()
        .next()
        .unwrap_or("Discord task")
        .trim()
        .to_string();
    if title.chars().count() > 80 {
        title = title.chars().take(80).collect();
        title.push_str("...");
    }
    if title.is_empty() {
        "Discord task".to_string()
    } else {
        title
    }
}

fn build_prompt_from_message(content: &str) -> String {
    let urls = extract_urls(content);
    if urls.is_empty() {
        return content.to_string();
    }

    format!("{}\n\nReferenced URLs:\n{}", content, urls.join("\n"))
}

fn extract_urls(content: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for token in content.split_whitespace() {
        let trimmed = token.trim_matches(|c: char| {
            c == '(' || c == ')' || c == '[' || c == ']' || c == ',' || c == '.'
        });
        if Url::parse(trimmed).is_ok() {
            urls.push(trimmed.to_string());
        }
    }
    urls.sort();
    urls.dedup();
    urls
}

#[cfg(test)]
mod tests {
    use super::{build_prompt_from_message, extract_urls};

    #[test]
    fn extracts_urls_from_message() {
        let urls = extract_urls("look at https://example.com and https://example.com/a.");
        assert_eq!(urls.len(), 2);
    }

    #[test]
    fn keeps_prompt_and_url_section() {
        let prompt = build_prompt_from_message("summarize https://example.com");
        assert!(prompt.contains("Referenced URLs"));
    }
}
