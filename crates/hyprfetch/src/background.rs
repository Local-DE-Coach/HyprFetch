//! Background-mode commands (v0.4.6/v0.4.8): `hyprfetch open` / `close` /
//! `add`.
//!
//! The daemon is a server — it never really "closes". These commands give
//! it app-like behavior:
//! - `close` puts the running daemon into low-usage background mode
//!   (downloads keep running, own wakeups drop 10×) and tells the user
//!   how to come back.
//! - `open` starts the daemon when needed and opens the web UI in the
//!   default browser.
//! - `add URL…` starts the daemon when needed and queues downloads —
//!   the terminal/widget entry point (the Quickshell widget calls this).

use anyhow::{bail, Context, Result};
use std::time::Duration;

use crate::daemon;

/// Normalize a bind address for local HTTP calls: an unspecified IP
/// (0.0.0.0 / ::) becomes 127.0.0.1 — the server also listens on loopback.
fn local_base(bind: &str) -> Result<String> {
    let addr: std::net::SocketAddr = bind
        .parse()
        .with_context(|| format!("invalid bind in pid file: {bind}"))?;
    let ip = if addr.ip().is_unspecified() {
        std::net::IpAddr::from([127, 0, 0, 1])
    } else {
        addr.ip()
    };
    Ok(format!("http://{ip}:{}", addr.port()))
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("building http client")
}

/// Server base URL + optional bearer token for a RUNNING daemon.
fn running_base() -> Result<(String, Option<String>)> {
    let entry = daemon::is_running().context("daemon not running")?;
    let base = local_base(&entry.bind)?;
    let token = std::fs::read_to_string(crate::helpers::token_file_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Ok((base, token))
}

/// POST/GET/DELETE helper that retries once with the stored API token on 401.
fn http_call(base: &str, path: &str, method: &str, token: Option<&str>) -> Result<(u16, String)> {
    let client = client()?;
    let url = format!("{base}{path}");
    let send = |tok: Option<&str>| -> Result<(u16, String)> {
        let mut req = match method {
            "POST" => client.post(&url),
            "DELETE" => client.delete(&url),
            _ => client.get(&url),
        };
        if let Some(t) = tok {
            req = req.bearer_auth(t);
        }
        let resp = req.send().context("request to the running server failed")?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        Ok((status, body))
    };
    let (status, body) = send(token)?;
    if status == 401 {
        // Token may have rotated or wasn't loaded — retry from disk.
        let fresh = std::fs::read_to_string(crate::helpers::token_file_path())
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if fresh.is_some() && fresh.as_deref() != token {
            return send(fresh.as_deref());
        }
    }
    Ok((status, body))
}

/// `hyprfetch open` — start the daemon if it isn't running, then open the
/// web UI in the default browser.
pub fn open() -> Result<()> {
    let (base, entry) = match daemon::is_running() {
        Some(entry) => (local_base(&entry.bind)?, Some(entry)),
        None => {
            println!("HyprFetch is not running — starting it in the background…");
            daemon::start(&[])?;
            // daemon::start waits for /healthz; resolve the bind it used.
            let entry = daemon::is_running().context("daemon exited right after start")?;
            (local_base(&entry.bind)?, Some(entry))
        }
    };

    // Sanity: is the server actually answering?
    match http_call(&base, "/healthz", "GET", None) {
        Ok((200, _)) => {}
        Ok((status, _)) => bail!("server answered {status} at {base} — check `hyprfetch logs`"),
        Err(e) => bail!("server not reachable at {base}: {e}"),
    }

    let url = format!("{base}/");
    let status = std::process::Command::new("xdg-open")
        .arg(&url)
        .spawn()
        .context("launching the default browser (xdg-open)")?;
    drop(status);
    println!("HyprFetch UI: {url}");
    if let Some(e) = entry {
        println!("daemon pid {}", e.pid);
    }
    Ok(())
}

/// `hyprfetch close` — put the running daemon into low-usage background
/// mode. Downloads keep running; the process stays resident at its usual
/// few MiB. Reopen any time with `hyprfetch open`.
pub fn close() -> Result<()> {
    let (base, token) = match running_base() {
        Ok(v) => v,
        Err(_) => {
            println!("HyprFetch is not running — nothing to close.");
            return Ok(());
        }
    };

    let (status, body) = http_call(&base, "/api/power/quiet", "POST", token.as_deref())?;
    if status == 200 {
        // Best-effort parse of rss for a friendly line.
        let rss = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("rss_bytes").and_then(|r| r.as_u64()));
        let rss_txt = rss.map(human_bytes).unwrap_or_else(|| "a few".to_string());
        println!("HyprFetch is now in the background (using {rss_txt} of RAM).");
        println!("Downloads keep running. Reopen any time with:  hyprfetch open");
        Ok(())
    } else if status == 401 {
        bail!(
            "not authorized to control this server (non-loopback bind).\n\
             Run the command as the same user that started the daemon, or use the WebUI."
        )
    } else {
        bail!("server answered {status} — try `hyprfetch daemon status`")
    }
}

