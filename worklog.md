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

---

## 2026-09-29 (session 3) — private-repo self-update without a token (v0.3.2)

- **Owner report:** on their Arch PC the repo is accessed via SSH only
  (`git clone git@github.com:…`, no PAT anywhere) — `hyprfetch update
  --check` printed "unauthenticated (public repos only)" and "no published
  release found". SSH keys cannot call the GitHub REST API, but they CAN
  run git — the updater now exploits exactly that.
- **Core (`hyprfetch-core/src/update.rs`):** new git tier —
  `parse_version_tags` / `latest_version_tag` / `ls_remote_latest_tag`
  (non-interactive env: batch-mode SSH, no prompts), `git_url_candidates`
  (config `git_url` → clone origin → derived `git@github.com:<repo>.git` →
  anonymous HTTPS), `check_via_git(_sync)` → `GitCheck{via_url, tag}`,
  `clone_tag_shallow` + `find_cargo` (PATH → `~/.cargo/bin`) +
  `cargo_build_release` + `build_from_tag`. `UpdateConfig` gained
  `token_source` + `git_url`; new `UpdateError::Git`; `UpdateCheck.via_git`.
  Shared cargo-build code with `run_git_update`. 5 new unit tests incl.
  real-subprocess ls-remote/shallow-clone against a local `file://` repo.
- **CLI (`update_cmd.rs`):** two-tier flow — API check → git fallback →
  labelled auth line (`authenticated (env)` / `(gh cli)` / `(clone origin)`
  / unauthenticated), git install = shallow clone at the exact tag →
  `cargo build --release --locked` → atomic swap (`hyprfetch.old` kept) →
  daemon restart. No-auth path prints concrete unlock hints. `update` now
  loads the config file + new `--config` flag. Fixed pre-existing bug:
  `[update]` config section was ignored by the update subcommand.
- **Token chain (helpers.rs):** + `GH_TOKEN`, `gh auth token`, `git
  credential fill` (prompting disabled); clone scan returns BOTH the
  PAT-in-URL and the origin URL; every source labelled.
- **API (routes.rs):** `GET /api/update/check` falls back to the git tier;
  payload carries `via_git`; `POST /api/update/apply` returns 400 with CLI
  guidance for git-found updates (web requests cannot rebuild the binary).
- **Doctor:** shows token source + the git remote the updater will use.
- **Tests:** cargo fmt + clippy clean; **134** workspace tests green; main
  downloader E2E **43/43** green; NEW `scripts/e2e_update_tiers.sh` — 4
  scenarios / 20 checks green: (A) no-auth hints, (B) PAT tier against the
  real v0.3.1 GitHub release, (C) full git-tier install vs a local bare
  git remote (shallow clone → build → swap → verified `--version` of the
  swapped binary + rollback copy), (D) clone-origin discovery via doctor.
  E2E caught 2 real bugs pre-commit (tier-fallback `unreachable!()` panic;
  unlabelled env token) — both fixed.
- **Docs:** README "Self-update" rewritten (two tiers), install.md
  "Keeping it updated" (+ `[update] git_url` example), api.md (via_git,
  400-on-git-apply). CHANGELOG [0.3.2]; worktasks section 17.

## 2026-09-29 (session 4) — self-hosted fast update channel (v0.3.3)

- **Owner request:** use their own server (domain `https://istias.tech/`,
  origin `138.197.73.65`) so `hyprfetch update --check` is fast, and mirror
  every release there **in a NEW directory** so the existing site on that
  host (a Next.js app behind Cloudflare) keeps working untouched.
