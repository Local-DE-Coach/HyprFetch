//! Tracing logger setup for the three run modes.
//!
//! - **dev** — pretty console format, `debug` default level, colors on.
//! - **prod** — compact console format, `info` default level.
//! - **daemon** — same compact format but written to a size-rotated log file
//!   under the state dir (`logs/hyprfetch.log`, 5 MiB × 3 rotations).
//!
//! `RUST_LOG` always wins over the built-in default filter.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tracing_subscriber::fmt::MakeWriter;

/// Maximum size of one log file before rotation kicks in.
const ROTATE_BYTES: u64 = 5 * 1024 * 1024;
/// How many rotated files to keep (`hyprfetch.log.1` .. `.3`).
const ROTATE_KEEP: u32 = 3;

/// A size-rotating append-only log file.
struct RotatingLog {
    path: PathBuf,
    file: Option<std::fs::File>,
}

impl RotatingLog {
    fn new(path: PathBuf) -> Self {
        Self { path, file: None }
    }

    fn rotate_if_needed(&mut self) -> std::io::Result<&mut std::fs::File> {
        let needs_rotate = match self.file.as_ref() {
            Some(f) => f
                .metadata()
                .map(|m| m.len() >= ROTATE_BYTES)
                .unwrap_or(false),
            None => {
                self.path.exists()
                    && std::fs::metadata(&self.path)
                        .map(|m| m.len() >= ROTATE_BYTES)
                        .unwrap_or(false)
            }
        };
        if needs_rotate {
            self.file = None;
            // Shift .3 → deleted, .2 → .3, .1 → .2, current → .1
            for i in (1..ROTATE_KEEP).rev() {
                let from = self.path.with_extension(format!("log.{i}"));
                let to = self.path.with_extension(format!("log.{}", i + 1));
                if from.exists() {
                    let _ = std::fs::rename(&from, &to);
                }
            }
            let first = self.path.with_extension("log.1");
            let _ = std::fs::rename(&self.path, &first);
        }
        if self.file.is_none() {
            self.file = Some(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
        }
        Ok(self.file.as_mut().expect("file opened"))
    }
}

impl Write for RotatingLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.rotate_if_needed()?.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

/// `MakeWriter` adapter over a shared `RotatingLog`.
struct RotatingWriter(Arc<Mutex<RotatingLog>>);

struct WriterGuard<'a>(std::sync::MutexGuard<'a, RotatingLog>);

impl Write for WriterGuard<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl<'a> MakeWriter<'a> for RotatingWriter {
    type Writer = WriterGuard<'a>;
    fn make_writer(&'a self) -> Self::Writer {
        WriterGuard(self.0.lock().expect("log lock poisoned"))
    }
}

/// Which console format to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogMode {
    /// Verbose pretty console output (`hyprfetch dev`).
    Dev,
    /// Compact console output (`hyprfetch serve`).
    Prod,
}

/// Initialize the global tracing subscriber.
///
/// `file` redirects output into a rotating log file (daemon mode). Returns
/// after installing the subscriber; calling twice is a no-op.
pub fn init(mode: LogMode, file: Option<&Path>) -> Result<()> {
    let default_filter = match mode {
        LogMode::Dev => "hyprfetch=debug,hyprfetch_core=debug,hyprfetch_api=debug",
        LogMode::Prod => "hyprfetch=info,hyprfetch_core=info,hyprfetch_api=info",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));

    match file {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating log dir {}", parent.display()))?;
            }
            let writer = RotatingWriter(Arc::new(Mutex::new(RotatingLog::new(path.to_path_buf()))));
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(writer)
                .with_ansi(false)
                .compact()
                .try_init()
                .map_err(|e| anyhow::anyhow!("logger already initialised: {e}"))?;
        }
        None => {
            let builder = tracing_subscriber::fmt().with_env_filter(filter);
            match mode {
                LogMode::Dev => builder
                    .pretty()
                    .try_init()
                    .map_err(|e| anyhow::anyhow!("logger already initialised: {e}"))?,
                LogMode::Prod => builder
                    .compact()
                    .try_init()
                    .map_err(|e| anyhow::anyhow!("logger already initialised: {e}"))?,
            };
        }
    }
    Ok(())
}