/// `hyprfetch reveal <task-id>` — open a download's folder in the file
/// manager (the daemon picks a real GUI file manager). Used by the
/// Quickshell widget's recent list.
pub fn reveal(task_id: &str) -> Result<()> {
    let (base, token) = running_base_or_start()?;
    let (status, body_text) = post_json(
        &base,
        &format!("/api/tasks/{task_id}/reveal"),
        &serde_json::json!({}),
        token.as_deref(),
    )?;
    if status == 200 {
        println!("opened the folder for {task_id}");
        Ok(())
    } else if status == 404 {
        bail!("no such download: {task_id}")
    } else {
        bail!(
            "server answered {status}: {}",
            body_text.lines().next().unwrap_or("")
        )
    }
}

/// Daemon base + token, starting the daemon when it isn't running.
fn running_base_or_start() -> Result<(String, Option<String>)> {
    match daemon::is_running() {
        Some(entry) => {
            let token = std::fs::read_to_string(crate::helpers::token_file_path())
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            Ok((local_base(&entry.bind)?, token))
        }
        None => {
            println!("HyprFetch is not running — starting it in the background…");
            daemon::start(&[])?;
            let entry = daemon::is_running().context("daemon exited right after start")?;
            let token = std::fs::read_to_string(crate::helpers::token_file_path())
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            Ok((local_base(&entry.bind)?, token))
        }
    }
}

/// POST a JSON body with bearer-token retry on 401 (same policy as
/// `http_call`, but with a body).
fn post_json(
    base: &str,
    path: &str,
    body: &serde_json::Value,
    token: Option<&str>,
) -> Result<(u16, String)> {
    let client = client()?;
    let url = format!("{base}{path}");
    let send = |tok: Option<&str>| -> Result<(u16, String)> {
        let mut req = client.post(&url).json(body);
        if let Some(t) = tok {
            req = req.bearer_auth(t);
        }
        let resp = req.send().context("request to the running server failed")?;
        let status = resp.status().as_u16();
        let text = resp.text().unwrap_or_default();
        Ok((status, text))
    };
    let (status, text) = send(token)?;
    if status == 401 {
        let fresh = std::fs::read_to_string(crate::helpers::token_file_path())
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if fresh.as_deref() != token {
            return send(fresh.as_deref());
        }
    }
    Ok((status, text))
}

fn human_bytes(n: u64) -> String {
    let v = n as f64;
    if v >= 1024.0 * 1024.0 {
        format!("{:.1} MiB", v / (1024.0 * 1024.0))
    } else if v >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GiB", v / (1024.0 * 1024.0 * 1024.0))
    } else {
        format!("{:.0} KiB", v / 1024.0)
    }
}

/// `hyprfetch remove <task-id> [--file]` — drop a download from the list.
/// The Quickshell widget's delete button runs this; `--file` also deletes
/// the (partially) downloaded file from disk. The daemon removes the task
/// and (when asked) the file; the widget's next status read reflects it.
pub fn remove(task_id: &str, delete_file: bool) -> Result<()> {
    if task_id.trim().is_empty() {
        bail!("no task id — pass the download's id, e.g. t_ab12");
    }
    let (base, token) = running_base_or_start()?;
    let suffix = if delete_file { "?delete_file=true" } else { "" };
    let (status, body_text) = http_call(
        &base,
        &format!("/api/tasks/{task_id}{suffix}"),
        "DELETE",
        token.as_deref(),
    )?;
    if status == 200 {
        if delete_file {
            println!("removed {task_id} and its file");
        } else {
            println!("removed {task_id} from the list (file kept on disk)");
        }
        Ok(())
    } else if status == 404 {
        bail!("no such download: {task_id}")
    } else {
        bail!(
            "server answered {status}: {}",
            body_text.lines().next().unwrap_or("")
        )
    }
}