- **Core (`hyprfetch-core/src/update.rs`):** new **tier 0 — update
  channel**. `ChannelManifest`/`ChannelAsset` deserialise `latest.json`
  (`version`, `tag`, `published_at`, `notes_url`,
  `assets: {<target-triple>: {url, sha256, size}}`);
  `manifest_url` joins the base; `pick_channel_asset` matches
  `target_candidates()`; `check_via_channel` returns the shared
  `UpdateCheck` (new `via_channel` + `channel` fields, `#[serde(default)]`
  so older payloads stay compatible); `apply_channel` downloads the
  manifest-listed tarball, verifies the **manifest sha256**, extracts and
  swaps atomically via the existing `extract_binary`/`swap_binary`.
  `UpdateConfig.channel_url` (default `https://istias.tech/hyprfetch/updates/`,
  empty string disables); `UpdateError::Channel`. 4 new unit tests
  (URL join, enable/disable, parse+pick, strict schema) — 138 workspace
  tests green.
- **CLI (`update_cmd.rs`):** three-tier chain channel → API → git with a
  `Tier` enum routing installs; per-tier labels
  (`checked via update channel (<url>)` / auth label / `git access detected
  via <url>`); new `--channel <url>` flag; fast-path hint on `--check`.
  Config/env plumbing in `helpers.rs` (`[update] channel`,
  `HYPRFETCH_UPDATE_CHANNEL`, precedence flag > env > config > default);
  `doctor` prints the active channel.
- **API/UI:** `/api/update/check` tries the channel first and reports
  `via_channel`; `/api/update/apply` installs from the mirror when the
  cached check was channel-found. Updates card got a `via update channel`
  badge; `ui/dist` rebuilt (npm) and committed.
- **Release workflow:** new `deploy-update-channel` job after publish —
  generates `latest.json` from the built archives + `.sha256` files (jq),
  scp's archives into `<DEPLOY_PATH>/updates/<version>/`, swaps the
  top-level manifest **atomically** (temp name + `mv(2)`), then verifies
  the public URL serves the new version (warning + setup pointer if the
  origin route is missing). Secrets: `DEPLOY_SSH_KEY` (required),
  `DEPLOY_HOST`/`DEPLOY_USER`/`DEPLOY_PORT`/`DEPLOY_PATH` with the owner's
  server as defaults; job skips with a notice when the key is absent.
- **Docs:** new `docs/update-channel.md` (manifest schema, one-time nginx
  `location /hyprfetch/` recipe, Cloudflare notes, secrets + deploy-key
  generation, client overrides, verification); README Self-update +
  `docs/install.md` rewritten around the three tiers; `docs/api.md`
  documents the new payload fields.
- **E2E:** new `scripts/e2e_update_channel.sh` — 8 scenarios / 28 checks
  green (check via channel, full install download→sha256→swap→rollback,
  tampered-manifest refusal leaves the binary untouched, up-to-date, 404
  fallback, `--channel ""` silence, `/api/update/check` payload).
  `scripts/e2e_update_tiers.sh` updated for the channel-first reality
  (channel disabled per scenario, dead API base for the git-tier install)
  — 20/20 green again. Main downloader E2E 43/43 still green.
- **Side finding:** the GitHub repo is **public** now (anonymous
  `ls-remote` works, API unauthenticated answers past rate limits), so
  scenarios that need the dead-end UX pin `--repo fake/nonexistent-repo`
  + `--channel ""`. The fast channel is still worth it: no rate limits,
  no GitHub dependency, download stays on the owner's server.
- **Owner TODO (one-time, outside CI):** create the new directory on the
  server, add the nginx `location /hyprfetch/` route (recipe in
  docs/update-channel.md), then set the `DEPLOY_SSH_KEY` secret — the
  v0.3.3 workflow run will skip the mirror step with a notice until then
  and the updater falls back to GitHub tiers automatically.

## 2026-09-29 (session 5) — server-only updater v0.4.0 + istias.tech docs pages

**Task:** owner: (1) remove GitHub from the update flow in ANY mode — only
the own server; (2) GitHub Action sends every release to the server in a new
path; (3) new pages in Local-DE-Coach/Docs (which OWNS the istias.tech
server webUI) at /hyprfetch + /hyprfetch/updates; (4) show the latest
version on those pages; (5) clean asset names; (6) richer release notes;
server has only 500 MB RAM.

