use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{error, info};

use crate::config::AppConfig;
use crate::local_input::build_input_summary;
use crate::models::{PublicTaskSummary, TaskRecord};
use crate::task_processor::build_public_summary;

const NOTION_VERSION: &str = "2022-06-28";

#[derive(Clone)]
pub struct NotionClient {
    client: Client,
    token: Option<String>,
    database_id: Option<String>,
    public_base_url: String,
}

#[derive(Clone, Debug)]
pub struct PublishedPage {
    pub id: String,
    pub url: String,
}

impl NotionClient {
    pub fn new(config: &AppConfig) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert("Notion-Version", HeaderValue::from_static(NOTION_VERSION));

        if let Some(token) = &config.notion_token {
            let bearer = format!("Bearer {}", token);
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&bearer).context("invalid notion token")?,
            );
        }

        let client = Client::builder()
            .default_headers(headers)
            .build()
            .context("failed to build notion http client")?;

        Ok(Self {
            client,
            token: config.notion_token.clone(),
            database_id: config.notion_task_database_id.clone(),
            public_base_url: config.public_base_url.clone(),
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.token.is_some() && self.database_id.is_some()
    }

    pub async fn publish_task(&self, task: &TaskRecord) -> Result<Option<PublishedPage>> {
        if !self.is_enabled() {
            return Ok(None);
        }

        let database_id = self.database_id.as_ref().unwrap();
        info!(task_id = %task.id, notion_database_id = %database_id, "publishing task to notion");
        let summary = truncate(&build_public_summary_text(task), 1800);
        let report = parse_report_sections(task);
        let payload = json!({
            "parent": { "database_id": database_id },
            "properties": {
                "Task ID": rich_text_property(&task.id),
                "Title": title_property(&task.title),
                "Status": select_property("Completed"),
                "Task Type": select_property(match task.task_type.as_str() {
                    "coding" => "coding",
                    _ => "research",
                }),
                "Publish": { "checkbox": true },
                "Public Summary": rich_text_property(&summary),
                "Updated At": { "date": { "start": task.updated_at } },
                "Completed At": { "date": { "start": task.completed_at } },
                "Thread ID": rich_text_property(&task.thread_id.to_string()),
                "Public URL": rich_text_property(&format!("{}/tasks/{}", self.public_base_url, task.id))
            },
            "children": build_page_children(task, &report)
        });

        let response = self
            .client
            .post("https://api.notion.com/v1/pages")
            .json(&payload)
            .send()
            .await
            .context("failed to call notion create page")?;

        let status = response.status();
        let body_text = response
            .text()
            .await
            .context("failed to read notion create page response body")?;
        if !status.is_success() {
            error!(task_id = %task.id, http_status = %status, response_body = %body_text, "notion create page returned error");
            return Err(anyhow!(
                "notion create page returned error: {} {}",
                status,
                body_text
            ));
        }

        let body: Value = serde_json::from_str(&body_text).context("invalid notion response")?;
        let page_id = body
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("notion response did not include page id"))?;
        let page_url = body
            .get("url")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("notion response did not include page url"))?;

        info!(task_id = %task.id, notion_page_id = %page_id, notion_page_url = %page_url, "published task to notion");
        Ok(Some(PublishedPage {
            id: page_id.to_string(),
            url: page_url.to_string(),
        }))
    }

    pub async fn query_published_tasks(&self, limit: usize) -> Result<Vec<PublicTaskSummary>> {
        if !self.is_enabled() {
            return Ok(Vec::new());
        }

        let database_id = self.database_id.as_ref().unwrap();
        let payload = json!({
            "page_size": limit,
            "filter": {
                "property": "Publish",
                "checkbox": { "equals": true }
            },
            "sorts": [
                {
                    "property": "Updated At",
                    "direction": "descending"
                }
            ]
        });

        let response = self
            .client
            .post(&format!(
                "https://api.notion.com/v1/databases/{}/query",
                database_id
            ))
            .json(&payload)
            .send()
            .await
            .context("failed to query notion database")?;

        let status = response.status();
        let body_text = response
            .text()
            .await
            .context("failed to read notion query response body")?;
        if !status.is_success() {
            error!(http_status = %status, response_body = %body_text, "notion query returned error");
            return Err(anyhow!(
                "notion query returned error: {} {}",
                status,
                body_text
            ));
        }

        let body: Value = serde_json::from_str(&body_text).context("invalid notion query response")?;
        let results = body
            .get("results")
            .and_then(|value| value.as_array())
            .ok_or_else(|| anyhow!("notion query did not include results"))?;

        let mut items = Vec::new();
        for page in results {
            let empty = Value::Null;
            let properties = page.get("properties").unwrap_or(&empty);
            let task_id = extract_plain_text(properties, "Task ID");
            if task_id.is_empty() {
                continue;
            }

            items.push(PublicTaskSummary {
                task_id,
                title: extract_title(properties, "Title"),
                summary: extract_plain_text(properties, "Public Summary"),
                completed_at: extract_date(properties, "Completed At"),
                updated_at: extract_date(properties, "Updated At").unwrap_or_default(),
            });
        }

        Ok(items)
    }

    pub async fn fetch_public_task(&self, task_id: &str) -> Result<Option<PublicTaskSummary>> {
        let tasks = self.query_published_tasks(100).await?;
        for task in tasks {
            if task.task_id == task_id {
                return Ok(Some(task));
            }
        }
        Ok(None)
    }
}

