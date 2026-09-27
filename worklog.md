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
