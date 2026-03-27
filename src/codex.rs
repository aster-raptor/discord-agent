use std::fs;
use std::process::Stdio;

use anyhow::{anyhow, Context, Result};
use tokio::process::Command;

use crate::config::AppConfig;
use crate::models::TaskRecord;

#[derive(Clone, Debug)]
pub struct CodexOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

#[derive(Clone)]
pub struct CodexRunner {
    config: AppConfig,
}

impl CodexRunner {
    pub fn new(config: AppConfig) -> Self {
        Self { config }
    }

    pub async fn run_research(&self, task: &TaskRecord) -> Result<CodexOutput> {
        let prompt = self.build_prompt(task)?;

        let mut command = Command::new(&self.config.codex_bin);
        command.arg("exec");
        command.arg("--skip-git-repo-check");
        if let Some(model) = &self.config.codex_model {
            command.arg("--model").arg(model);
        }
        command.arg(prompt);
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let output = command
            .output()
            .await
            .with_context(|| format!("failed to spawn {}", self.config.codex_bin))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if stdout.trim().is_empty() && stderr.trim().is_empty() {
            return Err(anyhow!("codex returned no output"));
        }

        Ok(CodexOutput {
            stdout,
            stderr,
            success: output.status.success(),
        })
    }

    fn build_prompt(&self, task: &TaskRecord) -> Result<String> {
        let template = fs::read_to_string(&self.config.research_prompt_path).with_context(|| {
            format!(
                "failed to read research prompt template: {}",
                self.config.research_prompt_path
            )
        })?;
        Ok(render_prompt_template(&template, task))
    }
}

fn render_prompt_template(template: &str, task: &TaskRecord) -> String {
    let local_input_section = build_local_input_section(task);

    template
        .replace("{task_type}", task.task_type.as_str())
        .replace("{user_request}", &task.prompt)
        .replace("{local_input_path}", task.input_source_path.as_deref().unwrap_or(""))
        .replace("{local_input_data}", task.input_payload.as_deref().unwrap_or(""))
        .replace("{local_input_section}", &local_input_section)
}

fn build_local_input_section(task: &TaskRecord) -> String {
    match (&task.input_source_path, &task.input_payload) {
        (Some(path), Some(payload)) => format!(
            "Local input path:\n{}\n\nLocal input data:\n{}",
            path, payload
        ),
        (Some(path), None) => format!("Local input path:\n{}", path),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::env;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::render_prompt_template;
    use super::CodexRunner;
    use crate::config::AppConfig;
    use crate::models::{TaskRecord, TaskType};

    #[test]
    fn renders_prompt_template_with_local_input() {
        let mut task = TaskRecord::new(1, 1, 1, "title".into(), "prompt".into(), TaskType::Research);
        task.input_source_path = Some("/tmp/input.json".into());
        task.input_payload = Some("{\"hello\":\"world\"}".into());

        let prompt = render_prompt_template(
            "Type: {task_type}\nRequest: {user_request}\n{local_input_section}",
            &task,
        );

        assert!(prompt.contains("Type: research"));
        assert!(prompt.contains("Request: prompt"));
        assert!(prompt.contains("Local input path:\n/tmp/input.json"));
        assert!(prompt.contains("Local input data:\n{\"hello\":\"world\"}"));
    }

    #[test]
    fn renders_prompt_template_without_local_input() {
        let task = TaskRecord::new(1, 1, 1, "title".into(), "prompt".into(), TaskType::Research);

        let prompt = render_prompt_template(
            "Request: {user_request}\n{local_input_section}",
            &task,
        );

        assert!(prompt.contains("Request: prompt"));
        assert!(!prompt.contains("Local input path:"));
    }

    #[test]
    fn loads_prompt_template_from_configured_path() {
        let mut path = env::temp_dir();
        path.push(format!(
            "discord-agent-prompt-{}.txt",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, "Request: {user_request}\nType: {task_type}").unwrap();

        let config = sample_config(path.clone());
        let runner = CodexRunner::new(config);
        let task = TaskRecord::new(1, 1, 1, "title".into(), "prompt".into(), TaskType::Research);

        let prompt = runner.build_prompt(&task).unwrap();
        assert!(prompt.contains("Request: prompt"));
        assert!(prompt.contains("Type: research"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn returns_error_when_prompt_template_is_missing() {
        let path = PathBuf::from("C:/nonexistent/discord-agent/research-prompt.txt");
        let config = sample_config(path.clone());
        let runner = CodexRunner::new(config);
        let task = TaskRecord::new(1, 1, 1, "title".into(), "prompt".into(), TaskType::Research);

        let error = runner.build_prompt(&task).unwrap_err().to_string();
        assert!(error.contains("failed to read research prompt template"));
        assert!(error.contains(path.to_string_lossy().as_ref()));
    }

    fn sample_config(research_prompt_path: PathBuf) -> AppConfig {
        AppConfig {
            discord_token: "token".into(),
            discord_allowed_channel_ids: vec![1],
            sqlite_path: "data/test.sqlite3".into(),
            log_file_path: "logs/test.log".into(),
            research_prompt_path: research_prompt_path.to_string_lossy().into_owned(),
            codex_bin: "codex".into(),
            codex_model: None,
            worker_concurrency: 1,
            notion_token: None,
            notion_task_database_id: None,
            public_base_url: "http://localhost:3000".into(),
        }
    }
}