fn title_property(value: &str) -> Value {
    json!({
        "title": [{
            "type": "text",
            "text": { "content": truncate(value, 200) }
        }]
    })
}

fn rich_text_property(value: &str) -> Value {
    json!({
        "rich_text": [{
            "type": "text",
            "text": { "content": truncate(value, 2000) }
        }]
    })
}

fn select_property(value: &str) -> Value {
    json!({
        "select": { "name": value }
    })
}

fn heading_block(text: &str) -> Value {
    json!({
        "object": "block",
        "type": "heading_2",
        "heading_2": {
            "rich_text": [{
                "type": "text",
                "text": { "content": truncate(text, 200) }
            }]
        }
    })
}

fn paragraph_block(body: &str) -> Value {
    json!({
        "object": "block",
        "type": "paragraph",
        "paragraph": {
            "rich_text": [{
                "type": "text",
                "text": { "content": truncate(body, 1800) }
            }]
        }
    })
}

fn bulleted_list_item_block(body: &str) -> Value {
    json!({
        "object": "block",
        "type": "bulleted_list_item",
        "bulleted_list_item": {
            "rich_text": [{
                "type": "text",
                "text": { "content": truncate(body, 1800) }
            }]
        }
    })
}

fn build_public_summary_text(task: &TaskRecord) -> String {
    if let Some(summary) = &task.public_summary {
        if !summary.trim().is_empty() {
            return summary.trim().to_string();
        }
    }

    build_public_summary(&task.raw_output.clone().unwrap_or_default())
}

fn build_task_input_summary(task: &TaskRecord) -> Option<String> {
    match (&task.input_source_path, &task.input_payload) {
        (Some(source_path), Some(payload)) => Some(build_input_summary(source_path, payload)),
        (Some(source_path), None) => Some(format!("Source Path: {}", source_path)),
        _ => None,
    }
}

