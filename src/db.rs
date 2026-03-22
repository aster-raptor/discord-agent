use std::fs;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{PublicTaskSummary, TaskRecord, TaskStatus, TaskType};

pub struct Database {
    connection: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "failed to create sqlite parent directory: {}",
                        parent.display()
                    )
                })?;
            }
        }

        let connection = Connection::open(path)
            .with_context(|| format!("failed to open sqlite database at {}", path))?;
        let database = Self {
            connection: Mutex::new(connection),
        };
        database.init_schema()?;
        Ok(database)
    }

    fn init_schema(&self) -> Result<()> {
        let connection = self.connection.lock().unwrap();
        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                channel_id TEXT NOT NULL,
                guild_id TEXT NOT NULL,
                requester_id TEXT NOT NULL,
                discord_message_id TEXT NOT NULL,
                title TEXT NOT NULL,
                prompt TEXT NOT NULL,
                task_type TEXT NOT NULL,
                status TEXT NOT NULL,
                publish INTEGER NOT NULL DEFAULT 0,
                public_summary TEXT,
                raw_output TEXT,
                notion_page_id TEXT,
                error_text TEXT,
                started_at TEXT,
                completed_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS task_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                discord_message_id TEXT NOT NULL,
                author_id TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS task_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                status TEXT NOT NULL,
                detail TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS approvals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                approved_by TEXT NOT NULL,
                decision TEXT NOT NULL,
                detail TEXT,
                created_at TEXT NOT NULL
            );
            "#,
        )?;
        Ok(())
    }

    pub fn insert_task(&self, task: &TaskRecord) -> Result<()> {
        let connection = self.connection.lock().unwrap();
        connection.execute(
            r#"
            INSERT INTO tasks (
                id, thread_id, channel_id, guild_id, requester_id, discord_message_id,
                title, prompt, task_type, status, publish, public_summary, raw_output,
                notion_page_id, error_text, started_at, completed_at, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
            "#,
            params![
                &task.id,
                task.thread_id.to_string(),
                task.channel_id.to_string(),
                "",
                "",
                task.discord_message_id.to_string(),
                &task.title,
                &task.prompt,
                task.task_type.as_str(),
                task.status.as_str(),
                bool_to_sql(task.publish),
                &task.public_summary,
                &task.raw_output,
                &task.notion_page_id,
                &task.error_text,
                &task.started_at,
                &task.completed_at,
                &task.created_at,
                &task.updated_at,
            ],
        )?;

        connection.execute(
            r#"
            INSERT INTO task_messages (task_id, discord_message_id, author_id, content, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                &task.id,
                task.discord_message_id.to_string(),
                "",
                &task.prompt,
                &task.created_at
            ],
        )?;

        connection.execute(
            r#"
            INSERT INTO task_events (task_id, status, detail, created_at)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                &task.id,
                task.status.as_str(),
                "task accepted",
                &task.created_at
            ],
        )?;

        Ok(())
    }

    pub fn update_status(
        &self,
        task_id: &str,
        status: TaskStatus,
        detail: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let connection = self.connection.lock().unwrap();
        connection.execute(
            "UPDATE tasks SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.as_str(), now, task_id],
        )?;
        connection.execute(
            "INSERT INTO task_events (task_id, status, detail, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![task_id, status.as_str(), detail.unwrap_or(""), now],
        )?;
        Ok(())
    }

    pub fn mark_running(&self, task_id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let connection = self.connection.lock().unwrap();
        connection.execute(
            "UPDATE tasks SET status = ?1, started_at = ?2, updated_at = ?2 WHERE id = ?3",
            params![TaskStatus::Running.as_str(), now, task_id],
        )?;
        connection.execute(
            "INSERT INTO task_events (task_id, status, detail, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                task_id,
                TaskStatus::Running.as_str(),
                "codex execution started",
                now
            ],
        )?;
        Ok(())
    }

    pub fn mark_summarizing(&self, task_id: &str) -> Result<()> {
        self.update_status(
            task_id,
            TaskStatus::Summarizing,
            Some("publishing task output"),
        )
    }

    pub fn complete_task(
        &self,
        task_id: &str,
        public_summary: &str,
        raw_output: &str,
        notion_page_id: Option<&str>,
        publish: bool,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let connection = self.connection.lock().unwrap();
        connection.execute(
            r#"
            UPDATE tasks
            SET status = ?1,
                public_summary = ?2,
                raw_output = ?3,
                notion_page_id = COALESCE(?4, notion_page_id),
                publish = ?5,
                completed_at = ?6,
                updated_at = ?6
            WHERE id = ?7
            "#,
            params![
                TaskStatus::Completed.as_str(),
                public_summary,
                raw_output,
                notion_page_id,
                bool_to_sql(publish),
                now,
                task_id
            ],
        )?;
        connection.execute(
            "INSERT INTO task_events (task_id, status, detail, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                task_id,
                TaskStatus::Completed.as_str(),
                "task completed",
                now
            ],
        )?;
        Ok(())
    }

    pub fn fail_task(
        &self,
        task_id: &str,
        error_text: &str,
        raw_output: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let connection = self.connection.lock().unwrap();
        connection.execute(
            r#"
            UPDATE tasks
            SET status = ?1,
                error_text = ?2,
                raw_output = COALESCE(?3, raw_output),
                completed_at = ?4,
                updated_at = ?4
            WHERE id = ?5
            "#,
            params![
                TaskStatus::Failed.as_str(),
                error_text,
                raw_output,
                now,
                task_id
            ],
        )?;
        connection.execute(
            "INSERT INTO task_events (task_id, status, detail, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![task_id, TaskStatus::Failed.as_str(), error_text, now],
        )?;
        Ok(())
    }

    pub fn get_task(&self, task_id: &str) -> Result<TaskRecord> {
        let connection = self.connection.lock().unwrap();
        let task = connection
            .query_row(
                r#"
                SELECT id, thread_id, channel_id, discord_message_id, title, prompt,
                       task_type, status, publish, public_summary, raw_output, notion_page_id, error_text,
                       started_at, completed_at, created_at, updated_at
                FROM tasks
                WHERE id = ?1
                "#,
                params![task_id],
                row_to_task,
            )
            .optional()?;

        task.ok_or_else(|| anyhow!("task not found: {}", task_id))
    }

    pub fn list_completed_public_tasks(&self, limit: usize) -> Result<Vec<PublicTaskSummary>> {
        let connection = self.connection.lock().unwrap();
        let mut statement = connection.prepare(
            r#"
            SELECT id, title, COALESCE(public_summary, ''), completed_at, updated_at
            FROM tasks
            WHERE publish = 1 AND status = 'completed'
            ORDER BY updated_at DESC
            LIMIT ?1
            "#,
        )?;

        let rows = statement.query_map(params![limit as i64], |row| {
            Ok(PublicTaskSummary {
                task_id: row.get(0)?,
                title: row.get(1)?,
                summary: row.get(2)?,
                completed_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRecord> {
    Ok(TaskRecord {
        id: row.get(0)?,
        thread_id: row.get::<_, String>(1)?.parse().unwrap_or_default(),
        channel_id: row.get::<_, String>(2)?.parse().unwrap_or_default(),
        discord_message_id: row.get::<_, String>(3)?.parse().unwrap_or_default(),
        title: row.get(4)?,
        prompt: row.get(5)?,
        task_type: TaskType::from_str(&row.get::<_, String>(6)?),
        status: TaskStatus::from_str(&row.get::<_, String>(7)?),
        publish: row.get::<_, i64>(8)? != 0,
        public_summary: row.get(9)?,
        raw_output: row.get(10)?,
        notion_page_id: row.get(11)?,
        error_text: row.get(12)?,
        started_at: row.get(13)?,
        completed_at: row.get(14)?,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
    })
}

fn bool_to_sql(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}
