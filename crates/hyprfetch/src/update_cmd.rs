//! `hyprfetch update` — in-app updater CLI.
//!
//! Three access tiers, tried in order:
//!
//! 0. **Update channel** (default: `https://istias.tech/hyprfetch/updates/`)
//!    — CI mirrors every release's tarballs + a `latest.json` manifest to
//!    the project's own server. One fast HTTPS GET checks for updates with
//!    NO GitHub involvement: no rate limits, no tokens, works for private
//!    repos. Install = download → sha256-verify (manifest hash) → atomic
//!    swap → restart.
//! 1. **GitHub REST API** with a token (env / config / settings DB /
//!    clone-origin PAT / `gh auth token` / git credential helpers) —
//!    downloads the prebuilt release tarball through the octet-stream
//!    endpoint, sha256-verifies it, swaps the binary atomically and
//!    restarts the daemon.
//! 2. **Plain git** (SSH keys, credential helpers, local clone remotes) —
//!    resolves the newest version tag via `ls-remote`, shallow-clones it,
//!    builds with `cargo build --release --locked`, then swaps + restarts.
//!    This is what makes `hyprfetch update` work on PRIVATE repos without
//!    any token: if `git pull` works for the user, the updater works too.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use hyprfetch_core::update::{UpdateCheck, UpdateConfig};

use crate::{daemon, helpers, UpdateArgs};

/// Which tier answered the update check — decides the install path.
enum Tier {
    /// Self-hosted mirror: download the manifest-listed tarball.
    Channel,
    /// GitHub release: download through the API octet-stream endpoint.
    Api,
    /// Plain git: shallow-clone the tag and build from source.
    /// Carries `(remote_url, tag)`.
    Git(String, String),
}

/// Outcome of the interactive confirmation gate.
enum Confirm {
    Proceed,
    Abort,
}

/// Entry point for the `update` subcommand.
pub async fn run(args: UpdateArgs) -> Result<()> {
    crate::logger::init(crate::logger::LogMode::Prod, None)?;

    if args.from_git {
        return run_from_git(&args).await;
    }

    // Load the config file so `[update]` settings (channel / token /
    // git_url / source_dir / repo) apply to the CLI updater too.
    let cfg_file = helpers::load_config(args.config.as_deref())?;
    let cfg = helpers::resolve_update_cfg_with_db(
        args.channel.as_deref(),
        args.repo.as_deref(),
        args.token.as_deref(),
        &cfg_file.update,
        None,
    );

    // ---- Tier 0: self-hosted update channel (fastest, no GitHub) ---------
    if let Some(base) = cfg.effective_channel().map(str::to_string) {
        match hyprfetch_core::update::check_via_channel(&cfg).await {
            Ok(Some(chk)) => {
                println!("checked via update channel ({base})");
                return finish_check(&args, &cfg, chk, Tier::Channel).await;
            }
            Ok(None) | Err(_) => {
                tracing::debug!("update channel tier failed; falling back");
                println!("update channel unreachable ({base}) — trying GitHub…");
            }
        }
    }

    if args.repo.is_none() {
        let auth = match cfg.token_source {
            Some(src) => format!("authenticated ({src})"),
            None => "unauthenticated (public repos + git/SSH access)".to_string(),
        };
        println!("checking {} ({auth})…", cfg.repo);
    }

    // ---- Tier 1: REST API (token or public repo) -------------------------
    // ---- Tier 2: plain git (private repos with SSH/clone access) --------
    // `git` carries (remote_url, tag) of the working git-tier remote.
    let (chk, tier) = match hyprfetch_core::update::check(&cfg).await {
        Ok(Some(c)) => (Some(c), Tier::Api),
        // 404 (private repo without a token / no release) or API trouble:
        // fall back to the git tier before giving up.
        Ok(None) | Err(_) => match hyprfetch_core::update::check_via_git(&cfg).await {
            Ok(Some(g)) => {
                println!("git access detected via {}", g.via_url);
                (Some(g.check), Tier::Git(g.via_url, g.tag))
            }
            Ok(None) => {
                println!("no published release found for {}", cfg.repo);
                return Ok(());
            }
            Err(ge) => {
                println!("no published release found for {}", cfg.repo);
                println!();
                println!("no GitHub access could be established for this repo.");
                println!("Any ONE of these unlocks updates:");
                println!(
                    "  export HYPRFETCH_GITHUB_TOKEN=<PAT with access to {}>",
                    cfg.repo
                );
                println!("  gh auth login            # github CLI login");
                println!(
                    "  git clone git@github.com:{}.git   # SSH key with access",
                    cfg.repo
                );
                println!("  # or [update] token = \"…\" / git_url = \"…\" in the config file");
                tracing::debug!(error = %ge, "git tier also failed");
                return Ok(());
            }
        },
    };

    // Both surviving arms always carry a check result (the None/Err arms
    // return early), so unwrap once here.
    let Some(chk) = chk else {
        unreachable!("tier fallbacks return early when no result exists");
    };
    finish_check(&args, &cfg, chk, tier).await
}

