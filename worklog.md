# Worklog — multi-sandbox coordination file

This file is the **shared session log for every sandbox / agent / human** that works
on the HyprFetch repository. Its purpose is simple: when the repo is used by multiple
sandboxes (AI agents, CI bots, or developers on different machines), each one can read
this file before starting work and instantly know **what was already done, why, and in
which commit**.

## Protocol (MUST follow)

1. **Every commit updates this file.** A commit that does not touch `worklog.md` and
   `CHANGELOG.md` is considered incomplete. Docs-only commits still append a log line.
2. **Append-only.** Never rewrite or delete previous entries — later sandboxes rely on
   the history. Add the new entry at the **top** of the Sessions section.
3. **Read before you start.** Before making changes, read the latest entries to avoid
   redoing or conflicting with previous work.
4. **Reference commits.** Every entry must reference the commit hash(es) it produced
   (fill in after committing, or amend before push).
5. **Never commit secrets.** Tokens, passwords, private keys are forbidden in this
   file (and everywhere else in the repo).

### Entry template

```markdown
### [YYYY-MM-DD] <short title> — <author/sandbox name>
- **Commit(s):** <hash(es) + subject>
- **Did:** <what was changed, files touched>
- **Why:** <root cause / motivation>
- **Result / state:** <what works now, what is left, how to verify>
- **Notes for next sandbox:** <pitfalls, follow-ups, ongoing decisions>
```

## Sessions

### [2026-09-29] v0.3.1 RE-CUT — multi-page UI, category folders, zero-config clone updates, 43/43 + 1 GiB E2E — Super Z sandbox
- **Commit(s):** (this commit) — re-tag `v0.3.1` after CI build; supersedes the earlier `v0.3.1` cut.
- **Did (owner feedback applied — "aply it, test before commit"):**
  - **Multi-page web UI** (`ui/src/lib/{router,store,format}.js`, `lib/TaskCard.svelte`,
    `pages/{Dashboard,Tasks,Settings,Updates}.svelte`, rewritten `App.svelte`): hash-routed
    4-page SPA — Dashboard (speed, per-state counters, active downloads, recent finished,
    folder overview), Tasks (filter/search table + per-task controls), Settings (folders,
    auto-sort, queue, QoS, SSRF, GitHub token), Updates (version check + install & restart).
    Still pure Svelte + rust-embed — DaisyUI/Tailwind remain build-time only (no SvelteKit
    runtime; keeps RAM minimal).
  - **Category save folders** (`hyprfetch-core/src/categories.rs`, routes, binary startup):
    7 folders `video/pictures/music/compress/documents/apps/other` under `~/Downloads`,
    auto-created at startup / settings change / task creation; extension-based auto-sort
    (default on), explicit `category` per task, `save_dir` direct-save wins, per-category
    `category_dir_<name>` overrides; `GET /api/categories`; `category` field on task DTOs;
    default base dir `~/Desktop` → `~/Downloads`.
  - **Zero-config updates for clone installs** (`update.rs::parse_pat_from_git_url`,
    `helpers.rs::detect_token_from_source_clones`): when no token is configured the updater
    reads the PAT from the local clone's origin URL (`~/HyprFetch`, `~/Projects/…`,
    `~/src/…`, `~/code/…`, `~/Developer/…`, `[update] source_dir`). Verified live:
    authenticated `update --check` with only a clone in `$HOME`. Also: `github_token`
    settable from the UI settings and **masked** on `GET /api/settings` (`<key>_set`).
  - **Docs:** README defaults, `docs/install.md` (defaults table + update token chain),
    `docs/api.md` (category resolution, categories endpoint, settings masking).
  - **E2E rebuilt after full sandbox recycle** (rustup reinstalled, repo re-cloned):
    `scripts/e2e_full_test.py` 43 checks green (incl. categories, direct save, masking,
    WS frames, restart hand-off, cold-restart resume), `scripts/e2e_large_test.py` 4/4
    (1 GiB sha256-exact, mid-flight pause/resume, 16 MB/s loopback).
- **Why:** owner reported the web UI had only one page, wanted per-type save folders
  (auto-created), a working `hyprfetch update` for a clone-based install, and the update
  version visible in the UI; earlier v0.3.1 cut shipped run modes/updater/sleep mode but
  not these, so v0.3.1 is re-cut to be the true "last webUI+backend" release.
