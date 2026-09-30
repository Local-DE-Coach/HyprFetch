# Changelog

All notable changes to HyprFetch are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
While pre-1.0, breaking API changes are allowed in MINOR bumps.

## [Unreleased]

## [0.4.6] — 2026-09-30 (Auto-save extensions from Content-Type, cross-browser theme sync, real file managers, resource usage + background mode)

### Fixed — images saved as `images?q=tbn:ANd9Gc…` in `other/` instead of Pictures
- Download names are now derived properly: the filename comes from the last
  URL path segment with query/fragment junk stripped and percent-decoding
  applied, then the server's `Content-Type` decides the real extension.
  An image without any extension in the URL (`image/jpeg` → `.jpg`,
  `image/png` → `.png`, …) is corrected **before any byte is written** and
  an auto-categorized task is re-sorted into the matching folder — exactly
  what the owner reported: Google-thumbnail URLs landing as
  `images?q=tbn:…` inside `other/`. Files that already have a known
  extension are never touched; explicit save dirs and forced categories
  keep their folder (extension-only fix). ~70 media types mapped.
- `POST /api/inspect` returns the sniffed name + `content_type`, so the
  IDM-style confirm dialog shows `images.jpg → ~/Downloads/pictures/`
  before the download starts.

### Fixed — "GO" opened a terminal instead of the file manager
- Root cause: on minimal window-manager setups (Hyprland + foot/kitty/…)
  the desktop's default `inode/directory` handler is missing or IS the
  terminal, so `xdg-open <folder>` spawned a terminal. Folder opening now
  detects that case and launches the first installed GUI file manager
  (nautilus, dolphin, nemo, thunar, caja, pcmanfm-qt, pcmanfm, krusader,
  spacefm, doublecmd) instead — never a terminal. `HYPRFETCH_FILE_OPENER`
  override unchanged.

### Added — cross-browser theme sync
- The selected theme (5 styles × dark/light) is stored on the SERVER
  (`ui_theme_style` / `ui_theme_mode` settings) and re-applied on every UI
  load — a NEW browser or device now opens with the same look the owner
  picked instead of falling back to per-browser localStorage. Settings
  validation keeps the values sane (`400` on garbage).

### Added — Dashboard "Save folders" cards are buttons
- Every folder card (and the base folder) now opens that exact location in
  the file manager via the new `POST /api/open-folder` endpoint, which
  only permits HyprFetch's own save folders (400 for anything else).

### Added — resource usage (this app only) + close-to-background mode
- `GET /api/system/usage` reports the RAM (current + peak), CPU % and
  thread count of the HyprFetch process itself, read from the kernel's own
  accounting. The footer widget (Settings → **App & background** → show
  resource usage) samples it every 3 s — no system-wide metrics.
- Background (quiet) mode: the app looks closed but stays alive. The
  header ⏾ button (or Settings) puts the daemon into low-usage mode —
  downloads keep running, the engine's own wakeups drop 10× — and
  `hyprfetch open` starts it if needed and opens the browser instantly.
  `hyprfetch close` is the CLI twin of the ⏾ button.

### Tests
- 10 new tests: Content-Type → extension table + filename sniffing/sanitizer
  units, engine-level rename + re-sort e2e (auto-sorted and explicit-dir
  variants), create/inspect filename behavior, settings validation,
  open-folder allow/deny, usage + power endpoints. `cargo test --workspace`
  175 green, clippy clean, UI eslint clean.

## [0.4.5] — 2026-09-30 (Updater: in-app update that actually works + stale-copy cleanup)

### Fixed — update inside the WebUI failed on system installs
- `POST /api/update/apply` refused every system install (pacman/deb/rpm,
  `/usr/bin`) with "run `sudo hyprfetch update`" — the button could never
  work. The daemon now escalates **without a TTY**: passwordless `sudo -n`
  first (NOPASSWD entries or cached credentials), then **`pkexec`** (the
  desktop polkit agent shows the graphical password prompt, the same way GUI
  package managers elevate). When neither answers, the response is the usual
  actionable terminal hint. CLI `hyprfetch update` keeps interactive sudo.

### Fixed — "updated but the WebUI still shows the old version" (shadowing copies)
- Root cause: a **second hyprfetch copy earlier on `PATH`** (typically an old
  `install.sh` build in `/usr/local/bin` next to the pacman-managed
  `/usr/bin` one) keeps launching the stale build — `--version`, autostart
  and the WebUI all come from the old file, no matter what was just
  installed. HyprFetch now detects, reports and removes such copies:
  - `GET /api/update/check` and `POST /api/update/apply` responses carry
    `stale_copies` (path, version probe, pacman owner, whether the copy
    *shadows* the running binary); the Updates page shows a warning banner
    with a **Remove stale copies** button (`POST /api/update/stale-copies/fix`).
  - `hyprfetch update` warns before installing and offers removal after the
    swap (interactive prompt on a TTY; exact `rm`/pacman commands otherwise).
  - `hyprfetch doctor` gained a **stale copies** report.
- Removal safety rails: never the running binary, never package-owned files
  (they print the `pacman -Rns` command instead), one privileged `rm -f`
  through `sudo -n`/`pkexec` when the directory is root-owned.

### Changed — installer hardening (first-time install & uninstall)
- `install.sh` now verifies what `hyprfetch` will actually launch right
  after installing: when `command -v hyprfetch` resolves to a different
  copy, it prints exactly which file shadows the fresh install and the two
  ways to fix it (remove the stale file, or `hyprfetch doctor` / the WebUI
  Updates page on 0.4.5+). `--uninstall` continues to sweep
  `/usr/local/bin`, `~/.local/bin`, `/usr/bin`, desktop entry, icon, docs
  and updater leftovers; package-manager removal commands are echoed at the
  end.

### Tests
- 11 new core tests (non-interactive tool discovery via sudo/pkexec shims,
  shadow-scan ordering, removal rails incl. escalation routing); 2 new API
  handler tests; updater e2e extended to **13 scenarios / 43 PASS** (WebUI
  apply escalates via `sudo -n` or refuses with the hint — env-dependent;
  shim-verified escalation attempt + rollback; real shadow-copy
  detection → one-click fix → clean re-check). `cargo test --workspace`
  157 green, clippy clean.

## [0.4.4] — 2026-09-30 (Web UI: confirm-before-download, desktop GO/Open actions, themes)