fn display_prompt(task: &TaskRecord) -> String {
    task.prompt
        .split("\n\nReferenced URLs:\n")
        .next()
        .unwrap_or(&task.prompt)
        .trim()
        .to_string()
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ReportSections {
    summary: String,
    key_points: Vec<String>,
    next_steps: Vec<String>,
}

fn build_page_children(task: &TaskRecord, report: &ReportSections) -> Vec<Value> {
    let mut children = Vec::new();
    let summary_body = if report.summary.is_empty() {
        build_public_summary_text(task)
    } else {
        report.summary.clone()
    };

    children.push(heading_block("要約"));
    children.push(paragraph_block(&summary_body));

    if !report.key_points.is_empty() {
        children.push(heading_block("主要ポイント"));
        for point in &report.key_points {
            children.push(bulleted_list_item_block(point));
        }
    }

    if !report.next_steps.is_empty() {
        children.push(heading_block("次に見るべき点"));
        for point in &report.next_steps {
            children.push(bulleted_list_item_block(point));
        }
    }

    children.push(heading_block("依頼内容"));
    children.push(paragraph_block(&truncate(&display_prompt(task), 1800)));

    if let Some(input_summary) = build_task_input_summary(task) {
        children.push(heading_block("入力データ概要"));
        children.push(paragraph_block(&input_summary));
    }

    children
}

fn parse_report_sections(task: &TaskRecord) -> ReportSections {
    let stdout = extract_stdout(task.raw_output.as_deref().unwrap_or_default());
    if stdout.trim().is_empty() {
        return ReportSections {
            summary: build_public_summary_text(task),
            ..ReportSections::default()
        };
    }

    let mut report = ReportSections::default();
    let mut current_section: Option<&str> = None;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if is_summary_heading(trimmed) {
            current_section = Some("summary");
            continue;
        }
        if is_key_points_heading(trimmed) {
            current_section = Some("key_points");
            continue;
        }
        if is_next_steps_heading(trimmed) {
            current_section = Some("next_steps");
            continue;
        }

        match current_section {
            Some("summary") => {
                if !report.summary.is_empty() {
                    report.summary.push('\n');
                }
                report.summary.push_str(trimmed);
            }
            Some("key_points") => report.key_points.push(clean_list_item(trimmed)),
            Some("next_steps") => report.next_steps.push(clean_list_item(trimmed)),
            _ => {}
        }
    }

    if report.summary.is_empty() {
        report.summary = build_public_summary_text(task);
    }
    report.key_points.retain(|item| !item.is_empty());
    report.next_steps.retain(|item| !item.is_empty());
    report
}

fn extract_stdout(raw_output: &str) -> &str {
    if let Some(rest) = raw_output.strip_prefix("STDOUT\n") {
        if let Some((stdout, _)) = rest.split_once("\n\nSTDERR\n") {
            return stdout.trim();
        }
        return rest.trim();
    }
    raw_output.trim()
}

fn is_summary_heading(value: &str) -> bool {
    looks_like_section_heading(value)
        && normalized_section_heading(value).contains("要約")
}

fn is_key_points_heading(value: &str) -> bool {
    looks_like_section_heading(value)
        && normalized_section_heading(value).contains("主要ポイント")
}

fn is_next_steps_heading(value: &str) -> bool {
    looks_like_section_heading(value)
        && normalized_section_heading(value).contains("次に見るべき点")
}

fn looks_like_section_heading(value: &str) -> bool {
    let trimmed = value.trim_start();
    trimmed.starts_with('#')
        || trimmed.starts_with('*')
        || trimmed.chars().next().is_some_and(|c| c.is_ascii_digit())
}

fn normalized_section_heading(value: &str) -> String {
    value
        .trim()
        .trim_matches('*')
        .trim_start_matches('#')
        .trim()
        .trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, '.' | ')' | ' ' | '\t'))
        .trim()
        .trim_matches('*')
        .trim()
        .to_string()
}

fn clean_list_item(value: &str) -> String {
    value
        .trim_start_matches('-')
        .trim_start_matches('\u{30fb}')
        .trim_start_matches('*')
        .trim()
        .to_string()
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect::<String>()
}

fn extract_plain_text(properties: &Value, key: &str) -> String {
    let rich_text = properties
        .get(key)
        .and_then(|value| value.get("rich_text"))
        .and_then(|value| value.as_array());

    if let Some(items) = rich_text {
        let mut combined = String::new();
        for item in items {
            if let Some(content) = item
                .get("plain_text")
                .and_then(|value| value.as_str())
                .or_else(|| {
                    item.get("text")
                        .and_then(|value| value.get("content"))
                        .and_then(|value| value.as_str())
                })
            {
                combined.push_str(content);
            }
        }
        return combined;
    }

    let title = properties
        .get(key)
        .and_then(|value| value.get("title"))
        .and_then(|value| value.as_array());

    if let Some(items) = title {
        let mut combined = String::new();
        for item in items {
            if let Some(content) = item
                .get("plain_text")
                .and_then(|value| value.as_str())
                .or_else(|| {
                    item.get("text")
                        .and_then(|value| value.get("content"))
                        .and_then(|value| value.as_str())
                })
            {
                combined.push_str(content);
            }
        }
        return combined;
    }

    String::new()
}