- **Docs repo (deployed, live):** found the deploy had been BROKEN since
  Sep 19 — raw `<60` in `test-results/page.tsx` JSX text failed the
  Turbopack parse. Fixed (`&lt;60`). Added `web/src/app/hyprfetch/page.tsx`
  (product page: hero + live version badge, features, per-platform install
  commands with copy buttons, 4-step test & verify guide) and
  `web/src/app/hyprfetch/updates/page.tsx` (live version panel fetched
  client-side from `/hyprfetch/updates/latest.json`, CLI update commands,
  manual per-platform steps, troubleshooting). Both are static prerender —
  zero extra RAM on the 500 MB box. deploy.yml now self-heals the HyprFetch
  update-channel route: creates `/var/www/istias.tech/hyprfetch/updates`
  (world-readable) and injects nginx locations into the existing 443 block
  (exact-match page proxy for `/hyprfetch/updates` + `^~` static alias for
  `/hyprfetch/updates/` with CORS + max-age=60). One escaping bug found by
  CI (bare `"` inside the ssh printf mangled the nginx header → `nginx -t`
  refused the reload; site stayed on the old config) — fixed with `\"`
  escaping like the existing `Connection "upgrade"` line. Verified live:
  `/hyprfetch` → 200, `/hyprfetch/updates` → 200 (manifest 404 until the
  first deploy lands — the pages degrade gracefully).
- **Updater core (`crates/hyprfetch-core/src/update.rs` rewritten):**
  channel-only. API tier, git tier, token discovery, clone scanning, PAT
  parsing, `--from-git` — all deleted. `UpdateConfig` is now just
  `{channel_url}`. `check()` = one GET of `latest.json`; `apply()` =
  download → sha256-verify (manifest) → extract → atomic swap. `UpdateCheck`
  dropped `via_git`/`via_channel`; `notes_url` now points at the updates
  page. Unit tests rewritten (9 green).
- **CLI (`update_cmd.rs`):** single-source flow. Unreachable channel → clear
  error + pointer to https://istias.tech/hyprfetch/updates + `hyprfetch
  doctor` hint; `--channel ""` disables the updater with a message. Removed
  `--repo/--token/--from-git/--source-dir` flags. doctor prints the channel.
- **helpers.rs:** `resolve_update_cfg_with_db` (token chain) replaced by
  `resolve_update_cfg(channel_flag, cfg_update)`; legacy `[update]` keys are
  accepted but ignored (serde ignores unknown fields); tests for precedence
  + explicit disable + legacy-key parsing.
- **API/UI:** `update_check` route channel-only (error payload carries
  `updates_page`); `update_apply` channel-only; Settings GitHub-token card →
  Update-channel card; Updates card copy updated; `ui/dist` rebuilt (npm).
- **Workflow (release.yml):** clean asset names (`hyprfetch-<ver>-linux-x64`
  / `-linux-arm64` / `-linux-musl-x64`; PKGBUILD + rpm templates repinned);
  manifest generation maps clean names back to target triples and points
  `notes_url` at the updates page; new "Generate release notes" step assembles
  the body with python3 (CHANGELOG `## [X.Y.Z]` section + update/install
  commands + per-platform install table + downloads table + sha256 verify +
  page links) — no shell interpolation, so changelog backticks are safe.
- **Testing:** 133 workspace unit tests green; new
  `scripts/e2e_update_channel.sh` (COMMITTED this time) — 8 scenarios /
  23 checks green against a local mock channel: check-available, up-to-date,
  full install (swap + `.old` backup + no temp leftovers), tampered-manifest
  refusal (binary untouched), unreachable-channel error (asserts NO GitHub
  fallback attempt in output), disabled mode, `/api/update/check` payload
  (channel echoed, no legacy fields), malformed manifest (no panic). Also
  `scripts/simulate_release_notes.sh` — runs the release-notes step locally
  (exact extraction of the run block from the workflow YAML) — green.
  Disk-full on the sandbox caused two false failures mid-run (mktemp empty,
  incremental compilation OOS) — cleaned target/debug + /tmp, reran with
  `CARGO_INCREMENTAL=0`, all green.
