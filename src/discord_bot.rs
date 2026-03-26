use std::sync::Arc;

use anyhow::{Context as _, Result};
use serenity::async_trait;
use serenity::http::Http;
use serenity::model::application::command::Command;
use serenity::model::application::command::CommandOptionType;
use serenity::model::application::interaction::application_command::ApplicationCommandInteraction;
use serenity::model::application::interaction::Interaction;
use serenity::model::application::interaction::InteractionResponseType;
use serenity::model::channel::Message;
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
use crate::task_processor::build_public_summary;

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

    let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

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
        if let Err(error) = register_commands(&ctx).await {
            error!("failed to register application commands: {:?}", error);
        }
        info!("connected as {}", ready.user.name);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        if msg.guild_id.is_none() {
            return;
        }

        if !is_allowed_channel(&self.state.config, msg.channel_id) {
            return;
        }

        let _ = msg.reply(&ctx.http, usage_message()).await;
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Interaction::ApplicationCommand(command) = interaction else {
            return;
        };

        info!(
            command = %command.data.name,
            channel_id = command.channel_id.0,
            guild_id = ?command.guild_id.map(|id| id.0),
            user_id = command.user.id.0,
            "received slash command"
        );
        let result = match command.data.name.as_str() {
            "research" => handle_research_command(&self.state, &ctx, &command).await,
            "status" => handle_status_command(&self.state, &ctx, &command).await,
            "help" => handle_help_command(&self.state, &ctx, &command).await,
            _ => Ok(()),
        };

        if let Err(error) = result {
            error!("failed to handle interaction {}: {:?}", command.data.name, error);
            let _ = command
                .create_interaction_response(&ctx.http, |response| {
                    response
                        .kind(InteractionResponseType::ChannelMessageWithSource)
                        .interaction_response_data(|message| {
                            message
                                .content("Failed to handle command.")
                                .ephemeral(true)
                        })
                })
                .await;
        }
    }
}

async fn register_commands(ctx: &Context) -> Result<()> {
    Command::set_global_application_commands(&ctx.http, |commands| {
        commands
            .create_application_command(|command| {
                command
                    .name("research")
                    .description("Queue a research task from the current channel")
                    .create_option(|option| {
                        option
                            .name("prompt")
                            .description("Research request")
                            .kind(CommandOptionType::String)
                            .required(true)
                    })
            })
            .create_application_command(|command| {
                command
                    .name("status")
                    .description("Show the current status of a task")
                    .create_option(|option| {
                        option
                            .name("task_id")
                            .description("Task ID to inspect")
                            .kind(CommandOptionType::String)
                            .required(true)
                    })
            })
            .create_application_command(|command| {
                command
                    .name("help")
                    .description("Show available bot commands")
            })
    })
    .await
    .context("failed to register global application commands")?;
    Ok(())
}

async fn handle_research_command(
    state: &Arc<BotState>,
    ctx: &Context,
    command: &ApplicationCommandInteraction,
) -> Result<()> {
    if command.guild_id.is_none() {
        respond_ephemeral(ctx, command, "Use this bot inside an allowed server channel.").await?;
        return Ok(());
    }

    if !ensure_allowed_channel(state, ctx, command).await? {
        return Ok(());
    }

    let Some(prompt) = command_option_string(command, "prompt") else {
        respond_ephemeral(ctx, command, "Missing required option: prompt").await?;
        return Ok(());
    };

    let title = build_title(&prompt);
    let prompt = build_prompt_from_message(&prompt);
    let mut task = TaskRecord::new(
        command.channel_id.0,
        command.channel_id.0,
        command.id.0,
        title,
        prompt,
        TaskType::Research,
    );
    info!(
        task_id = %task.id,
        channel_id = task.channel_id,
        title = %task.title,
        "created research task from slash command"
    );

    if let Err(error) = state.database.insert_task(&task) {
        error!("failed to insert task: {:?}", error);
        respond_ephemeral(ctx, command, "Failed to persist task.").await?;
        return Ok(());
    }

    if let Err(error) = state
        .database
        .update_status(&task.id, TaskStatus::Queued, Some("task queued"))
    {
        error!("failed to queue task status: {:?}", error);
    }
    task.status = TaskStatus::Queued;
    info!(task_id = %task.id, "queued task");

    if let Err(error) = state
        .tx
        .send(TaskJob {
            task_id: task.id.clone(),
        })
        .await
    {
        error!("failed to enqueue task: {:?}", error);
        respond_ephemeral(ctx, command, "Failed to enqueue task.").await?;
        return Ok(());
    }

    let queue_message = format!(
        "Accepted task `{}`.\nStatus: queued\nTask ID: `{}`",
        task.title, task.id
    );
    respond_public(ctx, command, &queue_message).await?;
    Ok(())
}

async fn handle_status_command(
    state: &Arc<BotState>,
    ctx: &Context,
    command: &ApplicationCommandInteraction,
) -> Result<()> {
    if command.guild_id.is_none() {
        respond_ephemeral(ctx, command, "Use this bot inside an allowed server channel.").await?;
        return Ok(());
    }
    if !ensure_allowed_channel(state, ctx, command).await? {
        return Ok(());
    }

    let Some(task_id) = command_option_string(command, "task_id") else {
        respond_ephemeral(ctx, command, "Missing required option: task_id").await?;
        return Ok(());
    };
    info!(task_id = %task_id, "received status lookup");

    match state.database.get_task(&task_id) {
        Ok(task) => {
            let message = render_task_status(&task);
            respond_ephemeral(ctx, command, &message).await?;
        }
        Err(_) => {
            respond_ephemeral(ctx, command, &format!("Task not found: `{}`", task_id)).await?;
        }
    }

    Ok(())
}