### Added — Web UI & API (owner request, "like IDM on Windows")
- **IDM-style confirm popup before every download.** The Add dialog is now
  two steps: enter the URL(s), then a **Confirm download** panel shows, per
  file, the probed file name, real size (HTTP HEAD with SSRF + redirect
  checks), single- vs multi-segment support and the **exact resolved save
  path** — nothing starts until you press *Start download*. Powered by the
  new `POST /api/inspect` endpoint (uses the same SSRF policy and user agent
  as real downloads; probe failure never blocks the download).
- **Floating download monitor (per-download streaming progress).** A compact
  bottom-right panel streams live progress for every active download (bar,
  percent, bytes, per-task speed, totals) — collapsed to a `⇣N` bubble, and
  can be hidden entirely from the panel or the header `⇣N` button. The
  choice persists in localStorage; it reuses the existing WebSocket stores
  (zero extra connections, zero extra RAM).
- **GO / Open desktop actions on downloaded files** (browser control panel
  meets the Linux desktop):
  - **GO** — opens the file's folder in your file manager at the right
    location, *without* launching the file (`POST /api/tasks/:id/reveal`);
  - **Open** — opens the file with its default Linux application via
    `xdg-open` (`POST /api/tasks/:id/open`, finished downloads only).
    Buttons appear next to the filename on hover in the Tasks rows and the
    Dashboard's recent list (always visible on touch screens). Override the
    opener program with `HYPRFETCH_FILE_OPENER`.
- **5 theme styles × dark & light mode.** Slate (the classic look), Ocean,
  Forest, Coffee and Cyber — each with a dark and a light variant, chosen on
  the new Settings → Appearance card (color swatches) or the header ☀️/🌙
  button; first visit follows the browser preference and the choice is
  remembered per browser. Themes are plain CSS variables compiled once —
  switching costs zero extra RAM (bundle stays ~45 KiB gzipped).

### Fixed
- **Tasks page columns were unreadable on smaller/odd screen sizes** (the
  wide table squeezed its last columns into an unusable smear). The page now
  uses responsive rows: filename + badges on line one, save path + date as a
  secondary mono line, progress + size + speed below — plus the new GO/Open
  and lifecycle buttons. Nothing is cut off at any width.
- **The layout no longer feels "locked" on different screen sizes**: the
  navbar collapses to icons on narrow screens (speed/count hidden, full
  labels from `md` up), the main container fluidly resizes, the Add modal
  scrolls instead of clipping, and the Tasks filter tabs scroll instead of
  overflowing. Verified: zero horizontal overflow at 390 px.
- **`category: "auto"` was rejected with 400** when a task was created with
  the Add dialog's default option (the *recommended* "Auto-sort by file
  type") — `resolve_save_dir` forgot to treat `"auto"` like an omitted
  category, so the modal's default could never start a download. Caught by
  the v0.4.4 API battery; `POST /api/tasks` and `POST /api/inspect` both
  accept it now.

### Tests
- 8 new handler tests (inspect scheme/SSRF/no-side-effect; open/reveal 404,
  409 unfinished, missing file, real spawn via `HYPRFETCH_FILE_OPENER`);
  `category:"auto"` regression test. `cargo test --workspace`: 150 green.
- New live battery `scripts/test_hyprfetch_app3.sh` — 25 checks (fresh
  bundle, 10 theme CSS sets, inspect fields, auto fix, real download →
  GO/Open, WS upgrade). Legacy batteries refreshed to dynamic asset names:
  38 + 13 + 25 = 76 checks green; updater e2e still 33/33.
- Real-browser verification: every page rendered error-free; themes,
  confirm dialog, floatbar hide/show cycle and mobile layout (390 px)
  screenshot-verified.

## [0.4.3] — 2026-09-30 (system installs update cleanly — no more "permission denied")

### Fixed
- **`hyprfetch update` failed with `io: Permission denied (os error 13)` on
  any package-manager install** (Arch PKGBUILD / .deb / .rpm put the binary
  in a root-owned directory such as `/usr/bin`, which a user process cannot
  write to). The updater now probes whether the running binary's directory is
  writable **before** downloading:
  - user-owned installs (install.sh per-user, tarball in `$HOME`) — unchanged
    direct atomic swap;
  - system installs — the swap runs through one self-recovering privileged
    script (`sudo`, falling back to `doas`): move old → `install -m 0755` new
    → on any failure move old back, so a half-finished update can never leave
    the machine without a working `hyprfetch`. The CLI prints what will happen
    (and a pacman-ownership note) before the confirmation prompt; `sudo`
    prompts for the password once.
- **The web UI updater button hit the same wall** and surfaced a raw error.
  The daemon cannot prompt for a password, so `POST /api/update/apply` now
  refuses system installs with an actionable message pointing at
  `sudo hyprfetch update`.
- **Arch instructions no longer litter your folders.** The PKGBUILD flow in
  the docs, release notes and istias.tech pages now builds in a `mktemp -d`
  scratch directory, so makepkg's `src/`, `pkg/`, `PKGBUILD` and
  `*.pkg.tar.zst` never land in `~/Desktop` again.

### Added
- **App icon + desktop integration everywhere**: a proper scalable icon
  (`packaging/icons/hyprfetch.svg`, `Icon=hyprfetch` in the desktop entry)
  ships in the release tarball and is installed by the PKGBUILD, .rpm, .deb
  and install.sh (system and per-user hicolor paths) — HyprFetch now shows up
  in app launchers like any other installed application. Uninstall removes
  all of it.
- install.sh warns when a pacman-managed `/usr/bin/hyprfetch` would be
  shadowed by a standalone install.

## [0.4.2] — 2026-09-30 (blank-UI hotfix — the web UI works again)

### Fixed
- **Web UI rendered a completely blank page** (v0.4.1 regression). The SPA
  died at boot with `Uncaught ReferenceError: Cannot access '$t' before
  initialization`: `lib/router.js` read the `PAGES` constant from `page`'s
  module initializer, but `PAGES` was declared *below* — a temporal-dead-zone
  crash that killed the whole app before anything could mount. `PAGES` now
  precedes its users. Verified in a real headless browser: Dashboard, Tasks,
  Settings and Updates all render with a clean console.
- **Release CI now guards this bug class so it cannot ship again**:
  `release.yml` rebuilds the UI from source (so the embedded bundle can never
  go stale against the committed sources), runs ESLint with
  `no-use-before-define` on the UI sources (flags the exact pattern), and
  imports the built bundle in Node with browser stubs
  (`scripts/ui_boot_check.mjs`) — failing the release on any boot-time TDZ.
  Both gates were verified against the broken v0.4.1 bundle (both catch it)
  and the fixed one (both pass).