/// `hyprfetch add URL…` — queue downloads on the daemon (starting it when
/// it isn't running). This is what the Quickshell widget calls; it is
/// also handy from scripts and keybindings.
///
/// `dir` (-d) sends everything into one directory; `output` (-o) pins a
/// single download to an exact path (the widget's confirm-path dialog).
pub fn add(
    urls: &[String],
    dir: Option<&std::path::Path>,
    output: Option<&std::path::Path>,
    quality: Option<&str>,
    audio: bool,
) -> Result<()> {
    if urls.is_empty() {
        bail!("nothing to add — pass one or more http(s) URLs");
    }
    for u in urls {
        let lowered = u.trim().to_lowercase();
        if !(lowered.starts_with("http://") || lowered.starts_with("https://")) {
            bail!("invalid URL `{u}` — hyprfetch add accepts http(s) URLs");
        }
    }

    // Start the daemon when it isn't running (same path as `open`).
    let (base, token) = running_base_or_start()?;

    // Media-engine routing (v0.6.1): --quality / --audio send each URL
    // through /api/media/download — yt-dlp picks the format, the daemon
    // tracks it like any other task. One media request per URL.
    if audio || quality.is_some() {
        let mut queued = 0usize;
        for u in urls {
            let mut body = serde_json::json!({ "url": u, "audio_only": audio });
            if let Some(q) = quality.filter(|_| !audio) {
                body["quality"] = serde_json::json!(q);
            }
            if let Some(d) = dir {
                body["save_dir"] = serde_json::json!(d.to_string_lossy());
            }
            let (status, body_text) =
                post_json(&base, "/api/media/download", &body, token.as_deref())?;
            if status != 200 && status != 201 {
                bail!(
                    "server answered {status}: {}",
                    body_text.lines().next().unwrap_or("")
                );
            }
            let parsed: serde_json::Value =
                serde_json::from_str(&body_text).unwrap_or(serde_json::Value::Null);
            let id = parsed.get("id").and_then(|i| i.as_str()).unwrap_or("?");
            let q = parsed
                .get("media")
                .and_then(|m| m.get("quality"))
                .and_then(|q| q.as_str())
                .unwrap_or("media");
            println!("  → {q}  ({id})");
            queued += 1;
        }
        println!("{queued} media download(s) queued — watch them in the widget or the web UI.");
        return Ok(());
    }

    let mut body = serde_json::json!({ "urls": urls });
    if let Some(d) = dir {
        body["save_dir"] = serde_json::json!(d.to_string_lossy());
    }
    if let Some(o) = output {
        let o = if o.is_absolute() {
            o.to_path_buf()
        } else {
            std::env::current_dir()
                .context("resolving --output against the current directory")?
                .join(o)
        };
        let filename = o
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|n| !n.is_empty() && n != "." && n != "..")
            .context("--output must name a file, not end in / or .");
        let save_dir = o
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .filter(|p| !p.is_empty())
            .context("--output has no parent directory");
        body["save_dir"] = serde_json::json!(save_dir?);
        body["filename"] = serde_json::json!(filename?);
    }
    let (status, body_text) = post_json(&base, "/api/tasks", &body, token.as_deref())?;
    if status != 200 && status != 201 {
        bail!(
            "server answered {status}: {}",
            body_text.lines().next().unwrap_or("")
        );
    }

    // Friendly per-file output: `→ arch.iso  (t_ab12)`.
    let parsed: serde_json::Value =
        serde_json::from_str(&body_text).unwrap_or(serde_json::Value::Null);
    let mut n = 0usize;
    if let Some(tasks) = parsed.get("tasks").and_then(|t| t.as_array()) {
        for t in tasks {
            n += 1;
            let name = t
                .get("filename")
                .and_then(|f| f.as_str())
                .unwrap_or("download");
            let id = t.get("id").and_then(|i| i.as_str()).unwrap_or("?");
            println!("  → {name}  ({id})");
        }
    }
    if n == 0 {
        println!("added {len} download(s)", len = urls.len());
    } else {
        println!("{n} download(s) queued — watch them in the widget or the web UI.");
    }
    Ok(())
}
