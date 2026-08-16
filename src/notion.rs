use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{error, info, warn};

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

    pub fn missing_configuration(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.token.is_none() {
            missing.push("NOTION_TOKEN");
        }
        if self.database_id.is_none() {
            missing.push("NOTION_TASK_DATABASE_ID");
        }
        missing
    }

    pub async fn publish_task(&self, task: &TaskRecord) -> Result<Option<PublishedPage>> {
        if !self.is_enabled() {
            warn!(
                task_id = %task.id,
                missing = ?self.missing_configuration(),
                "skipping notion publish because notion integration is not configured"
            );
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

fn linked_paragraph_block(label: &str, url: &str) -> Value {
    json!({
        "object": "block",
        "type": "paragraph",
        "paragraph": {
            "rich_text": [{
                "type": "text",
                "text": {
                    "content": truncate(label, 1800),
                    "link": { "url": url }
                }
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
    changes_since_previous: Option<PreviousChanges>,
    key_points: Vec<String>,
    next_steps: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct PreviousChanges {
    baseline_available: Option<bool>,
    items: Vec<String>,
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

    if let Some(changes) = &report.changes_since_previous {
        children.push(heading_block("前回からの変化"));
        if changes.baseline_available == Some(false) {
            children.push(paragraph_block("初回のため比較対象なし"));
        } else if changes.items.is_empty() {
            children.push(paragraph_block("前回からの明示的な変化なし"));
        } else {
            for item in &changes.items {
                children.push(bulleted_list_item_block(item));
            }
        }
    }

    if let Some(url) = task
        .previous_notion_page_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        children.push(heading_block("前回の分析リンク"));
        children.push(linked_paragraph_block("前回の分析を開く", url));
    }

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

    if let Some(report) = parse_json_report_sections(stdout) {
        return finalize_report_sections(task, report);
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
        if is_changes_heading(trimmed) {
            report
                .changes_since_previous
                .get_or_insert_with(PreviousChanges::default);
            current_section = Some("changes_since_previous");
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
            Some("changes_since_previous") => report
                .changes_since_previous
                .get_or_insert_with(PreviousChanges::default)
                .items
                .push(clean_list_item(trimmed)),
            Some("key_points") => report.key_points.push(clean_list_item(trimmed)),
            Some("next_steps") => report.next_steps.push(clean_list_item(trimmed)),
            _ => {}
        }
    }

    finalize_report_sections(task, report)
}

fn finalize_report_sections(task: &TaskRecord, mut report: ReportSections) -> ReportSections {
    if report.summary.is_empty() {
        report.summary = build_public_summary_text(task);
    }
    report.key_points.retain(|item| !item.is_empty());
    report.next_steps.retain(|item| !item.is_empty());
    if let Some(changes) = &mut report.changes_since_previous {
        changes.items.retain(|item| !item.is_empty());
    }
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

fn parse_json_report_sections(stdout: &str) -> Option<ReportSections> {
    let json = extract_first_json_object(stdout)?;
    let mut report = ReportSections::default();

    report.summary = flatten_json_text(json.get("market_summary")).unwrap_or_default();
    report.changes_since_previous = parse_previous_changes(json.get("changes_since_previous"));

    if let Some(services) = json.get("services").and_then(|value| value.as_array()) {
        report.key_points = services
            .iter()
            .filter_map(build_service_key_point)
            .collect();

        let mut actions = build_action_plan_items(json.get("action_plan"));
        if actions.is_empty() {
            actions = services
                .iter()
                .filter_map(build_service_action)
                .collect::<Vec<_>>();
        }
        report.next_steps = actions;
    }

    if report.summary.is_empty()
        && report.changes_since_previous.is_none()
        && report.key_points.is_empty()
        && report.next_steps.is_empty()
    {
        None
    } else {
        Some(report)
    }
}

fn parse_previous_changes(value: Option<&Value>) -> Option<PreviousChanges> {
    let changes = value?.as_object()?;
    let mut result = PreviousChanges {
        baseline_available: changes.get("baseline_available").and_then(Value::as_bool),
        ..PreviousChanges::default()
    };

    for (key, label) in [
        ("new", "新規"),
        ("strengthened", "強まった"),
        ("weakened", "弱まった"),
        ("reversed", "反転"),
        ("resolved", "解消"),
    ] {
        for item in json_text_items(changes.get(key)) {
            result.items.push(format!("{}: {}", label, item));
        }
    }

    Some(result)
}

fn json_text_items(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| flatten_json_text(Some(item)))
            .collect(),
        Some(value) => flatten_json_text(Some(value)).into_iter().collect(),
        None => Vec::new(),
    }
}

fn extract_first_json_object(value: &str) -> Option<Value> {
    let start = value.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, ch) in value[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = start + offset + ch.len_utf8();
                    return serde_json::from_str::<Value>(&value[start..end]).ok();
                }
            }
            _ => {}
        }
    }

    None
}

fn flatten_json_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Array(items) => {
            let texts = items
                .iter()
                .filter_map(|item| flatten_json_text(Some(item)))
                .collect::<Vec<_>>();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join(" / "))
            }
        }
        Value::Object(map) => {
            let texts = map
                .values()
                .filter_map(|item| flatten_json_text(Some(item)))
                .collect::<Vec<_>>();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join(" / "))
            }
        }
        _ => None,
    }
}

