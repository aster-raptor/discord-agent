use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde_json::{json, Value};

use crate::config::AppConfig;
use crate::models::{PublicTaskSummary, TaskRecord};

const NOTION_VERSION: &str = "2022-06-28";

#[derive(Clone)]
pub struct NotionClient {
    client: Client,
    token: Option<String>,
    database_id: Option<String>,
    public_base_url: String,
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

    pub async fn publish_task(&self, task: &TaskRecord) -> Result<Option<String>> {
        if !self.is_enabled() {
            return Ok(None);
        }

        let database_id = self.database_id.as_ref().unwrap();
        let summary = truncate(&build_public_summary(task), 1800);
        let details = truncate(&task.raw_output.clone().unwrap_or_default(), 1800);
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
                "Requester": rich_text_property(&task.requester_id.to_string()),
                "Publish": { "checkbox": true },
                "Public Summary": rich_text_property(&summary),
                "Updated At": { "date": { "start": task.updated_at } },
                "Completed At": { "date": { "start": task.completed_at } },
                "Thread ID": rich_text_property(&task.thread_id.to_string()),
                "Public URL": rich_text_property(&format!("{}/tasks/{}", self.public_base_url, task.id))
            },
            "children": [
                paragraph_block("Task Summary", &summary),
                paragraph_block("Original Prompt", &truncate(&task.prompt, 1800)),
                code_block("Codex Output", &details)
            ]
        });

        let response = self
            .client
            .post("https://api.notion.com/v1/pages")
            .json(&payload)
            .send()
            .await
            .context("failed to call notion create page")?
            .error_for_status()
            .context("notion create page returned error")?;

        let body: Value = response.json().await.context("invalid notion response")?;
        let page_id = body
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("notion response did not include page id"))?;

        Ok(Some(page_id.to_string()))
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
            .context("failed to query notion database")?
            .error_for_status()
            .context("notion query returned error")?;

        let body: Value = response
            .json()
            .await
            .context("invalid notion query response")?;
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

fn paragraph_block(heading: &str, body: &str) -> Value {
    json!({
        "object": "block",
        "type": "paragraph",
        "paragraph": {
            "rich_text": [
                {
                    "type": "text",
                    "text": { "content": truncate(heading, 200) }
                },
                {
                    "type": "text",
                    "text": { "content": format!("\n{}", truncate(body, 1800)) }
                }
            ]
        }
    })
}

fn code_block(language: &str, body: &str) -> Value {
    json!({
        "object": "block",
        "type": "code",
        "code": {
            "language": "plain text",
            "rich_text": [{
                "type": "text",
                "text": { "content": format!("{}\n\n{}", language, truncate(body, 1800)) }
            }]
        }
    })
}

fn build_public_summary(task: &TaskRecord) -> String {
    let output = task.raw_output.clone().unwrap_or_default();
    if output.trim().is_empty() {
        return "No summary available.".to_string();
    }
    truncate(output.trim(), 800)
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
