//! `hyprfetch update` — in-app updater CLI.
//!
//! Binary install path: query the latest GitHub release (PAT-aware for the
//! private repo) → download the matching tarball through the API octet-stream
//! endpoint → sha256-verify → swap the binary atomically → restart the daemon
//! when one is running. Source install path (`--from-git`): `git pull` +
//! `cargo build --release --locked` inside the configured clone, then swap.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::{daemon, helpers, UpdateArgs};
/// Entry point for the `update` subcommand.
pub async fn run(args: UpdateArgs) -> Result<()> {
    crate::logger::init(crate::logger::LogMode::Prod, None)?;

    if args.from_git {
        return run_from_git(&args).await;
    }

    let cfg = helpers::resolve_update_cfg(args.repo.as_deref(), args.token.as_deref());
    if args.repo.is_none() && args.token.is_none() {
        let auth = if cfg.token.is_some() {
            "authenticated (token)"
        } else {
            "unauthenticated (public repos only)"
        };
        println!("checking {} ({auth})…", cfg.repo);
    }

    let Some(chk) = hyprfetch_core::update::check(&cfg)
        .await
        .context("update check failed")?
    else {
        println!("no published release found for {}", cfg.repo);
        return Ok(());
    };

    println!("current version : {}", chk.current);
    println!("latest release  : {}", chk.latest);
    match &chk.asset {
        Some(a) => println!("asset           : {} ({} bytes)", a.name, a.size),
        None => println!("asset           : none for this machine's target"),
    }

    if args.check {
        if chk.available {
            println!("→ update available: run `hyprfetch update` to install");
        } else {
            println!("→ up to date");
        }
        return Ok(());
    }

    if !chk.available {
        println!("already on the latest version — nothing to do");
        return Ok(());
    }
    if chk.asset.is_none() {
        bail!(
            "release {} has no tarball for this target — install manually from {}",
            chk.latest,
            chk.release_url.as_deref().unwrap_or("the releases page")
        );
    }

    // Confirmation gate: interactive prompt, or --yes / non-tty auto-yes.
    let interactive = unsafe { libc::isatty(0) } == 1;
    if interactive && !args.yes {
        print!("install {} over {}? [y/N] ", chk.latest, chk.current);
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !matches!(line.trim(), "y" | "Y" | "yes") {
            println!("aborted");
            return Ok(());
        }
    } else if !args.yes && std::env::var("HYPRFETCH_ASSUME_YES").is_err() {
        // Non-interactive without --yes: refuse, so scripts must opt in.
        bail!("refusing to update non-interactively without --yes");
    }

    println!(
        "downloading + verifying {}…",
        chk.asset.as_ref().unwrap().name
    );
    let applied = hyprfetch_core::update::apply(&cfg, &chk).await?;
    println!(
        "installed {} (sha256 {})",
        applied.installed,
        &applied.sha256[..16]
    );
    if let Some(b) = &applied.backup_path {
        println!("previous binary kept at {b}");
    }

    // Restart the daemon so the new binary takes effect (auto-resume).
    if daemon::is_running().is_some() {
        println!("restarting daemon to apply…");
        daemon::restart(&[])?;
        println!("daemon restarted with the new version");
    } else {
        println!("restart any running `hyprfetch serve` to apply the new binary");
    }
    Ok(())
}

/// `--from-git`: pull + rebuild inside a source clone, then swap.
async fn run_from_git(args: &UpdateArgs) -> Result<()> {
    let source_dir: PathBuf = match args.source_dir.clone() {
        Some(d) => d,
        // Fall back to [update] source_dir from the config file.
        None => helpers::load_config(None)
            .ok()
            .and_then(|c| c.update.and_then(|u| u.source_dir))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "--from-git needs --source-dir <clone> (or [update] source_dir in the config)"
                )
            })?,
    };
    if !source_dir.join("Cargo.toml").exists() {
        bail!(
            "{} does not look like the HyprFetch clone",
            source_dir.display()
        );
    }
    println!("pulling + building in {}…", source_dir.display());
    let built = hyprfetch_core::update::run_git_update(&source_dir)?;
    let bytes =
        std::fs::read(&built).with_context(|| format!("reading freshly built binary {built}"))?;
    let exe = std::env::current_exe().context("resolving current exe")?;
    hyprfetch_core::update::swap_binary(&exe, &bytes)?;
    println!("installed the freshly built binary over {}", exe.display());
    if daemon::is_running().is_some() {
        daemon::restart(&[])?;
        println!("daemon restarted");
    }
    Ok(())
}