### Added
- **Run & use section on every GitHub release page**: the generated release
  notes now include the day-to-day commands — run foreground/detached,
  status, stop, restart, logs, update — right between the install block and
  the changelog, so "how do I run/stop/update this" is answered on the
  release page itself.
- **README.md ships as a release asset**: the publish job attaches the
  README to every release next to the binaries, so the full command
  reference is downloadable without cloning the (private) repo.
- **Quick commands table in README.md**: install → run → stop → update →
  uninstall cheat sheet at the top, before the detailed sections.

## [0.4.1] — 2026-09-30 (install/uninstall hardening + tag-per-release flow)

### Added
- **Tag-per-release release flow** (`release.yml`): the workflow now derives
  the release tag from the app version in `[workspace.package]`
  (`Cargo.toml`) itself — bump the version, dispatch the workflow, and it
  creates + pushes the NEW annotated tag (`0.4.1` → `v0.4.1`), builds,
  publishes a NEW release on that tag and mirrors the update channel.
  Guards: dispatching with an already-released version **fails** ("re-
  releasing on an old tag is not allowed" — the old-tag re-publish that
  repaired v0.4.0 in place is now impossible); a stale tag pointing at
  another commit fails; a pushed tag that mismatches `Cargo.toml` fails;
  a tag that exists at HEAD without a release (a crashed run) is the only
  allowed re-entry path. Build/release jobs check out the resolved tag, so
  artifacts always correspond exactly to the tagged code.
- **One-line installer** (`https://istias.tech/hyprfetch/updates/install.sh`,
  served by the Docs-side mirror): POSIX sh, detects CPU + libc, resolves
  `latest.json`, sha256-verifies the tarball, installs to
  `/usr/local/bin` (or `~/.local/bin` without root) and keeps the previous
  binary as `.old`; `--uninstall [--purge]` stops the daemon and removes
  binary/desktop entry (data only with `--purge`; downloads never touched).
  E2E-tested against the live channel: install → `--version` →
  `update --check` → uninstall round-trip.
- istias.tech pages: "Four ways to get HyprFetch" (script / native package /
  manual tarball / browser) and a per-platform **Uninstall** section; the
  version panel now also lists the server-hosted `.deb`, `.rpm` and
  `PKGBUILD`.
- istias.tech/hyprfetch: new **Run & use** section — 4-step quick start
  (start server → open dashboard → add download → stop), foreground vs
  pm2-style daemon (`serve` / `daemon start|status|restart|stop`, `logs -f`),
  a 10-command cheat sheet, the most useful flags with their `HYPRFETCH_*`
  env twins, a where-your-files-live table and a dashboard-walkthrough card.
  Answers "I installed it — how do I run it?" directly on the product page.
- New page **istias.tech/hyprfetch/features**: the full feature catalogue
  transcribed from `docs/feature-research.md` — 73 tracked features
  (46 shipped & E2E-verified, 1 partial, 26 planned) across 12 categories,
  with All / Shipped / In-progress / Planned filters and a status legend.
  Shared data file keeps the counters on both pages in sync. Torrent note
  aligned with `architecture.md` non-goals (out of scope for v1).

### Changed
- Every install/update command on the pages and in the release notes now
  points at the istias.tech channel — the last GitHub download links are
  gone (Arch PKGBUILD included).
- `packaging/arch/PKGBUILD.bin.template`: fixed `_srcdir` to match the clean
  `hyprfetch-<ver>-linux-x64` archive root (the old triple name made
  `makepkg` fail) and moved `source=` from GitHub releases to the
  sha256-verified channel; the Docs mirror generates the server-side
  PKGBUILD from the same template + manifest, so the channel copy can never
  regress.
- `hyprfetch-mirror.yml` self-heals: it re-mirrors when the served version's
  packages / PKGBUILD / install.sh are missing, instead of skipping whenever
  `latest.json` matches.
- v0.4.0 release assets and notes were regenerated in place (dispatched
  re-run) so the existing release ships the fixed PKGBUILD and server links.

## [0.4.0] — 2026-09-29 (updates served only by our own server + new docs site)

### Changed — GitHub fully removed from the update flow (owner request)
- **The updater now has exactly ONE source: the self-hosted update channel**
  (`https://istias.tech/hyprfetch/updates/`). The GitHub REST API tier and the
  plain-git tier are **gone entirely** — `hyprfetch update --check` is one
  fast HTTPS GET and an install is download → sha256-verify → atomic swap.
  No tokens, no rate limits, works regardless of repo visibility. If the
  channel cannot be reached the CLI prints a clear error pointing at the
  updates page (`https://istias.tech/hyprfetch/updates`) — no silent fallback.
- `--repo` / `--token` / `--from-git` / `--source-dir` flags removed from
  `hyprfetch update`; `[update] repo / token / git_url / source_dir` config
  keys are still parsed but ignored (legacy configs keep working). Token /
  clone-scanning machinery deleted from the codebase.
- Web UI: the Settings **GitHub token** card became an **Update channel**
  card; the Updates card copy now states GitHub is never contacted;
  `ui/dist` rebuilt.
- `/api/update/check` answers from the channel only — on failure it returns
  `available: false` + a human-readable `error` + `updates_page`; legacy
  `via_channel` / `via_git` payload fields are gone.

### Added — clean release asset names
- Release archives are renamed for humans: `hyprfetch-<ver>-linux-x64.tar.gz`,
  `hyprfetch-<ver>-linux-arm64.tar.gz`, `hyprfetch-<ver>-linux-musl-x64.tar.gz`
  (the update-channel manifest maps them back to target triples for the
  updater). PKGBUILD + rpm spec templates pinned to the new names.

### Added — generated release notes with update/install commands
- The release workflow now assembles a rich release body automatically:
  the matching `## [X.Y.Z]` section extracted from this CHANGELOG, the
  one-line update commands (`hyprfetch update --check` / `hyprfetch update`),
  a per-platform install table, the downloads table and a sha256 verify
  block, plus links to the project pages. Assembled with python3 (no shell
  interpolation of changelog content).

### Added — istias.tech/hyprfetch docs pages (Local-DE-Coach/Docs)
- The Docs portal (which owns the istias.tech server) gained two static,
  RAM-friendly pages: **`/hyprfetch`** (app info, per-platform install
  commands, 4-step test & verify guide) and **`/hyprfetch/updates`** (live
  version panel fetched client-side from the update-channel manifest, CLI
  update commands, manual per-platform update steps, troubleshooting).
