use anyhow::Result;
use tracing::{error, info};

use crate::codex::{CodexOutput, CodexRunner};
use crate::db::Database;
use crate::models::TaskRecord;
use crate::notion::NotionClient;

pub async fn process_task(
    database: &Database,
    notion: &NotionClient,
    codex: &CodexRunner,
    task_id: &str,
) -> Result<()> {
    let task = database.get_task(task_id)?;
    info!(task_id = %task.id, "starting CLI task execution");

    database.mark_running(&task.id)?;
    let output = codex.run_research(&task).await;
    handle_task_completion(database, notion, &task, output).await
}

pub fn render_raw_output(output: &CodexOutput) -> String {
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

pub fn build_public_summary(output: &str) -> String {
    let mut summary = output.trim().to_string();
    if summary.chars().count() > 1200 {
        summary = summary.chars().take(1200).collect();
        summary.push_str("\n...");
    }
    summary
}

async fn handle_task_completion(
    database: &Database,
    notion: &NotionClient,
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
                database.fail_task(
                    &task.id,
                    "codex exited with a non-zero status",
                    Some(&raw_output),
                )?;
                return Ok(());
            }

            database.mark_summarizing(&task.id)?;

            let mut task = database.get_task(&task.id)?;
            task.raw_output = Some(render_raw_output(&output));
            task.public_summary = Some(build_public_summary(&output.stdout));
            task.updated_at = chrono::Utc::now().to_rfc3339();
            task.completed_at = Some(task.updated_at.clone());
            task.publish = true;

            info!(task_id = %task.id, "saving completed task to notion");
            let published_page = notion.publish_task(&task).await?;
            database.complete_task(
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
        }
        Err(error) => {
            let message = error.to_string();
            error!(task_id = %task.id, error = %message, "task execution failed");
            database.fail_task(&task.id, &message, None)?;
        }
    }

    Ok(())
}