fn first_non_empty_string(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(|item| item.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn build_service_key_point(service: &Value) -> Option<String> {
    let name = first_non_empty_string(service, &["normalized_name", "name"])?;
    let mut parts = vec![name];

    if let Some(category) = first_non_empty_string(service, &["category"]) {
        parts.push(format!("カテゴリ: {}", category));
    }
    if let Some(score) = service
        .get("attention_score_0_to_100")
        .and_then(|value| value.as_i64())
    {
        parts.push(format!("注目度: {}/100", score));
    }
    if let Some(sentiment) = first_non_empty_string(service, &["sentiment"]) {
        parts.push(format!("センチメント: {}", sentiment));
    }
    if let Some(reason) = first_non_empty_string(service, &["why_people_care"]) {
        parts.push(format!("理由: {}", reason));
    }
    if let Some(evidence) = build_evidence_summary(service.get("evidence")) {
        parts.push(format!("根拠: {}", evidence));
    }

    Some(parts.join(" / "))
}

fn build_evidence_summary(value: Option<&Value>) -> Option<String> {
    let evidence = value?.as_array()?.iter().find_map(|item| {
        let timestamp = item.get("timestamp").and_then(|value| value.as_str())?.trim();
        let speaker = item.get("speaker").and_then(|value| value.as_str())?.trim();
        let quote = item.get("quote").and_then(|value| value.as_str())?.trim();
        if timestamp.is_empty() || speaker.is_empty() || quote.is_empty() {
            return None;
        }
        Some(format!("{} {}: {}", timestamp, speaker, quote))
    })?;

    Some(evidence)
}

fn build_action_plan_items(value: Option<&Value>) -> Vec<String> {
    let Some(action_plan) = value else {
        return Vec::new();
    };

    let mut items = Vec::new();
    for (key, label) in [
        ("immediate", "今すぐ確認"),
        ("intraday", "今日中に監視"),
        ("swing", "中期で監視"),
    ] {
        match action_plan.get(key) {
            Some(Value::String(text)) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    items.push(format!("{}: {}", label, trimmed));
                }
            }
            Some(Value::Array(values)) => {
                for text in values.iter().filter_map(|value| value.as_str()) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        items.push(format!("{}: {}", label, trimmed));
                    }
                }
            }
            _ => {}
        }
    }

    items
}

fn build_service_action(service: &Value) -> Option<String> {
    let name = first_non_empty_string(service, &["normalized_name", "name"])?;
    match service.get("recommended_actions") {
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(format!("{}: {}", name, trimmed))
            }
        }
        Some(Value::Array(items)) => items.iter().find_map(|value| {
            let text = value.as_str()?.trim();
            if text.is_empty() {
                None
            } else {
                Some(format!("{}: {}", name, text))
            }
        }),
        _ => None,
    }
}

fn is_summary_heading(value: &str) -> bool {
    looks_like_section_heading(value)
        && normalized_section_heading(value).contains("要約")
}

fn is_key_points_heading(value: &str) -> bool {
    looks_like_section_heading(value)
        && normalized_section_heading(value).contains("主要ポイント")
}