- **Result / state:** cargo test 130 green; clippy `-D warnings` clean; fmt clean;
  E2E 43/43 + 4/4. All owner requests from the latest message implemented and tested.
- **Notes for next sandbox:** UI dist is committed (rust-embed) — run `npm ci && npm run
  build` in `crates/hyprfetch-api/ui/` after UI edits, then `cargo build`. Unit tests pin
  `HOME=/tmp/hf-test-home` in the two HOME-touching `categories.rs` tests (parallel-safe).
  E2E uses ports 7783/7893 (full) and 7785/7895 (large) — set `HF_PORT`/`HF_FSRV_PORT`
  to override. The updater's clone scan reads `git config remote.origin.url` only; it
  never logs the token.

### [2026-09-29] v0.3.1 — run modes + in-app updater APPLIED, DaisyUI UI, sleep mode, full E2E (73/73) — Super Z sandbox
- **Commit(s):** (this commit) — tag `v0.3.1`.
- **Did:**
  - Owner asked to stop describing and **apply** the §15 plan: "last commit
    say i description it roadmap but i say apply it". Also asked for a
    v0.3.0 release (none existed — verified via API: only v0.2.0/v0.1.1/
    v0.1.0), a beautiful release page, SvelteKit+DaisyUI for the UI, and a
    leaner resource profile ("not always active, sleep, less RAM than a
    normal download manager").
  - **R1 run modes (applied):** `hyprfetch dev` (pretty debug logs, auto-open
    UI), `serve --mode dev|prod --open --workers N --exit-when-idle min`,
    `daemon start|stop|restart|status` (detached process group, JSON PID
    file, 5 MiB × 3 rotating logs under `~/.local/state/hyprfetch/logs/`,
    start waits for `/healthz`), `logs [-f] [-n N]` tail/follow. New modules:
    `crates/hyprfetch/src/{daemon.rs,logger.rs,helpers.rs,update_cmd.rs}`.
  - **R2 updater (applied):** `hyprfetch-core/src/update.rs` — release check
    (PAT-aware, fine-grained PAT ok), target-triple asset matching, API
    octet-stream download, sha256 verify, tarball extraction hardened
    (regular-file `hyprfetch` only), atomic swap with `.old` rollback;
    source mode via `--from-git` (pull → build → swap). REST:
    `GET /api/update/check`, `POST /api/update/apply?restart=…`,
    `POST /api/update/restart` (drain-pause → re-exec → auto-resume);
    `SERVE_ARGS` OnceLock carries the original serve args. UI: **Updates**
    card (check / install & restart / restart).
  - **Resource mode:** tokio workers default 2 (`--workers`, config, env);
    `--exit-when-idle <min>` graceful-exit watchdog (tick 1–30 s, scales with
    budget); WS client counter in `AppState`; measured idle RSS **8.0 MB**,
    9.4 MB during download, no leak.
  - **`GET /api/server`:** version / uptime / active tasks / ws clients /
    cached update flag.
  - **UI restyle:** Tailwind 3 + DaisyUI 4 (`dim` theme) on the existing
    Svelte 4 SPA (SvelteKit adds SSR weight for zero benefit inside an
    embedded binary — documented choice); bundle ≈ 18 KiB gzipped; committed
    rebuilt `ui/dist`.
  - **Fixed en route:** restart port hand-off race (bind retry ≤ 15 s in
    `serve_with_token`); reqwest `json`+`blocking` features; workspace
    version → 0.3.1.
  - **Tests:** workspace 117/117; clippy clean; fmt clean; E2E suite
    recreated + extended at `/scripts` (sandbox-side): **73/73** incl. mock
    GitHub updater flow (check/apply/restore, in-app restart spawns NEW
    process, CLI `update --check`/`--yes`), daemon lifecycle 7/7,
    exit-when-idle, real network 10 MB + 50 MB, RSS footprint phase.
  - **Docs:** CHANGELOG §0.3.1; worktasks §15 all ✅ (+15.6–15.10);
    feature-research §12 ✅ + roadmap SHIPPED note; README (Features, Run
    modes, Self-update, Configuration, Security); docs/api.md (server info +
    in-app updates); docs/install.md (Keeping it updated).

### [2026-09-29] Feature-research comparison doc + v0.3.0 roadmap (run modes, in-app updater) — Super Z sandbox
- **Commit(s):** (this commit).
- **Did:**
  - Owner uploaded `Linux-Download-Manager-Research.md` +
    `Feature-Tree-Diagram.png` and asked for a docs checklist marking each
    feature HyprFetch already has vs the ones still missing ("without
    change"), plus a next-version plan for Node.js-style run modes and an
    in-app updater.
  - **Upload never reached the sandbox** (upload dir empty; web search found
    no public copy) — documented transparently in the doc's source note;
    rebuilt the feature list from the standard Linux-DM research dimensions
    and verified every line against code/tests/E2E (commit `ddfdd41`).
  - New `docs/feature-research.md`: 12 areas, ✅/🟨/⬜ per feature with
    concrete proofs; snapshot count table; roadmap section.
  - Roadmap recorded: **R1 run modes** (`dev` console-debug mode, `--daemon`
    + PID/log files, `logs/status/stop/restart` subcommands) and **R2
    in-app updater** (release/clone check → apply from source or binary →
    sha256 verify → drain-pause restart → auto-resume).
  - `worktasks.md` §15 rows 15.1–15.5 (all ⬜, nothing implemented);
    `CHANGELOG.md` Docs + Planned entries.
- **Proof:** `docs/feature-research.md` snapshot table; absence checks in
  code for proxy/cookie/torrent/metalink/checksum/notifications (0 hits).
- **Notes for next sandbox:** the owner may re-upload the original research
  file — merge its exact wording into `docs/feature-research.md` without
  altering the status marks; the PNG diagram also never arrived.

### [2026-09-29] Desktop integration + default dir ~/Desktop; full E2E re-run green; v0.2.0 re-cut — Super Z sandbox
- **Commit(s):** `4a6de25` feat(desktop) + this docs commit; tag `v0.2.0`
  re-cut at the feat commit (see below).
- **Did:**
  - Remote re-inventory: only `origin/main` remains — every feature /
    Dependabot branch from the earlier hygiene passes is merged and gone
    (axum 0.8.9 migration + resume-race fix are on main).
  - **Default download dir changed to `~/Desktop`** (was `~/Downloads`)
    per owner request — `routes.rs` built-in fallback, docs updated.
  - **Desktop entry added** (`packaging/desktop/hyprfetch.desktop`):
    release tarball ships it; Arch PKGBUILD, .rpm spec and .deb assets all
    install it to `/usr/share/applications/` — shows in desktop menus.
  - E2E harness hardening: hermetic `--db-path` per run (a crashed run
    leaves resumable tasks that auto-resume on next startup and break the
    WS idle-silence check), `HF_E2E_SKIP_LARGE` phase flag, standalone
    `1GB` phase script; fixed hardcoded 1 GiB size (thinkbroadband's
    `1GB.zip` is actually 1,073,725,334 B — trust Content-Length).
  - **Full E2E green:** 29/29 (UI assets, SPA fallback, API 404 no-leak,
    error paths, local Range sha256, real-network 10MB + 50MB byte-exact,
    pause→resume sha256, QoS 1 MiB/s cap timing, WS live events + idle
    silence (0 frames), DELETE ?delete_file=true) + 3/3 large phase
    (1GB exact size + head/tail 1MiB sha256 byte-exact).
  - Unit: fmt + clippy `-D warnings` + 111 tests green.
  - Tag `v0.2.0` re-pointed from the premature cut (`14f2f5b`) to this
    state so release assets include axum 0.8 + race fix + desktop entry;
    release notes now embed the copy-paste Arch install commands
    (only `<YOUR_PAT>` needs changing).
- **Proof:** E2E summary lines `29/29` / `LARGE-PHASE: PASS`; `hyprfetch
  --version` → 0.2.0; CI green on main.

### [2026-09-28] Merged final 2 Dependabot branches (axum 0.8!), fixed resume-race bug, 30/30 live E2E — Super Z sandbox
- **Commit(s):** merge `e761928` (axum 0.8.9 — amended with `{id}` route fix +
  WS `Utf8Bytes` fix), merge `23e5b93` (tokio-tungstenite 0.30.0, Cargo.lock
  conflict resolved by re-resolve); (this commit) resume-race fix +
  regression test + work files.
- **Did:**
  - Two NEW Dependabot branches had appeared after the previous batch (they
    were only offered once thiserror/tungstenite landed): `axum 0.7.9 → 0.8.9`
    and `tokio-tungstenite 0.24 → 0.30.0`. Merged both; remote again has ONLY
    `main`.
  - **axum 0.8 code fixes (merge commit):** all 5 `/api/tasks/:id/...` routes
    migrated to the new `{id}` syntax (old syntax PANICS at startup on axum
    0.8 — a compile-clean but runtime-fatal trap), and the WS sink adapted to
    `Message::Text(Utf8Bytes)`.
  - **Real bug found by the live E2E:** resume immediately after pause
    stranded the task in `downloading` forever. Root cause: pause is async;
    the resume route set the DB row to `downloading` and then
    `engine.start()` rejected that state unconditionally, swallowed the
    error, and returned 2xx. Fix (core+api): engine treats a `downloading`
    row with no live coordinator as restartable; the resume route retries
    through the wind-down window and on persistent failure rolls back to
    `paused` + returns 409. Regression test added:
    `resume_immediately_after_pause_does_not_strand_task` (wiremock with a
    dynamic Range responder, zero-delay pause→resume, must complete).
  - **Verified:** fmt + clippy `-D warnings` clean; **111/111 tests**; live
    E2E **30/30** against the release binary: UI assets + SPA fallback +
    retry/delete bundle, byte-exact sha256 on local 10 MB and network
    10 MB, 50 MB pause→resume→complete byte-exact, **1 GB large download
    completed in 122 s** (exact size + head/tail 1 MiB sha256), QoS 1 MiB/s
    cap accurate (10 MB in 9.0 s), WS progress/state/global-speed events
    received live during download, idle silence confirmed, delete-with-file
    removes the file, all error paths correct (400/404/409).
- **Result / state:** `main` is the only branch; everything merged, fixed,
  tested. E2E harness (sandbox-only, not committed) at
  `/home/z/my-project/scripts/e2e_full_test.py`.
- **Notes for next sandbox:** axum 0.8's `:id` → `{id}` migration is easy to
  miss because it fails at RUNTIME (router panic on startup), not compile
  time — keep the route-syntax check in mind for future axum upgrades. The
  resume rollback path returns 409 `invalid_state_transition`; UI treats
  non-2xx as failure so a retried tap works — acceptable. thinkbroadband
  blocks the python-urllib UA with 403; use curl for reference downloads in
  sandbox harnesses.

### [2026-09-28] Branch hygiene: merged all 10 Dependabot branches, fixed Cargo.lock, deleted branches + closed PRs — Super Z sandbox
- **Commit(s):** merge commits `36be434` (stale@v11), `17601b0` (upload-artifact@v7),
  `4a906bc` (download-artifact@v8), `95e6c17` (checkout@v7), `3751c42`
  (action-gh-release@v3), `8075f8b` (thiserror 2.0.21), `0454a8f` (toml 1.1.6),
  `24d7814` (tokio-tungstenite 0.24.0), `100a4bc` (rusqlite 0.40.2), `a79d93d`
  (governor 0.10.4, amended to repair Cargo.lock); (this commit) work-file updates.
- **Did:**
  - Inventoried the 10 open Dependabot branches (5 cargo + 5 GitHub Actions),
    all based on v0.2.0 / the audit commit; merged them into `main` one by one,
    running `cargo check --workspace --all-targets` after each cargo bump.
  - **Fix:** the governor merge textually auto-merged `Cargo.lock` into an
    unparseable state (`package hashbrown is specified twice` — duplicated
    stanza from the branch's lock). Repaired by restoring the previous good
    lock and letting cargo re-resolve governor minimally; amended the merge
    commit. Verified the final lock has no same name+version duplicates.
  - All 5 Actions bumps validated by YAML parse + `uses:` review
    (checkout v7 ×5, upload-artifact v7, download-artifact v8, stale v11,
    action-gh-release v3 — all drop-in, no schema changes needed).
  - **Zero code changes needed** for the 5 cargo bumps: the APIs we use
    (`DefaultDirectRateLimiter::direct`, `Quota::per_second`, rusqlite
    connection/params, thiserror derives, `toml::from_str`,
    `connect_async`/`Message::Text`) are stable across each jump.
  - **Verified:** `cargo fmt` clean, `cargo clippy --all-targets -- -D warnings`
    clean, **110/110 tests passing** (same count as pre-merge, includes ws e2e
    over real TCP with tungstenite 0.24 client), live smoke test (config.toml
    parsed by toml 1.1 → settings applied; SQLite opened WAL via rusqlite 0.40;
    a real download completed through the governor 0.10 token bucket; doctor OK).
- **Result / state:** `main` at `a79d93d` contains all 10 bumps; remote has
  exactly one branch (`main`); all 10 PRs (#10–#19) closed with an explanatory
  comment (Dependabot auto-deleted its branches on close). CI green on the
  merge train.
- **Notes for next sandbox:** Dependabot will NOT reopen these — versions in
  `main` now satisfy its checks. `thiserror 1.0.69` still appears in
  `Cargo.lock` as a transitive dep of another crate — that is normal and not
  actionable. If CI fails on `softprops/action-gh-release@v3` at the next tag,
  check its changelog for input renames (v2→v3 may deprecate inputs; not
  verifiable locally).

### [2026-09-28] Full audit → worktasks.md board, 3 engine bugs fixed, retry/auth/config shipped — Super Z sandbox
- **Commit(s):** (this commit) `fix(core)+feat(api,cli): audit fixes — task reaping, queue pump, non-Range 200 fallback, retry, delete_file, bearer auth, config.toml, worktasks.md`
- **Did:**
  - **Audit**: read all ~5.6k lines of source vs docs (api.md/architecture.md/
    design.md); ran the 93-test suite, clippy, fmt; live smoke tests of the
    release binary (loopback + 0.0.0.0 + config file + auth).
  - **Bugs fixed (core):** (1) finished tasks never reaped from the engine
    map (leak + wrong is_running + "already running" on restart);
    (2) downloads from non-Range servers always failed — worker required 206
    even for the single-connection fallback, now accepts 200 whole-file
    bodies at segment start with write-capping; (3) queue double-spawn race
    → spurious `removed` states (check-and-insert atomic spawn + per-pass id
    filter).
  - **Missing features added:** `max_concurrent_tasks` queue pump (oldest
    first, chain-start on completion, 0=unlimited, manual resume bypasses);
    `POST /api/tasks/:id/retry` (error→queued, offsets kept, remote-change
    validated); `DELETE ?delete_file=true` really removes the file;
    bearer auth on non-loopback binds (header or `?access_token=`, 401 +
    WWW-Authenticate, healthz/SPA open, token auto-generated to
    `~/.config/hyprfetch/token` 0600); config.toml loading with `--config`
    and CLI > env > file > default; `user_agent` + `ssrf_block_private`
    settings honored; in-session remote-change recheck on task (re)start.
  - **UI:** Retry buttons (Active error + Finished lists), Finished ✕ now
    `?delete_file=true`; `ui/dist` rebuilt via npm and committed.
  - **Docs:** api.md (auth reality, retry, delete_file, settings enforcement
    table), README (config keys, known-gaps ledger updated),
    **worktasks.md** — new project task board (per-area status with proof +
    verify commands, bug tracker, backlog, definition-of-done protocol).
  - **Tests:** 93 → **110 passing**; clippy `-D warnings` clean; fmt clean.
- **Why:** user asked to test everything, fix what doesn't work, add what's
  missing, review the architecture, and create a worktasks.md to manage the
  project tasks.
- **Result / state:** all green; live smoke re-run after fixes: non-Range
  download completes byte-exact, retry clears error + re-queues, delete_file
  removes the file, 0.0.0.0 bind enforces the token (401/200), config file
  drives bind/download_dir/segments. Remaining gaps are tracked in
  worktasks.md (max_connections, protocol_pref, stats/history/settings pages,
  checksums, arm64 deb, etc.).
- **Notes for next sandbox:** start from worktasks.md — pick any ⬜ row and
  follow its Verify command + the definition-of-done checklist at the top.
  Engine map/pump invariants: never call spawn_coordinator twice for the
  same id without checking the map; reap happens in the wrapper task. UI
  changes must be followed by `npm run build` in crates/hyprfetch-api/ui and
  the rebuilt dist committed.

### [2026-09-28] Release v0.2.0 — Super Z sandbox
- **Commit(s):** (this commit) `chore(release): v0.2.0` + tag `v0.2.0`
- **Did:** workspace version 0.1.1 → 0.2.0 (all 4 member crates via
  `version.workspace`, Cargo.lock refreshed with `cargo update -w`),
  CHANGELOG `[Unreleased]` finalized as `[0.2.0] — 2026-09-28`. Tag `v0.2.0`
  pushed to trigger the release workflow on the tag-push path.
- **Why:** ship the packaging feature as a real release so the GitHub release
  page carries the new `.deb` / `.rpm` / `PKGBUILD` artifacts.
- **Result / state:** expected v0.2.0 release artifacts:
  `hyprfetch-0.2.0-{x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu,
  x86_64-unknown-linux-musl}.tar.gz` + `.sha256`,
  `hyprfetch_0.2.0-1_amd64.deb`, `hyprfetch-0.2.0-1.x86_64.rpm`, and
  `PKGBUILD` (Arch, pinned to 0.2.0 + tarball sha256). Verified locally
  before tagging: build + all 93 tests pass, binary reports `hyprfetch 0.2.0`.
- **Notes for next sandbox:** if the release run fails, check the two new
  x86_64 job steps first ("Build .deb" needs cargo-deb; "Build .rpm" needs
  the apt `rpm` package) and the release job's PKGBUILD generation step —
  all validated locally, but CI runners are the real test. Tag and workspace
  version must always move together (rule now documented in
  docs/development.md).

### [2026-09-28] Docs: install guide + design rationale, drift fixes, dependabot — Super Z sandbox
- **Commit(s):** (this commit) `docs: add install + design docs, fix UI-path drift, add dependabot`
- **Did:**
  - `docs/install.md`: new end-to-end install guide (deb/rpm/PKGBUILD/tarball/
    source), first-run flags, `ulimit` note.
  - `docs/design.md`: new design-rationale doc (why Rust, architecture,
    resume, segmentation, QoS, crates, prior art, UI pages, WS contract,
    gotchas, v1→v2 roadmap with [done]/[planned] markers).
  - README: added "Install" section, fixed UI path to
    `crates/hyprfetch-api/ui/`, Rust requirement 1.75+ → 1.85+,
    project-layout tree now shows packaging/ and the real UI dir.
  - `docs/development.md`: same path fixes; "Releases" section now lists the
    new artifacts + the tag-version rule.
  - `.github/dependabot.yml`: weekly cargo + github-actions updates.
- **Why:** user asked to check the docs folder "has everything" and the
  `.github` folder completeness; README/development.md still referenced a
  `web/` directory that never existed in this layout.
- **Result / state:** docs now cover what (architecture.md) / why
  (design.md) / how to install (install.md) / how to develop
  (development.md) / HTTP surface (api.md); `.github` has CI + release +
  stale workflows, issue/PR templates, and dependabot.
- **Notes for next sandbox:** next commit bumps 0.1.1 → 0.2.0 and tags
  `v0.2.0` to produce the first release with packages. Keep
  `docs/install.md` filenames in sync with release.yml globs
  (`hyprfetch_<ver>-1_amd64.deb`, `hyprfetch-<ver>-1.x86_64.rpm`).

### [2026-09-28] Release packaging: .deb / .rpm / PKGBUILD — Super Z sandbox
- **Commit(s):** (this commit) `feat(packaging): add .deb, .rpm, and Arch PKGBUILD to release workflow`
- **Did:**
  - `crates/hyprfetch/Cargo.toml`: added `[package.metadata.deb]` (maintainer,
    section `net`, extended-description, assets = binary + README/CHANGELOG/
    copyright) so `cargo deb` produces the Ubuntu/Debian package; added
    `description` to the workspace `Cargo.toml` (cargo-deb warns without it).
  - `packaging/rpm/hyprfetch.spec`: new RPM spec (source = the x86_64 release
    tarball; `__VERSION__` substituted by CI; binary → `/usr/bin`, docs →
    `%_docdir`, license → `%_licensedir`).
  - `packaging/arch/PKGBUILD.bin.template`: new Arch template (`hyprfetch-bin`,
    downloads the release tarball; `__VERSION__` + `__SHA256__` substituted by
    CI so `makepkg` verifies the checksum).
  - `.github/workflows/release.yml`: x86_64-unknown-linux-gnu matrix job now
    also builds the `.deb` (`cargo deb --no-build --target …`) and the `.rpm`
    (`rpmbuild -bb` over the tarball made by the Package step); release job
    generates `PKGBUILD` from the template (version + sha256 of the tarball)
    and attaches it; upload-artifact globs extended with `hyprfetch_*.deb` and
    `hyprfetch-*.rpm`.
- **Why:** user asked the release to carry ready-made packages for
  Ubuntu/Debian, Fedora/RHEL, and Arch (AUR-style fast install) instead of
  only raw tarballs.
- **Result / state:** verified locally in the sandbox: full workspace builds
  (`cargo build --release --locked`), all 93 tests pass, `cargo deb
  --no-build --target x86_64-unknown-linux-gnu` produces
  `target/<triple>/debian/hyprfetch_0.1.1-1_amd64.deb` (contents inspected
  with dpkg-deb: `/usr/bin/hyprfetch` + docs, `Depends: libc6`), spec/PKGBUILD
  substitution + tarball layout simulated with the exact CI commands
  (rpmbuild itself is not installable in this sandbox — no sudo — the spec is
  standard and validated structurally).
- **Notes for next sandbox:** the release job `fail_on_unmatched_files: true`
  now also expects `PKGBUILD` at repo root — keep the template path
  `packaging/arch/PKGBUILD.bin.template` in sync with the workflow sed step.
  deb/rpm build only on the native x86_64 GNU target (aarch64/musl jobs
  unchanged). Docs commit follows: install guide + design rationale.

### [2026-09-28] Release v0.1.1 — Super Z sandbox
- **Commit(s):** (this commit) `chore(release): v0.1.1` + tag `v0.1.1`
- **Did:** workspace version 0.1.0 → 0.1.1 (all 4 member crates via
  `version.workspace`, Cargo.lock refreshed with `cargo update -w`), CHANGELOG
  `[Unreleased]` finalized as `[0.1.1] — 2026-09-28`. Tag `v0.1.1` pushed to
  trigger the release workflow from the tag path (v0.1.0 artifacts were
  produced via `workflow_dispatch` from `main` while the tag itself pointed at
  the pre-fix commit).
- **Why:** ship the aarch64 CI fix + the two engine fixes as a proper tagged
  release and prove the tag-push release path works end-to-end.
- **Result / state:** see the v0.1.1 GitHub release — expected artifacts:
  `hyprfetch-0.1.1-{x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu,
  x86_64-unknown-linux-musl}.tar.gz` + `.sha256`.
- **Notes for next sandbox:** release workflow requires
  `contents: write` for the publish job and the PAT needs workflow scope to
  push `.github/` changes; both verified working in this session.

### [2026-09-28] README: sandbox test data & analysis — Super Z sandbox
- **Commit(s):** (this commit) `docs(readme): add sandbox test results and analysis`
- **Did:** README gained a full "Sandbox test results" section: environment,
  results table (1 GB QoS-capped run, 5 GB two-phase pause/resume run, idle RSS,
  UI serving), byte-exact integrity verification (sha256 vs independent curl
  range downloads), the root-cause analysis of the two engine bugs fixed in the
  previous commit, observations (segment scaling, QoS accuracy), and known gaps.
- **Why:** the user asked for the sandbox tests + analysis to be recorded in the
  README as test data.
- **Result / state:** docs only; no code changes. Raw JSON time-series stay in
  the sandbox (not committed — 5 × ~50 KB of samples).
- **Notes for next sandbox:** when implementing `/api/tasks/:id/retry`, update
  both `docs/api.md` (remove "planned" note) and the README "Known gaps" list.

### [2026-09-28] Sandbox download tests → two engine bugs found & fixed — Super Z sandbox
- **Commit(s):** (this commit) `fix(core): remove 30s total request timeout, add per-segment retry`
- **Did:**
  - `crates/hyprfetch-core/src/http_client.rs`: removed reqwest client-level
    `.timeout(30s)` (covers the whole body stream → killed every segment at ~30 s);
    now `connect_timeout(10s)` + `read_timeout(30s)`.
  - `crates/hyprfetch-core/src/segment.rs`: `SegmentWorker::run()` retries transient
    errors (max 6 attempts, backoff 1→15 s) from the last written byte offset;
    added `is_retryable()` classification + `backoff_delay()` + 2 unit tests.
  - `docs/api.md`: fixed list-tasks example drift, marked unimplemented `/retry`.
- **Why:** sandbox tests against thinkbroadband test files exposed that ANY download
  whose segments take > 30 s failed (`only 0 of 8 segments completed`); after the
  timeout fix, real-world connection drops still failed whole tasks because a single
  worker error was fatal. Full data in `README.md` → "Sandbox test results".
- **Result / state:** 1 GB and 5 GB downloads complete end-to-end; byte-exact
  verification vs independent curl range downloads (sha256 match); pause →
  server restart → startup auto-resume from persisted offsets works
  (`resumed=1` log line); QoS 4 MiB/s cap holds within ±2% over a full download.
- **Notes for next sandbox:** test artifacts (time-series JSON, harness scripts)
  live in the sandbox at `/home/z/my-project/testdata` and
  `/home/z/my-project/scripts` (not committed). `DELETE /api/tasks/:id?delete_file=true`
  returns an empty body (harness JSON parse "fails" — it's a 204, not an error).
  Files at `ipv4.download.thinkbroadband.com` are raw random data named `.zip`
  (not real zip archives) — verify integrity by size + range sha256, not `unzip -t`.

### [2026-09-28] Fix aarch64 release build (CI) — Super Z sandbox
- **Commit(s):** (this commit) `fix(ci): wire aarch64 cross-linker into release build`
- **Did:**
  - `.github/workflows/release.yml`: added `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc`
    plus `CC_aarch64_unknown_linux_gnu` / `AR_aarch64_unknown_linux_gnu` to the *Build release* step.
  - Fixed workflow_dispatch packaging: `VERSION` and the release `tag_name` now resolve to
    `inputs.tag || github.ref_name` (previously a dispatch from `main` would archive as
    `hyprfetch-main-*` and try to publish a release named `main`).
  - Added this worklog protocol and the CHANGELOG `[Unreleased]` entry.
- **Why:** the v0.1.0 tag release run failed only on the `aarch64-unknown-linux-gnu` job.
  The workflow installed `gcc-aarch64-linux-gnu` but never told Cargo/rustc to use it, so
  the final link ran through the host `cc` with rustc's self-contained lld wrappers
  (`-B .../x86_64-unknown-linux-gnu/bin/gcc-ld -fuse-ld=lld`). lld selected the **x86_64**
  emulation from the host crt objects and aborted on the AArch64-only errata flag:
  `rust-lld: error: --fix-cortex-a53-843419 is only supported on AArch64`.
  Verified via Actions run 36329246449: `Build x86_64-unknown-linux-gnu` ✅,
  `Build x86_64-unknown-linux-musl` ✅, `Build aarch64-unknown-linux-gnu` ❌,
  `Publish release` skipped.
- **Result / state:** aarch64 cross-link is now deterministic. The release workflow was
  re-triggered via `workflow_dispatch` (tag `v0.1.0`) to repopulate the failed v0.1.0
  release — see next entries for the outcome.
- **Notes for next sandbox:** the repo is **private**, so free `ubuntu-24.04-arm` hosted
  runners are NOT available — keep the cross-compile approach. Sandbox download tests
  (thinkbroadband 1GB/5GB) and their data are being added to README.md in a follow-up
  commit. `https://ipv4.download.thinkbroadband.com/...` serves a TLS cert that does not
  match the hostname — it fails hostname verification by design; use the
  `http://...:8080` variant for real downloads.