- The Docs deploy workflow self-heals the server routing on every deploy:
  creates `/var/www/istias.tech/hyprfetch/updates` and installs the nginx
  `location` blocks (exact-match page proxy + `^~` static alias with CORS +
  short cache). nginx serves the archives straight from disk — zero Next.js
  RAM on the 500 MB VPS. Also fixed a month-old deploy breaker (raw `<60`
  in `test-results/page.tsx` JSX text).

### Added — update-channel E2E suite committed to the repo
- `scripts/e2e_update_channel.sh` — 8 scenarios / 23 checks against a local
  mock channel: check-available, up-to-date, full install (download →
  sha256 → swap → `.old` backup → no temp leftovers), tampered-manifest
  refusal (binary untouched), unreachable-channel error text (asserts NO
  GitHub fallback attempt), disabled mode, `/api/update/check` payload
  shape, malformed-manifest handling. Plus
  `scripts/simulate_release_notes.sh` which exercises the release-notes
  generation step locally before tagging.

## [0.3.3] — 2026-09-29 (fast self-hosted update channel on istias.tech)

### Added — update channel tier: `hyprfetch update` now checks the project's own server FIRST (owner request)
- **New tier 0 in the updater** — the **update channel**: a plain HTTPS
  manifest (`latest.json`) mirrored to `https://istias.tech/hyprfetch/updates/`
  (the owner's own server, in a NEW directory — the existing site on the same
  host is untouched). One fast HTTPS GET answers "is there a new version?" with
  **no GitHub API, no rate limits, no tokens** — and it works regardless of
  repo visibility because CI (with deploy credentials) populates the mirror on
  every release. Install = download the manifest-listed tarball →
  **sha256-verify against the manifest** → extract → atomic swap → daemon
  restart (identical safety to the API tier).
- **Graceful three-tier chain**: channel → GitHub REST API → plain git. If the
  mirror is unreachable the updater announces it and falls through; if the
  channel is disabled (`[update] channel = ""` / `--channel ""`) it skips
  silently. Install paths are per-tier: channel/API download tarballs, git
  builds from source.
- **Release workflow gained a `deploy-update-channel` job**: after the GitHub
  release publishes, CI generates `latest.json` (version, tag, notes URL and
  per-target `{url, sha256, size}` entries built from the produced archives +
  their `.sha256` files), uploads the archives into
  `<DEPLOY_PATH>/updates/<version>/` and atomically swaps the top-level
  `latest.json` (scp to temp name → `mv(2)`). Old versions stay downloadable.
  SSH target is configured via repo secrets: `DEPLOY_SSH_KEY` (required),
  `DEPLOY_HOST` (default `138.197.73.65`), `DEPLOY_USER` (default `root`),
  `DEPLOY_PORT` (default `22`), `DEPLOY_PATH` (default
  `/var/www/istias.tech/hyprfetch`). Without `DEPLOY_SSH_KEY` the job skips
  with a notice and the GitHub release still ships. A post-deploy step
  verifies the public URL serves the new version (warns with setup guidance
  when the origin route is not configured yet).
- **CLI**: `hyprfetch update [--check]` prints the answering tier
  (`checked via update channel (<url>)`, GitHub auth label, or
  `git access detected via <url>`); new `--channel <url>` flag;
  `--check` now hints `(fast download from the update channel)`.
- **Config**: `[update] channel` (default = the project mirror; `""`
  disables) + `HYPRFETCH_UPDATE_CHANNEL` env override; precedence
  `--channel` > env > config > default. `hyprfetch doctor` shows the active
  channel line.
- **API/UI**: `GET /api/update/check` tries the channel first and reports
  `"via_channel": true` + `"channel"` in the payload; `POST /api/update/apply`
  installs from the mirror when the cached check came from the channel. The
  Updates card shows a `via update channel` badge and updated copy.
- **Docs**: new `docs/update-channel.md` (architecture, manifest schema,
  one-time nginx/origin setup for the new directory, Cloudflare notes,
  secrets table + deploy-key generation, client overrides, verification
  commands); README Self-update + install.md rewritten around the three
  tiers; api.md documents the new payload fields.
- **Tests**: 4 new unit tests (manifest URL join, channel enable/disable,
  manifest parse + host-asset pick, strict schema); new
  `scripts/e2e_update_channel.sh` — 8 scenarios / 28 checks: mirror prep,
  check via channel, full install (download → sha256 → atomic swap →
  rollback copy), tampered-manifest refusal (binary untouched), up-to-date,
  404 fallback, `--channel ""` disable, `/api/update/check` payload.
  `scripts/e2e_update_tiers.sh` made hermetic for the channel-first reality
  (channel disabled per scenario; dead API base for the git-tier install).
  138 workspace tests + 43/43 downloader E2E + 28/28 channel E2E + 20/20
  tiers E2E green.

## [0.3.2] — 2026-09-29 (private-repo self-update without a token)

### Fixed — `hyprfetch update` on private repos with SSH-only access (owner request)
- The updater now has **two access tiers**. Tier 1 stays as before (GitHub
  REST API with a PAT → prebuilt tarball → sha256 → atomic swap). When the
  API cannot see the private repo and no token exists anywhere, tier 2
  kicks in: **plain git**. SSH keys cannot call the REST API but they can
  run git, so the updater now resolves the newest release tag via
  `git ls-remote --tags` and installs by **shallow-cloning that exact tag
  and running `cargo build --release --locked`** — atomic swap + daemon
  restart just like the tarball path. If `git pull` works for the user,
  `hyprfetch update` now works too.
- Git remote discovery: `[update] git_url` in the config → a local clone's
  `remote.origin.url` (well-known locations scanned) → the derived
  `git@github.com:<repo>.git`. All git invocations run non-interactively
  (batch-mode SSH, no prompts — the updater fails fast instead of hanging).
- Token discovery widened: `GH_TOKEN` env, `gh auth token` (GitHub CLI),
  and `git credential fill` (store/cache/libsecret/keyring) join the
  existing chain; each source is labelled in the CLI output
  (`authenticated (env)`, `(gh cli)`, `(clone origin)`, …).
- `hyprfetch update` now actually loads the config file (`[update]`
  section: `repo` / `token` / `git_url` / `source_dir`) and gained a
  `--config` flag.
