use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TaskType {
    Research,
    Coding,
}

impl TaskType {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskType::Research => "research",
            TaskType::Coding => "coding",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "coding" => TaskType::Coding,
            _ => TaskType::Research,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TaskStatus {
    Accepted,
    Queued,
    Running,
    AwaitingApproval,
    Summarizing,
    Completed,
    Failed,
    Rejected,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Accepted => "accepted",
            TaskStatus::Queued => "queued",
            TaskStatus::Running => "running",
            TaskStatus::AwaitingApproval => "awaiting_approval",
            TaskStatus::Summarizing => "summarizing",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Rejected => "rejected",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "accepted" => TaskStatus::Accepted,
            "queued" => TaskStatus::Queued,
            "running" => TaskStatus::Running,
            "awaiting_approval" => TaskStatus::AwaitingApproval,
            "summarizing" => TaskStatus::Summarizing,
            "completed" => TaskStatus::Completed,
            "rejected" => TaskStatus::Rejected,
            _ => TaskStatus::Failed,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub thread_id: u64,
    pub channel_id: u64,
    pub discord_message_id: u64,
    pub title: String,
    pub prompt: String,
    pub input_source_path: Option<String>,
    pub input_payload: Option<String>,
    pub task_type: TaskType,
    pub status: TaskStatus,
    pub publish: bool,
    pub public_summary: Option<String>,
    pub raw_output: Option<String>,
    pub notion_page_id: Option<String>,
    pub notion_page_url: Option<String>,
    pub error_text: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl TaskRecord {
    pub fn new(
        thread_id: u64,
        channel_id: u64,
        discord_message_id: u64,
        title: String,
        prompt: String,
        task_type: TaskType,
    ) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            id: Uuid::new_v4().to_string(),
            thread_id,
            channel_id,
            discord_message_id,
            title,
            prompt,
            input_source_path: None,
            input_payload: None,
            task_type,
            status: TaskStatus::Accepted,
            publish: false,
            public_summary: None,
            raw_output: None,
            notion_page_id: None,
            notion_page_url: None,
            error_text: None,
            started_at: None,
            completed_at: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TaskJob {
    pub task_id: String,
}

#[derive(Clone, Debug)]
pub struct PublicTaskSummary {
    pub task_id: String,
    pub title: String,
    pub summary: String,
    pub completed_at: Option<String>,
    pub updated_at: String,
}