fn is_changes_heading(value: &str) -> bool {
    let normalized = normalized_section_heading(value);
    (looks_like_section_heading(value) || normalized == "前回からの変化")
        && normalized.contains("前回からの変化")
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
        assert!(!serialized.contains("前回からの変化"));
        assert!(!serialized.contains("前回の分析"));
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

    #[test]
    fn parses_json_report_sections_for_cli_output() {
        let mut task = TaskRecord::new(1, 1, 1, "title".into(), "prompt".into(), TaskType::Research);
        task.raw_output = Some(
            "STDOUT\n{\n  \"services\": [\n    {\n      \"name\": \"Hyperliquid\",\n      \"normalized_name\": \"Hyperliquid\",\n      \"category\": \"DEX\",\n      \"attention_score_0_to_100\": 92,\n      \"sentiment\": \"強気\",\n      \"why_people_care\": \"出来高の急増が続いている\",\n      \"recommended_actions\": [\"Funding と OI を追う\"],\n      \"evidence\": [\n        {\n          \"timestamp\": \"10:15\",\n          \"speaker\": \"alice\",\n          \"quote\": \"Hyperliquid の出来高がまた増えている\"\n        }\n      ]\n    }\n  ],\n  \"market_summary\": \"市場全体では短期資金の流入期待が優勢。\",\n  \"action_plan\": {\n    \"immediate\": [\"Funding の急変を確認する\"],\n    \"intraday\": \"出来高継続を監視する\",\n    \"swing\": [\"TGE 関連日程を確認する\"]\n  }\n}\n\n人間向け要約\n- Hyperliquid が中心話題".into(),
        );
        task.public_summary = Some("公開用の一文。".into());

        let report = parse_report_sections(&task);

        assert_eq!(report.summary, "市場全体では短期資金の流入期待が優勢。");
        assert_eq!(report.key_points.len(), 1);
        assert!(report.key_points[0].contains("Hyperliquid"));
        assert!(report.key_points[0].contains("注目度: 92/100"));
        assert!(report.key_points[0].contains("根拠: 10:15 alice: Hyperliquid の出来高がまた増えている"));
        assert_eq!(
            report.next_steps,
            vec![
                "今すぐ確認: Funding の急変を確認する",
                "今日中に監視: 出来高継続を監視する",
                "中期で監視: TGE 関連日程を確認する"
            ]
        );
    }

    #[test]
    fn falls_back_when_json_is_invalid() {
        let mut task = TaskRecord::new(1, 1, 1, "title".into(), "prompt".into(), TaskType::Research);
        task.raw_output = Some("STDOUT\n{\"services\": [\n\n## 1. 要約\n短い要約です。".into());
        task.public_summary = Some("公開用の一文。".into());

        let report = parse_report_sections(&task);

        assert_eq!(report.summary, "短い要約です。");
    }

    #[test]
    fn parses_all_previous_change_categories_in_display_order() {
        let mut task = TaskRecord::new(1, 1, 1, "title".into(), "prompt".into(), TaskType::Research);
        task.raw_output = Some(
            r#"STDOUT
{
  "market_summary": "要約",
  "changes_since_previous": {
    "baseline_available": true,
    "resolved": ["懸念が解消"],
    "reversed": ["弱気から強気へ反転"],
    "weakened": ["売り圧力が弱まった"],
    "strengthened": ["出来高増加が強まった"],
    "new": ["新しい上場観測"]
  }
}"#
                .into(),
        );

        let changes = parse_report_sections(&task)
            .changes_since_previous
            .unwrap();

        assert_eq!(changes.baseline_available, Some(true));
        assert_eq!(
            changes.items,
            vec![
                "新規: 新しい上場観測",
                "強まった: 出来高増加が強まった",
                "弱まった: 売り圧力が弱まった",
                "反転: 弱気から強気へ反転",
                "解消: 懸念が解消",
            ]
        );
    }

    #[test]
    fn renders_initial_and_no_change_messages() {
        for (baseline, expected) in [
            (false, "初回のため比較対象なし"),
            (true, "前回からの明示的な変化なし"),
        ] {
            let mut task = TaskRecord::new(
                1,
                1,
                1,
                "title".into(),
                "prompt".into(),
                TaskType::Research,
            );
            task.raw_output = Some(format!(
                "STDOUT\n{{\"market_summary\":\"要約\",\"changes_since_previous\":{{\"baseline_available\":{},\"new\":[],\"strengthened\":[],\"weakened\":[],\"reversed\":[],\"resolved\":[]}}}}",
                baseline
            ));

            let report = parse_report_sections(&task);
            let serialized = serde_json::to_string(&build_page_children(&task, &report)).unwrap();

            assert!(serialized.contains(expected));
        }
    }

    #[test]
    fn falls_back_to_human_previous_changes_when_json_is_invalid() {
        let mut task = TaskRecord::new(1, 1, 1, "title".into(), "prompt".into(), TaskType::Research);
        task.raw_output = Some(
            "STDOUT\n{\"services\": [\n\n## 1. 要約\n短い要約です。\n\n## 2. 前回からの変化\n- 新規: 新しい話題\n- 解消: 古い懸念\n\n## 3. 主要ポイント\n- ポイント"
                .into(),
        );

        let report = parse_report_sections(&task);
        let changes = report.changes_since_previous.unwrap();

        assert_eq!(changes.items, vec!["新規: 新しい話題", "解消: 古い懸念"]);
        assert_eq!(report.key_points, vec!["ポイント"]);
    }

    #[test]
    fn renders_link_to_previous_notion_page_after_changes() {
        let mut task = TaskRecord::new(1, 1, 1, "title".into(), "prompt".into(), TaskType::Research);
        task.raw_output = Some(
            "STDOUT\n{\"market_summary\":\"要約\",\"changes_since_previous\":{\"baseline_available\":true,\"new\":[\"新しい話題\"]}}"
                .into(),
        );
        task.previous_task_id = Some("previous-task".into());
        task.previous_notion_page_url = Some("https://www.notion.so/previous-page".into());

        let report = parse_report_sections(&task);
        let serialized = serde_json::to_string(&build_page_children(&task, &report)).unwrap();

        let changes_position = serialized.find("前回からの変化").unwrap();
        let link_position = serialized.find("前回の分析を開く").unwrap();
        let key_points_position = serialized.find("依頼内容").unwrap();
        assert!(changes_position < link_position);
        assert!(link_position < key_points_position);
        assert!(serialized.contains("https://www.notion.so/previous-page"));
        assert!(serialized.contains("\"link\":{\"url\""));
    }
}
