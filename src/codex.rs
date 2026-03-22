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
        let mut command = Command::new(&self.config.codex_bin);
        command.arg("exec");
        if let Some(model) = &self.config.codex_model {
            command.arg("--model").arg(model);
        }
        command.arg(build_prompt(task));
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
}

fn build_prompt(task: &TaskRecord) -> String {
    format!(
        "You are handling a Discord task.\n\
         Task type: {}\n\
         Return a concise Japanese report with these sections:\n\
         1. 要約\n\
         2. 主要ポイント\n\
         3. 次に見るべき点\n\
         Keep it readable in Notion and safe for a public summary.\n\n\
         User request:\n{}",
        task.task_type.as_str(),
        task.prompt
    )
}