- `GET /api/update/check` (web UI Updates card) falls back to the git tier
  the same way and reports `"via_git": true` with no asset;
  `POST /api/update/apply` answers `400` with guidance to run the CLI for
  git-found updates (a web request cannot rebuild the binary).
- `hyprfetch doctor` now shows the update token source and the git remote
  it will use.
- E2E `scripts/e2e_update_tiers.sh` (4 scenarios, 20 checks): no-auth
  hints, PAT tier against the real GitHub release, full git-tier install
  (shallow clone → build → atomic swap → rollback copy) against a local
  git remote, clone-origin discovery. Unit tests for tag parsing,
  latest-tag selection, candidate ordering and real-subprocess
  ls-remote/shallow-clone against local repos (134 workspace tests green;
  downloader E2E 43/43 still green).

## [0.3.1] — 2026-09-29 (re-cut with the multi-page UI + category folders)

### Added — multi-page web UI (owner request, applied)
- The web UI is now a **4-page app** (hash-routed SPA, still one tiny
  embedded bundle — no SvelteKit runtime):
  - **Dashboard** — live speed, per-state counters (downloading / queued /
    paused / done / errors), every active download with progress, recent
    finished list, and a "save folders" overview.
  - **Tasks** — full table of all tasks with filters (all / active /
    downloading / done / errors), search, progress, speed, save path, and
    per-task controls (pause / resume / retry / cancel / remove).
  - **Settings** — save folders, auto-sort toggle, per-category folder
    overrides, queue defaults, QoS cap, SSRF protection toggle, and the
    GitHub token for updates.
  - **Updates** — current version vs latest release, one-click
    install-and-restart, plain restart, and the CLI equivalents.

### Added — category save folders (owner request, applied)
- Downloads are **auto-sorted by file type** into
  `~/Downloads/{video,pictures,music,compress,documents,apps,other}`.
- The folders are **created automatically** at server startup, whenever
  directory settings change, and when a task is created — nothing to mkdir.
- Default base dir moved from `~/Desktop` to **`~/Downloads`** (the
  standard Linux location; still overridable via `--download-dir`,
  config, or the UI).
- Each category folder can be **overridden individually**
  (`category_dir_<name>` setting / Settings page); a task can force a
  category (`POST /api/tasks` `category`) or **direct-save** to an exact
  folder (`save_dir`) — direct save always wins.
- `GET /api/categories` exposes the effective layout for the UI and scripts.
- Task DTOs carry a read-only `category` field (extension-derived).

### Added — updater works out-of-the-box for clone installs (owner request, applied)
- When no token is configured anywhere (`--token` → env → config →
  settings DB), the updater now **reads the PAT from the local source
  clone's origin URL** (`~/HyprFetch`, `~/Projects/HyprFetch`, `~/src/…`,
  `~/code/…`, `~/Developer/…`, or `[update] source_dir`). A
  `git clone https://<USER>:<PAT>@github.com/…` install needs **zero extra
  configuration** for `hyprfetch update --check` / `hyprfetch update`.
- The GitHub token can be saved from the UI settings page
  (`github_token`); it is stored locally and **masked on
  `GET /api/settings`** (`github_token_set`), never echoed back.

### Added — run modes, in-app updates, resource footprint (owner request, applied)
- **Node.js-style run modes:**
  - `hyprfetch dev` — foreground dev mode: pretty debug-level logs, auto-opens
    the web UI, same flags as `serve` (`serve --mode dev` is the flag form).
  - `hyprfetch daemon start [--serve flags…]` — detached prod server: own
    process group, stdout/stderr into a size-rotated log file
    (`~/.local/state/hyprfetch/logs/hyprfetch.log`, 5 MiB × 3 rotations), JSON
    PID file, and `start` returns only after `/healthz` answers.
  - `hyprfetch daemon stop|restart|status` + `hyprfetch logs [-f] [-n N]` —
    pm2-style lifecycle. `status` prints PID, uptime, live version/active
    task/ws-client counts and update availability.
- **In-app self-update (private-repo aware):**
  - `hyprfetch update [--check] [--yes] [--repo owner/name]
    [--token PAT] [--from-git --source-dir <clone>]` — queries the latest
    GitHub release, downloads the matching target tarball through the REST
    API octet-stream endpoint (works with fine-grained PATs on the private
    repo), verifies sha256, then atomically swaps the binary (previous
    binary kept as `hyprfetch.old`). Restarts a running daemon automatically.
  - REST: `GET /api/update/check`, `POST /api/update/apply?restart=…`,
    `POST /api/update/restart`. Apply + restart perform **drain-pause →
    swap → re-exec → auto-resume**; the new server binds with a retry window
    so the hand-off is race-free. The UI gets an **Updates** card (check /
    install & restart / restart).
  - Config: `[update] repo / token / source_dir`; env `HYPRFETCH_GITHUB_TOKEN`
    / `GITHUB_TOKEN`; `HYPRFETCH_UPDATE_API` overrides the API base (tests).
- **`GET /api/server`** — version, uptime, active tasks, ws clients, cached
  update availability (consumed by `daemon status` and the UI footer).
- **Sleep mode `--exit-when-idle <minutes>`** (config `exit_when_idle`): the
  server exits gracefully after N minutes with zero active downloads and zero
  UI clients — it never sits resident doing nothing. Default off.
- **`--workers <n>`** (config `workers`, env `HYPRFETCH_WORKERS`): tokio
  worker threads, default **2** — the workload is IO-bound, so a lean runtime
  keeps the idle RAM/CPU footprint minimal.
- **Web UI restyled with Tailwind + DaisyUI** (`dim` theme): navbar with live
  stats, card-based task list, DaisyUI badges/progress/buttons/toggle/modal.
  Total bundle ≈ 18 KiB gzipped (CSS 8.4 KiB + JS 9.7 KiB) — pure CSS, no
  runtime framework cost, RAM-neutral.
- **`docs/feature-research.md`** — feature checklist against the standard
  Linux download-manager research set: shipped items ✅ with proof
  (test / E2E / asset), missing items ⬜ unchanged.

### Perf (measured by the v0.3.1 E2E, 4 MiB segmented download)
- Idle RSS **8.0 MB**; peak during download **9.4 MB**; 9.4 MB after
  completion (no leak). Typical Electron/Java download managers sit at
  100–500 MB with the same feature surface.

### Fixed
- **In-app restart port hand-off:** `POST /api/update/restart` spawns the
  replacement before the old listener closes; the server now retries a busy
  bind for up to 15 s, making restart/update hand-offs deterministic.