- **Docs:** README Self-update rewritten (one source, no tiers);
  `docs/update-channel.md` rewritten (routing automated by the Docs repo
  deploys; DEPLOY_SSH_KEY is the only remaining owner step); `docs/install.md`
  + `docs/api.md` updated to one-source wording, clean asset names and the
  new `/api/update/check` payload; CHANGELOG 0.4.0 section added (the
  release-notes generator extracts it verbatim).
- **Owner TODO (one-time):** add the `DEPLOY_SSH_KEY` secret to
  Local-DE-Coach/HyprFetch (org secrets are not shared to this repo) —
  until then the release workflow skips the server mirror with a notice.
- Version bumped to **0.4.0**; tag `v0.4.0` → release CI builds + publishes
  + (once the secret lands) mirrors to istias.tech.

---
Task ID: 7 (session 6 — install/uninstall hardening)
Agent: Super Z (main session)
Task: Owner follow-ups after v0.4.0 went live — Arch command used a GitHub
link, no uninstall instructions, and users need multiple download paths.

Work Log:
- Confirmed the owner fixed DEPLOY_SSH_KEY: re-run of the release workflow
  shows "Deploy update channel (istias.tech)" green; latest.json serves
  0.4.0; version panel works.
- Found + fixed a real packaging bug: PKGBUILD.bin.template still pointed
  `_srcdir` at the old `x86_64-unknown-linux-gnu` archive root (makepkg
  would fail on the renamed clean tarballs) and sourced the tarball from
  GitHub — moved to the istias.tech channel.
- New hyprfetch/install.sh in the Docs repo (channel-owned): one-line
  install `curl -fsSL https://istias.tech/hyprfetch/updates/install.sh | sh`,
  POSIX sh, no jq; `--uninstall`, `--purge`, `--version`, `--channel`.
  E2E'd against the LIVE channel in the sandbox: latest.json → linux-x64
  pick → sha256 verify → /usr/local/bin install → `--version` 0.4.0 →
  `update --check` "up to date" → `--uninstall --purge` leaves nothing.
- istias.tech pages (Docs repo): new "Four ways to get HyprFetch" install
  section (one-line script / native package / manual tarball / browser
  download), per-platform Uninstall section with real paths
  (~/.config/hyprfetch, ~/.local/share/hyprfetch, pacman/apt/dnf names),
  server .deb/.rpm/PKGBUILD rows in the version panel; every GitHub
  download link on both pages replaced with versioned istias.tech URLs.
- hyprfetch-mirror.yml (Docs): self-healing check — re-mirrors when the
  served version's PKGBUILD/.deb/.rpm/install.sh are missing; uploads
  install.sh; generates the channel PKGBUILD from the repo template +
  manifest sha256 (release-asset regressions can't reach the server).
- release.yml: install table now server-only (+ uninstall line); restored
  the `runs-on` on deploy-update-channel that a half-applied edit had
  dropped (GitHub rejected workflow parsing with 422 until fixed).
- Dispatched the Release workflow for tag v0.4.0 on the fixed main — the
  run regenerates PKGBUILD + notes in place (asset replacement by
  softprops/action-gh-release), so v0.4.0 needs no re-cut.
- Docs: CHANGELOG [Unreleased], worktasks 20.1–20.7, this worklog.

Stage Summary:
- Owner-visible: one-line install + uninstall on the pages, server-only
  commands everywhere, fixed Arch PKGBUILD (server-sourced, correct dir).
- Pending at write time: release run 36589215269 finishing, then the Docs
  mirror self-heals the server (PKGBUILD/deb/rpm/install.sh) within 30 min
  (or on manual dispatch).
