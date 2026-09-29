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

use hyprfetch_core::update::{
    can_swap_in_place, find_priv_tool, package_owner, UpdateCheck, UpdateConfig, UPDATES_PAGE_URL,
};

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

/// Tell the user up front what kind of install this is and how the swap
/// will happen, so the sudo password prompt (if any) never comes as a
/// surprise. Runs before the download.
fn explain_swap_strategy() {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    if can_swap_in_place(&exe) {
        return; // user-owned install — the plain atomic swap just works
    }
    match find_priv_tool() {
        Some(tool) => {
            println!(
                "note: {} is a system location — the update will ask for rights via {tool}",
                exe.display()
            );
        }
        None => {
            println!(
                "note: {} is a system location but neither sudo nor doas was found — \
                 the update will fail; install sudo or run it as root",
                exe.display()
            );
        }
    }
    if let Some(owner) = package_owner(&exe) {
        println!(
            "note: this file belongs to pacman package `{owner}` — pacman may \
             list it as modified after the update"
        );
    }
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

    explain_swap_strategy();

    match confirm(args, &chk.current, &chk.latest)? {
        Confirm::Abort => return Ok(()),
        Confirm::Proceed => {}
    }

    println!("downloading + verifying {} (update channel)…", asset.name);
    // The user already consented above, so the updater may escalate via
    // sudo/doas when the binary lives in a system location.
    let applied =
        hyprfetch_core::update::apply(cfg, chk, hyprfetch_core::update::Escalation::Auto).await?;
    println!(
        "installed {} (sha256 {})",
        applied.installed,
        &applied.sha256[..16]
    );
    if applied.escalated {
        println!("system binary replaced via privilege escalation (mode 0755 kept)");
    }

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