### Added (2026-09-29 — desktop integration for the v0.2.0 re-release)
- **Desktop entry** (`packaging/desktop/hyprfetch.desktop`) so HyprFetch
  appears in Linux desktop menus / app launchers. Installed at the standard
  applications path `/usr/share/applications/hyprfetch.desktop` by every
  packaging channel: the release tarball now ships the file, and the Arch
  PKGBUILD, the .rpm spec, and the .deb (`cargo-deb` assets) all install it.
  Launching the entry runs `hyprfetch serve` (web UI at
  `http://127.0.0.1:7780`).

### Changed (2026-09-29 — default download directory)
- **Default download dir is now `~/Desktop`** (was `~/Downloads`): finished
  downloads land right on the user's desktop where they are immediately
  visible. Still overridable per request (`save_dir`), via the
  `download_dir` setting / config file / `--download-dir`, and falls back to
  `/tmp/Desktop` when `HOME` is unset. Docs (`README.md`,
  `docs/install.md`) updated to match.

### Fixed (found by the 2026-09-28 E2E re-run after the axum 0.8 upgrade)
- **Resume-after-pause race could strand a task in `downloading` forever.**
  Pausing is asynchronous; the resume route flipped the DB row straight to
  `downloading` and then called `engine.start()`, which rejected
  `downloading` rows unconditionally, swallowed the error, and answered 2xx —
  leaving a task with no coordinator attached. The engine now treats a
  `downloading` row with **no live coordinator** as restartable, the resume
  route retries through the pause wind-down window (~2 s) and, on persistent
  failure, rolls the row back to `paused` and returns **409** instead of a
  silent 200. Regression test:
  `resume_immediately_after_pause_does_not_strand_task` (drives pause→resume
  with zero delay against a live mock server and requires completion).
- **axum 0.8 migration fixes:** route path params updated to the new `{id}`
  syntax (the old `:id` syntax panics at startup on axum 0.8) and the WS sink
  now passes `Utf8Bytes` (`Message::Text(json.into())`).

### Dependencies (merged from 2 new Dependabot branches, 2026-09-28)
- **cargo:** `axum` 0.7.9 → 0.8.9 (breaking: route `{id}` syntax, WS
  `Message::Text` now `Utf8Bytes` — code fixed), `tokio-tungstenite` 0.24 →
  0.30.0 (tests compile unchanged). Verified: fmt + clippy `-D warnings`
  clean, **111/111 tests** (new resume-race regression test included), live
  E2E **30/30** incl. a 1 GB real-network download (byte-exact head+tail
  sha256 + exact size), pause→resume→complete on 50 MB, QoS cap accuracy
  (10 MB @ 1 MiB/s in 9.0 s), and live WebSocket event checks.
- **Housekeeping:** both Dependabot branches merged into `main` and deleted;
  PRs #20, #21 closed.

### Dependencies (merged from 10 Dependabot branches, 2026-09-28)
- **cargo:** `thiserror` 1 → 2.0.21, `toml` 0.8 → 1.1.6+spec-1.1.0,
  `tokio-tungstenite` 0.21 → 0.24.0, `rusqlite` 0.32 → 0.40.2 (bundled SQLite
  refreshed), `governor` 0.6 → 0.10.4. No source changes required — the APIs
  HyprFetch uses are stable across each bump. Verified by the full suite
  (110/110) + live smoke (config parse, WAL open, download through the new
  token bucket, doctor).
- **CI actions:** `actions/checkout` 4 → 7, `actions/upload-artifact` 4 → 7,
  `actions/download-artifact` 4 → 8, `actions/stale` 9 → 11,
  `softprops/action-gh-release` 2 → 3. Workflows re-validated (YAML parse +
  `uses:` review).
- **Housekeeping:** the governor merge initially produced a duplicated
  `Cargo.lock` stanza (`hashbrown` twice); the lock was regenerated and
  verified. All 10 Dependabot branches deleted (remote now has only `main`),
  PRs #10–#19 closed.

### Fixed (found by the 2026-09-28 full audit: code review + live smoke test)
- **Downloads from servers without Range support failed entirely.** The
  segment worker required `206 Partial Content` even on the single-connection
  fallback path, so any server answering `200 OK` with the whole file (no
  `Accept-Ranges`) produced `only 0 of 1 segments completed`. The worker now
  accepts a 200 whole-file body when the segment starts at byte 0 and caps
  writes at the segment end. Regression test:
  `download_from_server_without_range_support`.
- **Finished tasks were never reaped from the engine's active-task map** —
  the map grew forever, `is_running()` stayed true after completion, and
  re-starting a finished task failed with "already running". Coordinator
  wrappers now remove their own entry; the map only holds running tasks
  (`finished_task_is_reaped_from_active_map`).
- **Queue double-spawn race could mark tasks `removed` spuriously** —
  spawning two coordinators for one task overwrote the map entry and dropped
  the live coordinator's only command sender (interpreted as cancel).
  `spawn_coordinator` is now check-and-insert atomic and each pump pass
  filters ids it already started.

### Added
- **`max_concurrent_tasks` is now enforced** (was a seeded-but-dead setting):
  a queue pump starts queued tasks oldest-first while running < limit and
  chains new starts when running tasks finish; 0/missing = unlimited;
  explicit user resumes bypass the cap.
  Tests: `queue_pump_respects_max_concurrent_tasks`,
  `queue_pump_unlimited_drains_queue`.
- **`POST /api/tasks/:id/retry`** — re-run an errored task: clears the
  error, re-queues, keeps persisted offsets (remote-change recheck on
  restart discards them if the file changed). Tests: 3 API cases + live
  smoke.
- **`DELETE /api/tasks/:id?delete_file=true`** now really deletes the
  (partial) file from disk (previously the param was silently ignored).
- **Bearer-token auth on non-loopback binds** — was documented in api.md but
  not implemented. `/api/*` + `/ws` now require `Authorization: Bearer
  <token>` (or `?access_token=` for browser WebSocket clients) when the
  daemon is bound off-loopback; `401` + `WWW-Authenticate` otherwise;
  `/healthz` + SPA stay open. Token auto-generates on first run to
  `~/.config/hyprfetch/token` (0600); resolution order: CLI `--api-token` >
  `HYPRFETCH_API_TOKEN` > config > settings > token file. Tests: 7 auth
  cases + live smoke (401/200/query-param).
- **Config file support** — `~/.config/hyprfetch/config.toml` (keys: `bind`,
  `db_path`, `download_dir`, `segments`, `allow_private`, `api_token`) with
  `--config` override; precedence CLI > env > file > default. Was documented
  in README/development.md but not implemented.
