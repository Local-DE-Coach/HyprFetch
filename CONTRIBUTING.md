# Contributing to HyprFetch

Thanks for your interest in contributing. This is a small project with a clear direction; please read this guide before opening a PR.

## Project direction

HyprFetch is intentionally scoped. Before opening a feature PR, check whether it fits:

**In scope**:
- Faster / more correct download behavior
- Better QoS / bandwidth management
- Lower RAM / CPU usage
- Bug fixes for resume, segmented downloads, file integrity
- UI polish for the existing pages (Active, History, Settings)
- Cross-platform fixes (Linux primarily; macOS/BSD welcome if zero-cost)
- Documentation

**Out of scope (will be rejected)**:
- Torrent / DHT support
- Browser extension
- Cloud sync / account system
- Built-in video / audio transcoding
- Theme marketplaces / plugin systems

If unsure, open a Discussion first.

## Workflow

1. **Discuss** — open an issue or discussion for any non-trivial change
2. **Branch** — `git checkout -b feature/<name>` from latest `main`
3. **Implement** — keep PRs focused (ideally < 500 lines). Split if growing
4. **Test** — add tests for new behavior. Run `cargo fmt && cargo clippy -- -D warnings && cargo test`
5. **Document** — update `docs/` if API or behavior changes
6. **PR** — fill the template, link the issue, request review
7. **CI** — must be green. Address review comments inline
8. **Merge** — squash-merge to keep history linear

## Code standards

### Rust
- **Stable toolchain only.** No nightly features.
- **Clippy clean.** `cargo clippy --all-targets -- -D warnings` must pass
- **Formatted.** `cargo fmt --all`
- **Documented.** Every `pub` item in a library crate has a doc comment
- **Tested.** New behavior must have at least one test. Bug fixes must have a regression test
- **No `unwrap()` in production paths.** Use `?` or explicit error handling. `unwrap()` in tests is fine
- **Error types**: use `thiserror` for library errors, `anyhow` only in binaries

### Frontend (Svelte)
- TypeScript strict mode
- No `any` types
- Components are documented with a comment above `export`
- CSS is plain CSS + variables — no Tailwind, no CSS-in-JS

### SQL migrations
- One file per migration, numbered `NNN_description.sql`
- Migrations are forward-only. To undo, write a new migration that reverses it
- Test against fresh + previously-migrated DBs

## Commit messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(core): add segmented downloader with pwrite
fix(api): handle missing Content-Length on resume
docs(readme): add QoS example
chore(deps): bump tokio to 1.40
refactor(db): extract migration runner
test(core): add resume-after-restart integration test
```

When squashing on merge, the PR title becomes the commit message. Use the same format.

## Reporting bugs

Open an issue with:
1. HyprFetch version (`hyprfetch --version`)
2. OS / kernel / WM
3. Steps to reproduce
4. Expected vs actual behavior
5. Logs (`RUST_LOG=debug` output, trimmed to relevant lines)
6. The URL you were downloading (if public — otherwise just the host)

## Reporting security issues

**Do not open a public issue.** Email the maintainer directly or use GitHub's private vulnerability reporting on the repo.

## License

By contributing, you agree your contributions are licensed under the MIT license covering the project.
