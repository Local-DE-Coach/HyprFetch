//! Background-mode commands (v0.4.6): `hyprfetch open` / `hyprfetch close`.
//!
//! The daemon is a server — it never really "closes". These commands give
//! it app-like close/reopen behavior:
//! - `close` puts the running daemon into low-usage background mode
//!   (downloads keep running, own wakeups drop 10×) and tells the user
//!   how to come back.
//! - `open` starts the daemon when needed and opens the web UI in the
//!   default browser.

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

/// POST/GET helper that retries once with the stored API token on 401.
fn http_call(base: &str, path: &str, method: &str, token: Option<&str>) -> Result<(u16, String)> {
    let client = client()?;
    let url = format!("{base}{path}");
    let send = |tok: Option<&str>| -> Result<(u16, String)> {
        let mut req = match method {
            "POST" => client.post(&url),
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

fn human_bytes(n: u64) -> String {
    let v = n as f64;
    if v >= 1024.0 * 1024.0 {
        format!("{:.1} MiB", v / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KiB", v / 1024.0)
    }
}