- **`user_agent` and `ssrf_block_private` settings are honored** at startup
  (were seeded-dead).
- **UI: Retry button** on errored tasks (Active + Finished lists) and the
  Finished ✕ now removes task + downloaded file (`?delete_file=true`);
  `ui/dist` rebuilt and committed.
- **In-session remote-change recheck** — on every (re)start of a task
  (pause → resume, retry), stored ETag/Last-Modified/size are compared
  against a fresh probe; stale offsets are discarded instead of corrupting
  the output (`resume_after_remote_change_mid_session_resets_offsets`).
- **`worktasks.md`** — project task board: per-feature-area status with test
  proof and verify commands, open bug tracker, backlog, and the task
  definition-of-done protocol for future sandboxes.
- docs/api.md: retry + delete_file documented; Auth section rewritten to
  match reality; settings enforcement table (which keys are live vs not yet).

## [0.2.0] — 2026-09-28

Packaging release: every release now ships ready-made Linux packages —
`.deb` for Ubuntu/Debian, `.rpm` for Fedora/RHEL, and a per-release
`PKGBUILD` for Arch — next to the binary tarballs. No engine changes; the
download code is identical to v0.1.1.

### Added
- **Linux packages in every release** (fast installation) — the release
  workflow now builds and attaches three package formats next to the binary
  tarballs:
  - `hyprfetch_<version>-1_amd64.deb` (Ubuntu/Debian) — built with
    `cargo-deb` from `[package.metadata.deb]` in `crates/hyprfetch/Cargo.toml`.
    Installs `/usr/bin/hyprfetch` + README/CHANGELOG/copyright under
    `/usr/share/doc/hyprfetch/`; the only runtime dependency is glibc
    (SQLite is bundled, TLS is rustls).
  - `hyprfetch-<version>-1.x86_64.rpm` (Fedora/RHEL) — built with
    `rpmbuild` from the new `packaging/rpm/hyprfetch.spec`, repackaging the
    x86_64 release tarball (binary + docs at the standard locations).
  - `PKGBUILD` (Arch Linux) — generated per release from the new
    `packaging/arch/PKGBUILD.bin.template` with the version and the sha256 of
    the x86_64 tarball substituted in, so Arch users can download one file
    from the release page and run `makepkg -si` for a fast, checksum-verified
    install (`hyprfetch-bin`).
- **docs/install.md** — full installation guide: .deb / .rpm / PKGBUILD /
  tarball / from-source paths, first-run flags, fd-limit note for heavy use.
- **docs/design.md** — design rationale ("why") for new contributors and
  sandboxes: language choice vs Go/Python, architecture, resume design,
  segmented downloads, QoS strategy, crate choices, prior art (aria2, gopeed),
  UI structure, WebSocket event contract, honest gotchas, and v1→v2 scope
  with per-item status markers.
- `.github/dependabot.yml` — weekly update PRs for Cargo dependencies and
  GitHub Actions.

### Changed
- Doc drift fixed: README + `docs/development.md` now point to the real UI
  directory (`crates/hyprfetch-api/ui/`, not the planned `web/`), README
  requires Rust 1.85+ (matching `rust-version`), and `docs/development.md`
  "Releases" lists the new deb/rpm/PKGBUILD artifacts and the
  tag-version-must-match-workspace-version rule.

## [0.1.1] — 2026-09-28

CI and engine reliability release. No new features — fixes the release
pipeline (aarch64 artifact was missing from v0.1.0) and two download
bugs found by sandbox testing of the v0.1.0 binary.

### Fixed
- **30-second total request timeout killed all long downloads** (sandbox test
  finding). `HttpClient` set reqwest's client-level `.timeout(30s)`, which covers
  the *entire* request including streaming the response body — so every segment
  worker aborted exactly 30 s in. Reproduced on thinkbroadband: 1 GB at ~16 MB/s
  died at ~455 MB (`only 0 of 8 segments completed`), the same task QoS-capped
  to 4 MiB/s died at ~130 MB — both at exactly ~32 s. The client now uses
  `connect_timeout(10s)` + `read_timeout(30s)` (idle-read gap) and no total
  timeout, so multi-minute transfers run to completion.
- **One dropped connection failed the whole task** — segment workers now retry
  transient failures (up to `MAX_SEGMENT_ATTEMPTS = 6`, exponential backoff
  1 s → 15 s) and resume from the last written byte offset. Retryable:
  transport errors, mid-body EOF, HTTP 5xx / 429. Non-retryable: disk I/O
  errors, SSRF violations, ignored Range headers, 4xx. Observed live: 4 of 8
  segments were dropped ~150 s into a download; with retry the task completed
  without user intervention.

### Changed
- `docs/api.md`: list-tasks example now matches the real `TaskDto`
  (`segments_requested`, epoch timestamps; no `speed_bps`/`eta_sec`/`segments_active`),
  and the not-implemented `POST /api/tasks/:id/retry` endpoint is marked as planned.

### Added
- **README: "Sandbox test results (2026-09-28)"** — full test data of the
  1 GB / 5 GB thinkbroadband runs (throughput, RSS, integrity sha256
  verification, resume-across-restart proof) and the analysis that led to the
  two engine fixes above.
- **Multi-sandbox coordination** — repo-root `worklog.md` records what each
  sandbox/agent did per commit; every commit is expected to update both
  `CHANGELOG.md` and `worklog.md` (protocol documented in the file and in
  `CONTRIBUTING.md`).

### Tests
- 91 → 93: retryability classification (`is_retryable`) and backoff sequencing
  (`backoff_delay`), both pure unit tests.

### Fixed (CI)
- **aarch64 release build** — the release workflow now sets
  `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc` (plus
  `CC_`/`AR_` for the C dependencies `ring` and `libsqlite3-sys`). Previously the
  aarch64 job linked with the host `cc` and rustc's self-contained lld, which failed
  with `rust-lld: error: --fix-cortex-a53-843419 is only supported on AArch64`.
- **release packaging on `workflow_dispatch`** — archive names and the published
  release now derive from the `tag` input (`inputs.tag || github.ref_name`) instead of
  `GITHUB_REF_NAME`, so dispatching from a branch no longer produces
  `hyprfetch-main-*` archives / a `main` release.

## [0.1.0] — 2026-09-27

First user-facing release: segmented downloads, one shared QoS bucket,
live WebSocket events, embedded browser UI, and crash-safe resume.

