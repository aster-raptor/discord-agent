use std::fs::{self, File, OpenOptions};
use std::io::{self, Stderr, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tracing_subscriber::EnvFilter;

pub fn init_logging(log_file_path: &str) -> Result<()> {
    if let Some(parent) = Path::new(log_file_path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file_path)?;
    let file = Arc::new(Mutex::new(file));

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(move || TeeWriter::new(Arc::clone(&file)))
        .init();

    Ok(())
}

struct TeeWriter {
    stderr: Stderr,
    file: Arc<Mutex<File>>,
}

impl TeeWriter {
    fn new(file: Arc<Mutex<File>>) -> Self {
        Self {
            stderr: io::stderr(),
            file,
        }
    }
}

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stderr.write_all(buf)?;
        let mut file = self.file.lock().unwrap();
        file.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stderr.flush()?;
        let mut file = self.file.lock().unwrap();
        file.flush()
    }
}
