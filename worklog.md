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
