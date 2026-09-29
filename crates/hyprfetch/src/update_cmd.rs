//! `hyprfetch update` — in-app updater CLI.
//!
//! The ONLY update source is the project's own server:
//! `https://istias.tech/hyprfetch/updates/` (override with `--channel`,
//! `[update] channel` or `HYPRFETCH_UPDATE_CHANNEL`; empty disables).
//!
//! `--check` does one fast HTTPS GET of `latest.json` and prints
//! "up to date" or the available version. An install downloads the
//! manifest-listed archive, verifies its sha256, swaps the binary
//! atomically (keeping the old one as `.old` rollback) and restarts the
//! daemon when one is running. GitHub is never contacted.

use anyhow::{bail, Result};

use hyprfetch_core::update::{UpdateCheck, UpdateConfig, UPDATES_PAGE_URL};

use crate::{daemon, helpers, UpdateArgs};

/// Entry point for the `update` subcommand.
pub async fn run(args: UpdateArgs) -> Result<()> {
    crate::logger::init(crate::logger::LogMode::Prod, None)?;

    // Load the config file so `[update]` settings (channel) apply here too.
    let cfg_file = helpers::load_config(args.config.as_deref())?;
    let cfg = helpers::resolve_update_cfg(args.channel.as_deref(), &cfg_file.update);

    let Some(base) = cfg.effective_channel().map(str::to_string) else {
        println!("the updater is disabled ([update] channel = \"\")");
        println!("re-enable it or follow {UPDATES_PAGE_URL} for manual updates");
        return Ok(());
    };

    let chk = match hyprfetch_core::update::check(&cfg).await {
        Ok(c) => c,
        Err(e) => {
            println!("update channel unreachable ({base}): {e}");
            println!();
            println!("this is the only update source — GitHub is never contacted.");
            println!("check {UPDATES_PAGE_URL} for the latest version and manual steps,");
            println!("or run `hyprfetch doctor` to inspect the channel in use.");
            tracing::debug!(error = %e, "channel check failed");
            return Ok(());
        }
    };

    print_report(&chk);
    println!("checked via update channel ({base})");

    if args.check {
        if chk.available {
            println!("→ update available: run `hyprfetch update`");
        } else {
            println!("→ up to date");
        }
        return Ok(());
    }

    if !chk.available {
        println!("already on the latest version — nothing to do");
        return Ok(());
    }

    install(&args, &cfg, &chk).await
}

/// Print the standard check report.
fn print_report(chk: &UpdateCheck) {
    println!("current version : {}", chk.current);
    println!("latest release  : {}", chk.latest);
    match &chk.asset {
        Some(a) => println!("asset           : {} ({} bytes)", a.name, a.size),
        None => println!("asset           : none for this machine's target"),
    }
}

/// Confirmation gate: interactive prompt, or `--yes` / `HYPRFETCH_ASSUME_YES`
/// for non-interactive runs (scripts must opt in).
fn confirm(args: &UpdateArgs, from: &str, to: &str) -> Result<Confirm> {
    let interactive = unsafe { libc::isatty(0) } == 1;
    if interactive && !args.yes {
        print!("install {to} over {from}? [y/N] ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !matches!(line.trim(), "y" | "Y" | "yes") {
            println!("aborted");
            return Ok(Confirm::Abort);
        }
    } else if !args.yes && std::env::var("HYPRFETCH_ASSUME_YES").is_err() {
        // Non-interactive without --yes: refuse, so scripts must opt in.
        bail!("refusing to update non-interactively without --yes");
    }
    Ok(Confirm::Proceed)
}

enum Confirm {
    Proceed,
    Abort,
}

/// Install: download the manifest-listed archive from the server →
/// sha256-verify → atomic swap → restart the daemon when one is running.
async fn install(args: &UpdateArgs, cfg: &UpdateConfig, chk: &UpdateCheck) -> Result<()> {
    let Some(asset) = &chk.asset else {
        bail!(
            "channel release {} has no archive for this machine's target — \
             download a package manually from {UPDATES_PAGE_URL}",
            chk.latest
        );
    };

    match confirm(args, &chk.current, &chk.latest)? {
        Confirm::Abort => return Ok(()),
        Confirm::Proceed => {}
    }

    println!("downloading + verifying {} (update channel)…", asset.name);
    let applied = hyprfetch_core::update::apply(cfg, chk).await?;
    println!(
        "installed {} (sha256 {})",
        applied.installed,
        &applied.sha256[..16]
    );

    if let Some(b) = &applied.backup_path {
        println!("previous binary kept at {b}");
    }
    if daemon::is_running().is_some() {
        println!("restarting daemon to apply…");
        daemon::restart(&[])?;
        println!("daemon restarted with the new version");
    } else {
        println!("restart any running `hyprfetch serve` to apply the new binary");
    }
    Ok(())
}
