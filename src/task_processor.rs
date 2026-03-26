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
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "No summary available.".to_string();
    }

    let summary_source = first_summary_line(trimmed).unwrap_or(trimmed);
    let first_sentence = first_sentence(summary_source);
    truncate_with_ellipsis(first_sentence, 80)
}

fn first_summary_line(value: &str) -> Option<&str> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !looks_like_structural_line(line))
}

fn looks_like_structural_line(value: &str) -> bool {
    let normalized = normalize_heading_candidate(value);
    normalized.is_empty()
        || normalized == "stdout"
        || normalized == "stderr"
        || normalized.contains("\u{8981}\u{7d04}")
        || normalized.contains("\u{4e3b}\u{8981}\u{30dd}\u{30a4}\u{30f3}\u{30c8}")
        || normalized.contains("\u{6b21}\u{306b}\u{898b}\u{308b}\u{3079}\u{304d}\u{70b9}")
}

fn normalize_heading_candidate(value: &str) -> String {
    value
        .trim()
        .trim_matches('*')
        .trim_start_matches('#')
        .trim()
        .trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, '.' | ')' | ':' | '-' | ' '))
        .trim()
        .trim_matches('*')
        .trim()
        .to_lowercase()
}

fn first_sentence(value: &str) -> &str {
    let mut sentence_end = value.len();

    for (index, ch) in value.char_indices() {
        if matches!(ch, '\u{3002}' | '.' | '!' | '?' | '\n' | '\r') {
            sentence_end = index + ch.len_utf8();
            break;
        }
    }

    value[..sentence_end].trim()
}

fn truncate_with_ellipsis(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }

    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
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

#[cfg(test)]
mod tests {
    use super::build_public_summary;

    #[test]
    fn keeps_only_first_sentence() {
        let summary = build_public_summary("First sentence. Second sentence.");
        assert_eq!(summary, "First sentence.");
    }

    #[test]
    fn keeps_only_first_line() {
        let summary = build_public_summary("First line\nSecond line");
        assert_eq!(summary, "First line");
    }

    #[test]
    fn skips_markdown_heading_when_building_summary() {
        let summary = build_public_summary("**1. 要約**\n住友電工は非鉄・電線分野の大手企業です。");
        assert_eq!(summary, "住友電工は非鉄・電線分野の大手企業です。");
    }

    #[test]
    fn truncates_long_single_sentence() {
        let summary = build_public_summary(&"a".repeat(200));
        assert_eq!(summary.chars().count(), 83);
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn falls_back_for_empty_output() {
        let summary = build_public_summary("   ");
        assert_eq!(summary, "No summary available.");
    }
}