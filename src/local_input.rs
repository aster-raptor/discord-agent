use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

const SUPPORTED_EXTENSIONS: &[&str] = &["txt", "md", "json", "csv"];

#[derive(Clone, Debug)]
pub struct LoadedInput {
    pub source_path: String,
    pub payload: String,
}

pub fn load_from_path(path: &str) -> Result<LoadedInput> {
    let input_path = Path::new(path);
    if input_path.is_file() {
        return Ok(LoadedInput {
            source_path: input_path.display().to_string(),
            payload: load_file(input_path)?,
        });
    }

    if input_path.is_dir() {
        let mut files = Vec::new();
        for entry in fs::read_dir(input_path)
            .with_context(|| format!("failed to read input directory {}", input_path.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && is_supported_file(&path) {
                files.push(path);
            }
        }
        files.sort();

        if files.is_empty() {
            return Err(anyhow!(
                "input directory does not contain supported files: {}",
                input_path.display()
            ));
        }

        let mut sections = Vec::new();
        for file in files {
            let content = load_file(&file)?;
            sections.push(format!("File: {}\n{}", file.display(), content));
        }

        return Ok(LoadedInput {
            source_path: input_path.display().to_string(),
            payload: sections.join("\n\n---\n\n"),
        });
    }

    Err(anyhow!("input path not found: {}", input_path.display()))
}

pub fn build_input_summary(source_path: &str, payload: &str) -> String {
    let char_count = payload.chars().count();
    let line_count = payload.lines().count();
    format!(
        "Source Path: {}\nCharacters: {}\nLines: {}",
        source_path, char_count, line_count
    )
}

fn load_file(path: &Path) -> Result<String> {
    if !is_supported_file(path) {
        return Err(anyhow!(
            "unsupported input file type: {}",
            path.display()
        ));
    }

    fs::read_to_string(path)
        .with_context(|| format!("failed to read input file {}", path.display()))
}

fn is_supported_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| SUPPORTED_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}