/// Shared tail of the check: print the report, then either stop (`--check`)
/// or dispatch the install to the tier that produced the result.
async fn finish_check(
    args: &UpdateArgs,
    cfg: &UpdateConfig,
    chk: UpdateCheck,
    tier: Tier,
) -> Result<()> {
    println!("current version : {}", chk.current);
    println!("latest release  : {}", chk.latest);
    match (&chk.asset, &tier) {
        (Some(a), _) => println!("asset           : {} ({} bytes)", a.name, a.size),
        (None, Tier::Git(url, _)) => {
            println!("install method  : build from source via git ({url})")
        }
        (None, _) => println!("asset           : none for this machine's target"),
    }

    if args.check {
        if chk.available {
            let how = match &tier {
                Tier::Channel => " (fast download from the update channel)",
                Tier::Api => "",
                Tier::Git(..) => " (builds from source via git — no token needed)",
            };
            println!("→ update available: run `hyprfetch update`{how}");
        } else {
            println!("→ up to date");
        }
        return Ok(());
    }

    if !chk.available {
        println!("already on the latest version — nothing to do");
        return Ok(());
    }

    // ---- Install ---------------------------------------------------------
    match tier {
        Tier::Channel => install_from_channel(args, cfg, &chk).await,
        Tier::Api => install_from_release(args, cfg, &chk).await,
        Tier::Git(url, tag) => install_from_git(args, &url, &tag, &chk).await,
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

/// Print the post-install status + restart the daemon when one is running.
fn finish_install(backup: Option<&str>) -> Result<()> {
    if let Some(b) = backup {
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

/// Channel-tier install: download the manifest-listed tarball from the
/// project mirror → sha256-verify → atomic swap → restart.
async fn install_from_channel(
    args: &UpdateArgs,
    cfg: &UpdateConfig,
    chk: &UpdateCheck,
) -> Result<()> {
    if chk.asset.is_none() {
        bail!(
            "channel release {} has no tarball for this target — run \
             `hyprfetch update --from-git` or install manually from {}",
            chk.latest,
            chk.release_url.as_deref().unwrap_or("the releases page")
        );
    }

    match confirm(args, &chk.current, &chk.latest)? {
        Confirm::Abort => return Ok(()),
        Confirm::Proceed => {}
    }

    println!(
        "downloading + verifying {} (update channel)…",
        chk.asset.as_ref().unwrap().name
    );
    let applied = hyprfetch_core::update::apply_channel(cfg, chk).await?;
    println!(
        "installed {} (sha256 {})",
        applied.installed,
        &applied.sha256[..16]
    );
    finish_install(applied.backup_path.as_deref())
}

/// API-tier install: download → sha256 verify → atomic swap → restart.
async fn install_from_release(
    args: &UpdateArgs,
    cfg: &UpdateConfig,
    chk: &UpdateCheck,
) -> Result<()> {
    if chk.asset.is_none() {
        bail!(
            "release {} has no tarball for this target — install manually from {} \
             or run `hyprfetch update --from-git`",
            chk.latest,
            chk.release_url.as_deref().unwrap_or("the releases page")
        );
    }

    match confirm(args, &chk.current, &chk.latest)? {
        Confirm::Abort => return Ok(()),
        Confirm::Proceed => {}
    }

    println!(
        "downloading + verifying {}…",
        chk.asset.as_ref().unwrap().name
    );
    let applied = hyprfetch_core::update::apply(cfg, chk).await?;
    println!(
        "installed {} (sha256 {})",
        applied.installed,
        &applied.sha256[..16]
    );
    finish_install(applied.backup_path.as_deref())
}

/// Git-tier install: shallow-clone the release tag into a temp dir, build
/// with the user's own toolchain, swap the binary, restart the daemon.
async fn install_from_git(
    args: &UpdateArgs,
    via_url: &str,
    tag: &str,
    chk: &UpdateCheck,
) -> Result<()> {
    match confirm(args, &chk.current, &chk.latest)? {
        Confirm::Abort => return Ok(()),
        Confirm::Proceed => {}
    }

    let dest = std::env::temp_dir().join(format!("hyprfetch-update-{}", std::process::id()));
    println!(
        "building {tag} from source (shallow clone into {})…",
        dest.display()
    );
    let built = hyprfetch_core::update::build_from_tag(via_url, tag, &dest)
        .with_context(|| format!("building {tag} from source via git"))?;
    let bytes = std::fs::read(&built)
        .with_context(|| format!("reading freshly built binary {}", built.display()))?;

    let exe = std::env::current_exe().context("resolving current exe")?;
    hyprfetch_core::update::swap_binary(&exe, &bytes)
        .with_context(|| format!("swapping {} (permission denied? try sudo)", exe.display()))?;
    let _ = std::fs::remove_dir_all(&dest);

    println!(
        "installed {} (built from source, swapped over {})",
        chk.latest,
        exe.display()
    );
    finish_install(Some(&exe.with_extension("old").to_string_lossy()))
}

/// `--from-git`: pull + rebuild inside an existing source clone, then swap.
async fn run_from_git(args: &UpdateArgs) -> Result<()> {
    let source_dir: PathBuf = match args.source_dir.clone() {
        Some(d) => d,
        // Fall back to [update] source_dir from the config file.
        None => helpers::load_config(args.config.as_deref())
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
