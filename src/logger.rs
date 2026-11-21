use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Mutex;

struct DualWriter {
    file: Option<Mutex<File>>,
}

impl Write for DualWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write(buf);
        if let Some(ref file_mutex) = self.file {
            if let Ok(mut f) = file_mutex.lock() {
                let _ = f.write_all(buf);
                let _ = f.flush();
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        if let Some(ref file_mutex) = self.file {
            if let Ok(mut f) = file_mutex.lock() {
                let _ = f.flush();
            }
        }
        Ok(())
    }
}

/// Returns the path to the application's debug log file.
pub fn get_log_file_path() -> PathBuf {
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        PathBuf::from(local_appdata).join("teams-rust-lite").join("teams-debug.log")
    } else {
        std::env::temp_dir().join("teams-rust-lite-debug.log")
    }
}

/// Initializes dual logging to stderr and the persistent log file.
pub fn init() -> Result<PathBuf> {
    let log_path = get_log_file_path();

    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // Check size; truncate if larger than 10MB
    if let Ok(metadata) = fs::metadata(&log_path) {
        if metadata.len() > 10 * 1024 * 1024 {
            let _ = fs::remove_file(&log_path);
        }
    }

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    let writer = DualWriter {
        file: file.map(Mutex::new),
    };

    let env = env_logger::Env::default().default_filter_or("info");
    env_logger::Builder::from_env(env)
        .target(env_logger::Target::Pipe(Box::new(writer)))
        .format(|buf, record| {
            let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            writeln!(
                buf,
                "[{}] [{:5}] [{}] {}",
                ts,
                record.level(),
                record.target(),
                record.args()
            )
        })
        .init();

    log::info!("Logger initialized. Log file: {:?}", log_path);
    Ok(log_path)
}

/// Opens the debug log file using default system viewer (Notepad).
pub fn open_log_file() -> Result<()> {
    let log_path = get_log_file_path();
    if !log_path.exists() {
        if let Some(parent) = log_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = File::create(&log_path);
    }

    open::that_detached(&log_path).context("Failed to open log file with system default editor")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_file_path_not_empty() {
        let path = get_log_file_path();
        assert!(path.to_string_lossy().contains("teams-debug.log") || path.to_string_lossy().contains("teams-rust-lite"));
    }
}
