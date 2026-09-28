//! Daemon lifecycle — pm2-style process management for the prod run mode.
//!
//! - `hyprfetch daemon start [serve flags…]` — detach a `hyprfetch serve`
//!   child into its own process group, redirect its output into the rotating
//!   log file, write a JSON PID file, then wait for `/healthz`.
//! - `hyprfetch daemon stop` — SIGTERM, wait, SIGKILL fallback.
//! - `hyprfetch daemon restart [serve flags…]` — stop + start (flags default
//!   to the ones stored in the PID file).
//! - `hyprfetch daemon status` — PID, uptime, live server info.
//! - `hyprfetch logs [-f] [-n N]` — tail the daemon log file.
//!
//! State lives under the state dir: `HYPRFETCH_STATE_DIR` >
//! `$XDG_STATE_HOME/hyprfetch` > `~/.local/state/hyprfetch`.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Contents of the PID file.
#[derive(Debug, Serialize, Deserialize)]
pub struct PidFile {
    pub pid: u32,
    /// The `serve` arguments used at start (reused by `restart`).
    pub args: Vec<String>,
    pub started_at: u64,
    pub bind: String,
}

/// Resolve the state dir (PID file + logs live here).
pub fn state_dir() -> PathBuf {
    if let Ok(d) = std::env::var("HYPRFETCH_STATE_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    let base = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .ok()
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local").join("state"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp").join("hyprfetch-state"));
    base.join("hyprfetch")
}

pub fn pid_file_path() -> PathBuf {
    state_dir().join("hyprfetch.pid")
}

pub fn log_file_path() -> PathBuf {
    state_dir().join("logs").join("hyprfetch.log")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_pid_file() -> Option<PidFile> {
    let raw = std::fs::read_to_string(pid_file_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_pid_file(entry: &PidFile) -> Result<()> {
    let path = pid_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating state dir {}", parent.display()))?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(entry)?)
        .with_context(|| format!("writing pid file {}", path.display()))?;
    Ok(())
}

fn process_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// True when a daemon started by us is currently running.
pub fn is_running() -> Option<PidFile> {
    let entry = read_pid_file()?;
    process_alive(entry.pid).then_some(entry)
}

fn http_get(url: &str, timeout: Duration) -> Result<u16> {
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .context("building http client")?;
    let resp = client.get(url).send().context("request failed")?;
    Ok(resp.status().as_u16())
}

/// Extract `--bind <addr>` (or `HYPRFETCH_BIND`) from a flag list.
fn resolve_bind(flags: &[String]) -> String {
    if let Some(i) = flags.iter().position(|a| a == "--bind") {
        if let Some(v) = flags.get(i + 1) {
            return v.clone();
        }
    }
    if let Ok(v) = std::env::var("HYPRFETCH_BIND") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    "127.0.0.1:7780".to_string()
}

/// Start a detached `hyprfetch serve <flags>` child and wait for readiness.
pub fn start(flags: &[String]) -> Result<()> {
    if let Some(entry) = is_running() {
        bail!(
            "daemon already running (pid {}, logs: `hyprfetch logs -f`)",
            entry.pid
        );
    }
    let exe = std::env::current_exe().context("resolving current exe")?;
    let log_path = log_file_path();
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating log dir {}", parent.display()))?;
    }

    let bind = resolve_bind(flags);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening log file {}", log_path.display()))?;
    let log_err = log.try_clone().context("cloning log file handle")?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("serve")
        .args(flags)
        .env("HYPRFETCH_DAEMON", "1")
        .stdout(log)
        .stderr(log_err)
        .stdin(std::process::Stdio::null());
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // own group: survives terminal close, no SIGINT storms
    }
    let child = cmd
        .spawn()
        .with_context(|| format!("spawning {} serve", exe.display()))?;
    let pid = child.id();
    let started_at = now_secs();

    write_pid_file(&PidFile {
        pid,
        args: flags.to_vec(),
        started_at,
        bind: bind.clone(),
    })?;

    // Wait for /healthz to answer before declaring success.
    let url = format!("http://{bind}/healthz");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if !process_alive(pid) {
            let _ = std::fs::remove_file(pid_file_path());
            bail!("server process {pid} exited during startup — check `hyprfetch logs`");
        }
        match http_get(&url, Duration::from_secs(2)) {
            Ok(s) if (200..300).contains(&s) => break,
            _ if Instant::now() >= deadline => {
                bail!("server did not become healthy at {url} within 20s — check `hyprfetch logs`");
            }
            _ => std::thread::sleep(Duration::from_millis(250)),
        }
    }

    println!("started hyprfetch daemon (pid {pid})");
    println!("  ui       http://{bind}");
    println!("  logs     {}", log_path.display());
    println!("  follow   hyprfetch logs -f");
    println!("  stop     hyprfetch daemon stop");
    Ok(())
}