fn extract_title(properties: &Value, key: &str) -> String {
    extract_plain_text(properties, key)
}

fn extract_date(properties: &Value, key: &str) -> Option<String> {
    properties
        .get(key)
        .and_then(|value| value.get("date"))
        .and_then(|value| value.get("start"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::{build_page_children, build_public_summary_text, display_prompt, parse_report_sections};
    use crate::models::{TaskRecord, TaskType};

    #[test]
    fn parses_structured_report_sections() {
        let mut task = TaskRecord::new(1, 1, 1, "title".into(), "prompt".into(), TaskType::Research);
        task.raw_output = Some(
            "STDOUT\n## 1. 要約\n短い要約です。\n\n## 2. 主要ポイント\n- 一つ目\n- 二つ目\n\n## 3. 次に見るべき点\n- 次A\n- 次B\n\nSTDERR\nignored".into(),
        );
        task.public_summary = Some("公開用の一文。".into());

        let report = parse_report_sections(&task);
        assert_eq!(report.summary, "短い要約です。");
        assert_eq!(report.key_points, vec!["一つ目", "二つ目"]);
        assert_eq!(report.next_steps, vec!["次A", "次B"]);
    }

    #[test]
    fn parses_bold_numbered_headings() {
        let mut task = TaskRecord::new(1, 1, 1, "title".into(), "prompt".into(), TaskType::Research);
        task.raw_output = Some(
            "STDOUT\n**1. 要約**\n最初の要約です。\n\n**2. 主要ポイント**\n* 観点A\n\n**3. 次に見るべき点**\n* 確認A".into(),
        );

        let report = parse_report_sections(&task);
        assert_eq!(report.summary, "最初の要約です。");
        assert_eq!(report.key_points, vec!["観点A"]);
        assert_eq!(report.next_steps, vec!["確認A"]);
    }

    #[test]
    fn falls_back_to_public_summary_when_sections_missing() {
        let mut task = TaskRecord::new(1, 1, 1, "title".into(), "prompt".into(), TaskType::Research);
        task.public_summary = Some("公開用の一文。".into());
        task.raw_output = Some("STDOUT\n自由形式の本文".into());

        let report = parse_report_sections(&task);
        assert_eq!(report.summary, "公開用の一文。");
        assert!(report.key_points.is_empty());
        assert!(report.next_steps.is_empty());
    }

    #[test]
    fn omits_input_data_section_for_discord_tasks() {
        let mut task = TaskRecord::new(1, 1, 1, "title".into(), "prompt".into(), TaskType::Research);
        task.public_summary = Some("公開用の一文。".into());

        let report = parse_report_sections(&task);
        let children = build_page_children(&task, &report);
        let serialized = serde_json::to_string(&children).unwrap();

        assert!(!serialized.contains("入力データ概要"));
        assert!(!serialized.contains("No local input data."));
        assert!(!serialized.contains("STDERR"));
    }

    #[test]
    fn includes_input_data_section_for_cli_tasks() {
        let mut task = TaskRecord::new(0, 0, 0, "title".into(), "prompt".into(), TaskType::Research);
        task.public_summary = Some("公開用の一文。".into());
        task.input_source_path = Some("/tmp/input.json".into());
        task.input_payload = Some("{\"hello\":\"world\"}".into());

        let report = parse_report_sections(&task);
        let children = build_page_children(&task, &report);
        let serialized = serde_json::to_string(&children).unwrap();

        assert!(serialized.contains("入力データ概要"));
    }

    #[test]
    fn keeps_public_summary_short() {
        let mut task = TaskRecord::new(1, 1, 1, "title".into(), "prompt".into(), TaskType::Research);
        task.public_summary = Some("短い一文です。".into());

        assert_eq!(build_public_summary_text(&task), "短い一文です。");
    }

    #[test]
    fn strips_internal_url_section_from_display_prompt() {
        let task = TaskRecord::new(
            1,
            1,
            1,
            "title".into(),
            "原油について教えて\n\nReferenced URLs:\nhttps://example.com".into(),
            TaskType::Research,
        );

        assert_eq!(display_prompt(&task), "原油について教えて");
    }
}