async fn handle_help_command(
    state: &Arc<BotState>,
    ctx: &Context,
    command: &ApplicationCommandInteraction,
) -> Result<()> {
    if command.guild_id.is_none() {
        respond_ephemeral(ctx, command, "Use this bot inside an allowed server channel.").await?;
        return Ok(());
    }
    if !ensure_allowed_channel(state, ctx, command).await? {
        return Ok(());
    }
    respond_ephemeral(ctx, command, usage_message()).await
}

async fn ensure_allowed_channel(
    state: &Arc<BotState>,
    ctx: &Context,
    command: &ApplicationCommandInteraction,
) -> Result<bool> {
    if is_allowed_channel(&state.config, command.channel_id) {
        return Ok(true);
    }

    warn!(
        command = %command.data.name,
        channel_id = command.channel_id.0,
        allowed_channel_ids = ?state.config.discord_allowed_channel_ids,
        "rejected slash command from disallowed channel"
    );
    respond_ephemeral(
        ctx,
        command,
        "This bot can only be used in configured Discord channels.",
    )
    .await?;
    Ok(false)
}

async fn respond_public(
    ctx: &Context,
    command: &ApplicationCommandInteraction,
    content: &str,
) -> Result<()> {
    command
        .create_interaction_response(&ctx.http, |response| {
            response
                .kind(InteractionResponseType::ChannelMessageWithSource)
                .interaction_response_data(|message| message.content(content))
        })
        .await
        .context("failed to send public interaction response")?;
    Ok(())
}

async fn respond_ephemeral(
    ctx: &Context,
    command: &ApplicationCommandInteraction,
    content: &str,
) -> Result<()> {
    command
        .create_interaction_response(&ctx.http, |response| {
            response
                .kind(InteractionResponseType::ChannelMessageWithSource)
                .interaction_response_data(|message| message.content(content).ephemeral(true))
        })
        .await
        .context("failed to send ephemeral interaction response")?;
    Ok(())
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
        info!(worker_id, task_id = %task.id, "starting task execution");

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
            info!(
                task_id = %task.id,
                stdout_len = output.stdout.len(),
                stderr_len = output.stderr.len(),
                success = output.success,
                "codex execution finished"
            );
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

            info!(task_id = %task.id, "saving completed task to notion");
            let published_page = state.notion.publish_task(&task).await?;
            state.database.complete_task(
                &task.id,
                task.public_summary.as_deref().unwrap_or(""),
                task.raw_output.as_deref().unwrap_or(""),
                published_page.as_ref().map(|page| page.id.as_str()),
                published_page.as_ref().map(|page| page.url.as_str()),
                true,
            )?;
            info!(
                task_id = %task.id,
                notion_page_id = ?published_page.as_ref().map(|page| page.id.as_str()),
                notion_page_url = ?published_page.as_ref().map(|page| page.url.as_str()),
                "task completed successfully"
            );

            let completion_message = match published_page.as_ref() {
                Some(page) => format!("Task `{}` completed.\nNotion Page: {}", task.id, page.url),
                None => format!("Task `{}` completed.", task.id),
            };
            state
                .progress
                .send(task.channel_id, &completion_message)
                .await;
        }
        Err(error) => {
            let message = error.to_string();
            error!(task_id = %task.id, error = %message, "task execution failed");
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

fn truncate_for_discord(value: &str) -> String {
    let mut result = value.chars().take(1500).collect::<String>();
    if value.chars().count() > 1500 {
        result.push_str("\n...");
    }
    result
}

fn is_allowed_channel(config: &AppConfig, channel_id: ChannelId) -> bool {
    config
        .discord_allowed_channel_ids
        .iter()
        .any(|allowed| *allowed == channel_id.0)
}

fn command_option_string(
    command: &ApplicationCommandInteraction,
    option_name: &str,
) -> Option<String> {
    command
        .data
        .options
        .iter()
        .find(|option| option.name == option_name)
        .and_then(|option| option.value.as_ref())
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
}

fn usage_message() -> &'static str {
    "Use `/research` to start a task in this channel.\nUse `/status` to check a Task ID.\nUse `/help` to show this message."
}

fn render_task_status(task: &TaskRecord) -> String {
    let mut message = format!(
        "Task ID: `{}`\nTitle: {}\nStatus: {}",
        task.id,
        task.title,
        task.status.as_str()
    );

    match task.status {
        TaskStatus::Completed => {
            if let Some(summary) = &task.public_summary {
                message.push_str(&format!("\nSummary:\n{}", truncate_for_discord(summary)));
            }
        }
        TaskStatus::Failed => {
            if let Some(error_text) = &task.error_text {
                message.push_str(&format!("\nError:\n{}", truncate_for_discord(error_text)));
            }
        }
        _ => {}
    }

    message
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
    use super::{build_prompt_from_message, extract_urls, render_task_status, usage_message};
    use crate::models::{TaskRecord, TaskStatus, TaskType};

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

    #[test]
    fn usage_mentions_all_commands() {
        let usage = usage_message();
        assert!(usage.contains("/research"));
        assert!(usage.contains("/status"));
        assert!(usage.contains("/help"));
    }

    #[test]
    fn completed_status_includes_summary() {
        let mut task = TaskRecord::new(1, 1, 1, "hello".into(), "prompt".into(), TaskType::Research);
        task.status = TaskStatus::Completed;
        task.public_summary = Some("done".into());
        let message = render_task_status(&task);
        assert!(message.contains("done"));
    }
}
