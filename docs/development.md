# Development Guide

## Prerequisites

- **Rust** 1.75+ (stable). Install via [rustup](https://rustup.rs).
- **Node** 20+ and npm. Used only for the Svelte frontend.
- **SQLite** system libraries — `apt install libsqlite3-dev` on Debian/Ubuntu, `pacman -S sqlite` on Arch.
- **pkg-config** — `apt install pkg-config` (needed by `libsqlite3-sys` build script).

## Repository layout

```
.
├── crates/
│   ├── hyprfetch/         # binary
│   ├── hyprfetch-core/    # download engine
│   ├── hyprfetch-db/      # SQLite + migrations
│   └── hyprfetch-api/     # axum HTTP + WebSocket
├── web/                   # Svelte SPA (built → embedded in binary)
├── docs/
├── .github/workflows/
└── scripts/               # dev helpers
```

## First-time setup

```bash
git clone https://github.com/Local-DE-Coach/HyprFetch.git
cd HyprFetch
cargo build            # builds all crates
cargo test             # runs all unit + integration tests
```

For frontend dev:

```bash
cd web
npm install
npm run build          # outputs to web/dist/, embedded at compile time
# OR
npm run dev            # Vite dev server on :5173, proxies /api and /ws to :7780
```

When `web/dist/` exists at compile time, `hyprfetch-api` embeds it via `rust-embed`. When it doesn't exist, the binary serves a "build the frontend" placeholder page.

## Running

```bash
# Backend with hot reload (uses cargo-watch if installed)
cargo watch -x 'run -- serve'

# Or plain
cargo run -- serve

# With custom config
cargo run -- serve --config path/to/config.toml

# With flags
cargo run -- serve --bind 127.0.0.1:7780 --download-dir ~/Downloads --segments 8
```

Open `http://127.0.0.1:7780` in your browser.

## Testing

```bash
# All tests
cargo test

# One crate
cargo test -p hyprfetch-core

# Integration tests only
cargo test --test '*'

# With logs
RUST_LOG=debug cargo test -- --nocapture
```

The `hyprfetch-core` engine has integration tests that spin up a local HTTP server (via `wiremock` or `axum` test util) and verify segmented download + resume behavior end-to-end.

## Code style

- `cargo fmt` is enforced by CI. Run `cargo fmt --all` before committing.
- `cargo clippy -- -D warnings` is also enforced by CI. No warnings allowed on `main`.
- Public API docs are required for all `pub` items in libraries (`hyprfetch-core`, `hyprfetch-db`, `hyprfetch-api`). Use `cargo doc --no-deps -p <crate>` to verify.

## Git workflow

We use a feature-branch + PR model. **No commits land on `main` directly** (enforced by branch protection once enabled).

### Branch naming

- `feature/<short-name>` — new functionality (e.g. `feature/segmented-downloader`)
- `fix/<short-name>` — bug fix (e.g. `fix/race-on-pause`)

### Commit messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(core): add segmented downloader with pwrite
fix(api): handle missing Content-Length on resume
docs(readme): add QoS example
chore(deps): bump tokio to 1.40
refactor(db): extract migration runner
test(core): add resume-after-restart integration test
```

### Pull request flow

1. `git checkout -b feature/my-feature`
2. Implement + write tests
3. `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test`
4. Push the branch
5. Open a PR against `main`
6. CI runs: fmt check, clippy, unit tests, integration tests, `cargo audit`
7. Squash-merge after CI is green

### Releases

Tags follow `vMAJOR.MINOR.PATCH`. While pre-1.0, MINOR bumps are allowed to break APIs.

The `release.yml` workflow runs on tag push and produces:
- `hyprfetch-x86_64-unknown-linux-gnu` (static-linked binary tarball)
- `hyprfetch-aarch64-unknown-linux-gnu`
- SHA256 checksums
- A GitHub Release with the above attached

## Adding a new crate

If you need to split functionality into a new crate:

1. `cargo new --lib crates/hyprfetch-<name>`
2. Add to root `Cargo.toml` workspace `members` list
3. Document the crate's purpose in `docs/architecture.md`
4. Add to CI matrix if it needs special test setup

## Debugging

- `RUST_LOG=hyprfetch=debug,hyprfetch_core=trace cargo run -- serve` — verbose logs
- `RUST_LOG=hyper=debug` — see HTTP traffic in detail
- SQLite: open the DB file with `sqlite3 ~/.local/share/hyprfetch/hyprfetch.db` and `.tables`
- To trace tokio tasks: enable `tokio-console` (build with `--features tokio/tracing`)

## Profiling RAM

```bash
# Build release with debug symbols
cargo build --release

# Run under heaptrack
heaptrack ./target/release/hyprfetch serve

# Or under valgrind massif (slower, more accurate)
valgrind --tool=massif ./target/release/hyprfetch serve
ms_print massif.out.* | less
```

Idle RSS target: < 10 MB. If it's higher, suspect:
- `reqwest` keeping a connection pool alive
- SQLite page cache too high (default 2 MB; can lower to 256 KB)
- Tokio worker stack size (default 2 MB × N workers — can override)