/// Stop a running daemon. Returns Ok(()) even when nothing was running.
pub fn stop() -> Result<()> {
    let Some(entry) = is_running() else {
        println!("daemon is not running");
        let _ = std::fs::remove_file(pid_file_path()); // stale pid file
        return Ok(());
    };
    let pid = entry.pid as i32;
    println!("stopping pid {pid}…");
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(10);
    while process_alive(entry.pid) {
        if Instant::now() >= deadline {
            println!("graceful stop timed out — sending SIGKILL");
            unsafe { libc::kill(pid, libc::SIGKILL) };
            std::thread::sleep(Duration::from_millis(300));
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    let _ = std::fs::remove_file(pid_file_path());
    println!("stopped");
    Ok(())
}

/// Restart: stop (if running), then start with the given flags — or the
/// stored ones when `flags` is empty.
pub fn restart(flags: &[String]) -> Result<()> {
    if is_running().is_some() {
        stop()?;
        // Give the kernel a beat to release the listen port.
        std::thread::sleep(Duration::from_millis(500));
    }
    let flags: Vec<String> = if flags.is_empty() {
        read_pid_file().map(|e| e.args).unwrap_or_default()
    } else {
        flags.to_vec()
    };
    start(&flags)
}

/// Print a human-readable status line set.
pub fn status() -> Result<()> {
    match is_running() {
        None => {
            println!("daemon: not running");
            return Ok(());
        }
        Some(entry) => {
            let uptime = now_secs().saturating_sub(entry.started_at);
            println!("daemon: running (pid {})", entry.pid);
            println!("  uptime    = {uptime}s");
            println!("  bind      = {}", entry.bind);
            println!("  log file  = {}", log_file_path().display());
        }
    }

    // Live server info (best-effort; non-loopback binds need a token).
    let entry = read_pid_file().expect("checked above");
    let url = format!("http://{}/api/server", entry.bind);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let mut req = client.get(&url);
    // Token is required on non-loopback binds; try the standard token file.
    let token_file = std::env::var("XDG_CONFIG_HOME")
        .map(|p| PathBuf::from(p).join("hyprfetch").join("token"))
        .or_else(|_| {
            std::env::var("HOME").map(|h| PathBuf::from(h).join(".config/hyprfetch/token"))
        })
        .ok();
    if let Some(tf) = token_file {
        if let Ok(t) = std::fs::read_to_string(&tf) {
            let t = t.trim().to_string();
            if !t.is_empty() {
                req = req.bearer_auth(t);
            }
        }
    }
    if let Ok(resp) = req.send() {
        if resp.status().is_success() {
            if let Ok(v) = resp.json::<serde_json::Value>() {
                println!("  version   = {}", v["version"].as_str().unwrap_or("?"));
                println!(
                    "  active    = {} task(s), {} ws client(s)",
                    v["active_tasks"], v["ws_clients"]
                );
                if v["update_available"] == serde_json::json!(true) {
                    println!(
                        "  update    = available (latest {})",
                        v["latest_version"].as_str().unwrap_or("?")
                    );
                }
            }
        }
    }
    Ok(())
}

/// Tail the daemon log: last `lines` entries, optionally following.
pub fn logs(follow: bool, lines: usize) -> Result<()> {
    let path = log_file_path();
    if !path.exists() {
        bail!("no log file at {} — is the daemon running?", path.display());
    }
    let mut file =
        std::fs::File::open(&path).with_context(|| format!("opening {}", path.display()))?;

    // Seek to max(0, len - 256 KiB) and print the last `lines` lines.
    let len = file.metadata()?.len();
    let start = len.saturating_sub(256 * 1024);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    let all: Vec<&str> = buf.lines().collect();
    let skip = all.len().saturating_sub(lines);
    for line in &all[skip..] {
        println!("{line}");
    }

    if !follow {
        return Ok(());
    }

    // Follow mode: poll for appended bytes forever.
    let mut pos = std::fs::metadata(&path)?.len();
    eprintln!("— following {} (Ctrl-C to stop) —", path.display());
    loop {
        std::thread::sleep(Duration::from_millis(400));
        let cur = match std::fs::metadata(&path) {
            Ok(m) => m.len(),
            Err(_) => 0, // rotated away — reopen from the start
        };
        if cur < pos {
            pos = 0;
        }
        if cur > pos {
            if let Ok(mut f) = std::fs::OpenOptions::new().read(true).open(&path) {
                if f.seek(SeekFrom::Start(pos)).is_ok() {
                    let mut chunk = Vec::new();
                    if f.read_to_end(&mut chunk).is_ok() {
                        std::io::stdout().write_all(&chunk).ok();
                        std::io::stdout().flush().ok();
                    }
                }
            }
            pos = cur;
        }
    }
}
