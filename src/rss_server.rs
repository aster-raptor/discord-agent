use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::Path;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};

use crate::config::AppConfig;
use crate::models::PublicTaskSummary;
use crate::notion::NotionClient;

#[derive(Clone)]
struct RssState {
    notion: NotionClient,
    public_base_url: String,
}

pub async fn run(config: AppConfig) -> Result<()> {
    config.validate_for_rss()?;
    let state = Arc::new(RssState {
        notion: NotionClient::new(&config)?,
        public_base_url: config.public_base_url.clone(),
    });

    let router = Router::new()
        .route("/healthz", get(healthz))
        .route("/rss.xml", get(rss_feed))
        .route("/tasks/:task_id", get(public_task))
        .layer(Extension(state));

    let address: SocketAddr = config
        .rss_bind_addr
        .parse()
        .with_context(|| format!("invalid RSS_BIND_ADDR: {}", config.rss_bind_addr))?;

    axum::Server::bind(&address)
        .serve(router.into_make_service())
        .await
        .context("rss server exited unexpectedly")?;

    Ok(())
}

async fn healthz() -> &'static str {
    "ok"
}

async fn rss_feed(Extension(state): Extension<Arc<RssState>>) -> Response {
    match state.notion.query_published_tasks(50).await {
        Ok(tasks) => {
            let xml = render_rss(&state.public_base_url, &tasks);
            let mut response = xml.into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/rss+xml; charset=utf-8"),
            );
            response
        }
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            format!("failed to query notion: {}", error),
        )
            .into_response(),
    }
}

async fn public_task(
    Path(task_id): Path<String>,
    Extension(state): Extension<Arc<RssState>>,
) -> Response {
    match state.notion.fetch_public_task(&task_id).await {
        Ok(Some(task)) => Html(render_task_page(&task)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "task not found").into_response(),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            format!("failed to fetch task: {}", error),
        )
            .into_response(),
    }
}

fn render_rss(base_url: &str, tasks: &[PublicTaskSummary]) -> String {
    let mut items = String::new();
    for task in tasks {
        let link = format!("{}/tasks/{}", base_url, task.task_id);
        items.push_str(&format!(
            "<item><title>{}</title><link>{}</link><guid>{}</guid><description>{}</description><pubDate>{}</pubDate></item>",
            xml_escape(&task.title),
            xml_escape(&link),
            xml_escape(&task.task_id),
            xml_escape(&task.summary),
            xml_escape(task.completed_at.as_deref().unwrap_or(&task.updated_at))
        ));
    }

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <rss version=\"2.0\">\
         <channel>\
         <title>discord-agent reports</title>\
         <link>{}</link>\
         <description>Public summaries generated from private Notion task reports.</description>\
         {}\
         </channel>\
         </rss>",
        xml_escape(base_url),
        items
    )
}

fn render_task_page(task: &PublicTaskSummary) -> String {
    format!(
        "<!DOCTYPE html><html lang=\"ja\"><head><meta charset=\"utf-8\"><title>{}</title></head>\
         <body><main><h1>{}</h1><p><strong>Task ID:</strong> {}</p><p><strong>Updated:</strong> {}</p><article><pre style=\"white-space: pre-wrap;\">{}</pre></article></main></body></html>",
        html_escape(&task.title),
        html_escape(&task.title),
        html_escape(&task.task_id),
        html_escape(task.completed_at.as_deref().unwrap_or(&task.updated_at)),
        html_escape(&task.summary)
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn html_escape(value: &str) -> String {
    xml_escape(value)
}