### Added
- **QoS rate limiter** (PR #5) — the differentiator:
  - `QosLimiter::acquire()` is now a real governor-backed token bucket —
    `acquire(bytes)` awaits actual download budget; oversized chunks are
    split into capacity-sized acquisitions automatically
  - **One shared limiter across all active tasks**: the engine owns a
    single engine-wide bucket restored from the `qos_enabled` /
    `qos_target_bps` settings, so the *aggregate* bandwidth of the whole
    daemon is capped, not each task separately
  - `PUT /api/qos` applies the new rate live to the running engine
  - Per-task `qos_override: force_off` bypasses the shared bucket
  - Runtime retune atomically swaps in a fresh full bucket; bucket
    capacity equals one second of the target rate
- **WebSocket event fan-out** (PR #6):
  - `GET /ws` upgrades and streams JSON events: `task:progress`
    (downloaded/total/speed_bps), `task:state` (state/error),
    `global:speed` (aggregate B/s + active task count)
  - Core `EventBus` (tokio broadcast, 1024-deep per subscriber); slow
    clients skip missed events — progress is self-correcting since every
    event carries absolute totals
  - Background speed aggregator emits `global:speed` every 1s while tasks
    are active, with one final zero event on the active→idle transition
  - `task:progress` is throttled to one event per 500ms per task
- **Embedded web UI** (PR #7):
  - Minimal Svelte 4 SPA compiled into the binary via rust-embed:
    active list with live progress bars, add-task modal (multi-URL,
    save dir, segment count), QoS bandwidth toggle
  - Served at `/`: hashed assets cached `immutable`, extension-less
    paths fall back to the SPA, `/api/*` keeps proper JSON 404s
  - `ui/dist` is committed — `cargo build` needs no Node toolchain;
    rebuild with `npm run build` (dev proxy documented in
    `crates/hyprfetch-api/ui/README.md`)
- **Resume persistence** (PR #8):
  - `hyprfetch serve` runs a startup resume pass: every incomplete task
    (queued/downloading/paused) is reloaded, HEAD-probed, and its stored
    ETag / Last-Modified / Content-Length compared with the remote
  - Remote unchanged → workers restart **from persisted segment offsets**
  - Remote changed → offsets discarded, download restarts from byte 0
  - `downloading` rows (crash leftovers) are normalized to paused first
  - Probe failure marks the task `error` with a reason (retryable);
    the pass never aborts on individual task failures
  - Offsets also reset when the local file is missing/truncated vs the
    stored total
  - `SegmentsRepo::delete_for_task()` for stale offset cleanup

### Tests
- Total: 91 tests across the workspace, all passing (64 → 91)
  - QoS: 12 (throttle rate, oversized-chunk splitting, shared budget
    across concurrent workers, retune swap, force_off bypass, API
    live-apply, settings restore)
  - Events: 5 unit tests (bus fan-out, idle transition, serialization)
    + 1 E2E over a real TCP WebSocket (upgrade, live 2 MiB 4-segment
    download under QoS, all three event kinds observed)
  - UI serving: 3 (index, hashed asset + API 404, deep-link fallback)
  - Resume: 6 (byte-exact offset restart, stale-ETag reset with corrupt
    local file, crash-leftover normalization, probe-failure marking,
    empty pass, DB cascade helper)

### Previously added (PR #4 — download engine)
- **Segmented download engine** (`hyprfetch-core` crate):
  - `Engine` — orchestrator that owns the task table, spawns segment workers
    per task, aggregates progress via mpsc channels, debounces SQLite
    persistence (500ms), and handles pause/cancel via a command channel
  - `HttpClient` — reqwest wrapper with HEAD probe, Range fetch, ETag /
    Last-Modified / Accept-Ranges / Content-Length parsing, redirect
    following (max 5 hops), SSRF re-check on final URL
  - `SegmentWorker` — fetches one byte range, writes via `pwrite` (positional
    write) to a shared file `Arc<File>` so segments don't need locking
  - `planner::split()` — splits total bytes into N non-overlapping segments
    (last absorbs remainder)
  - `SsrfPolicy` + `check_url()` — blocks private/loopback/link-local/
    CGNAT/unique-local IPv4+IPv6 ranges; resolves DNS for domain hosts
  - `open_target_file()` — opens file with `ftruncate` pre-allocation
- `Engine::with_ssrf_policy()` — for tests and the `--allow-private` CLI flag
- `hyprfetch-api::make_state()` helper — constructs `AppState` with engine
- `hyprfetch serve` now accepts `--allow-private` flag (default: false).
  When false (production default), SSRF protection blocks loopback and
  private IPs. Set `HYPRFETCH_ALLOW_PRIVATE=true` to disable for testing.
- API integration: `POST /api/tasks` auto-starts the engine;
  `POST /api/tasks/:id/{pause,resume,cancel}` propagate to the engine

### Previously added (PR #3 — HTTP API)
- HTTP API (`hyprfetch-api` crate, axum 0.7):
  - `GET /healthz` — `{"status":"ok"}` for liveness probes
  - `GET /api/tasks?state=active|completed|all` — list with filter
  - `POST /api/tasks` — bulk create (up to 100 URLs per request), validates
    scheme is http/https, derives filename from URL, persists to SQLite,
    appends `task.created` audit event
  - `GET /api/tasks/:id` — full task detail with per-segment progress
  - `DELETE /api/tasks/:id` — removes task + cascades segments
  - `POST /api/tasks/:id/{pause,resume,cancel}` — state machine transitions
    with `409 Conflict` on invalid transitions
  - `GET /api/qos` / `PUT /api/qos` — read/write QoS state
  - `GET /api/settings` / `PATCH /api/settings` — flat key/value settings
  - Structured error responses: `{ "error": { "code": "...", "message": "..." } }`
    with stable codes (`invalid_url`, `ssrf_blocked`, `task_not_found`,
    `invalid_state_transition`, `invalid_request`, `internal_error`)
- `hyprfetch` binary:
  - `serve` command binds axum on `127.0.0.1:7780` (configurable via
    `--bind` or `HYPRFETCH_BIND` env var)
  - `doctor` command verifies db, settings, and pragma state
  - `--db-path`, `--download-dir`, `--segments` CLI flags + env var equivalents
  - Default db path: `${XDG_DATA_HOME:-~/.local/share}/hyprfetch/hyprfetch.db`
- `hyprfetch_db::open_in_memory()` — for tests; runs migrations + applies pragmas
