# Installation

HyprFetch ships as a single static-ish binary with an embedded web UI. Pick
the installation path for your distribution below; all of them end with the
same result — a `hyprfetch` executable on your `PATH` that serves the UI on
`127.0.0.1:7780`.

Every GitHub release provides:

| File | Use with |
|---|---|
| `hyprfetch_<version>-1_amd64.deb` | Ubuntu / Debian / Mint / Pop!_OS |
| `hyprfetch-<version>-1.x86_64.rpm` | Fedora / RHEL / CentOS / openSUSE |
| `PKGBUILD` | Arch Linux (fast install via `makepkg`) |
| `hyprfetch-<version>-x86_64-unknown-linux-gnu.tar.gz` | any Linux x86_64 |
| `hyprfetch-<version>-aarch64-unknown-linux-gnu.tar.gz` | any Linux ARM64 |
| `hyprfetch-<version>-x86_64-unknown-linux-musl.tar.gz` | any Linux (fully static) |
| `*.sha256` | checksums for the tarballs |

## Ubuntu / Debian (.deb)

```bash
# replace <version> with the release you downloaded, e.g. 0.2.0
sudo dpkg -i hyprfetch_<version>-1_amd64.deb
# if dpkg reports missing deps:
sudo apt-get -f install

# or, in one step (apt resolves the local file):
sudo apt install ./hyprfetch_<version>-1_amd64.deb
```

What it installs:

- `/usr/bin/hyprfetch` — the binary
- `/usr/share/doc/hyprfetch/` — README, CHANGELOG, copyright

The only runtime dependency is glibc; SQLite is bundled and TLS is rustls,
so there is nothing else to pull in. Remove with `sudo apt remove hyprfetch`.

## Fedora / RHEL (.rpm)

```bash
# replace <version> with the release you downloaded, e.g. 0.2.0
sudo dnf install ./hyprfetch-<version>-1.x86_64.rpm
# or on older systems:
sudo yum localinstall ./hyprfetch-<version>-1.x86_64.rpm
# or with plain rpm:
sudo rpm -ivh hyprfetch-<version>-1.x86_64.rpm
```

What it installs: `/usr/bin/hyprfetch` plus documentation under
`/usr/share/doc/hyprfetch/` and the license under
`/usr/share/licenses/hyprfetch/`. Remove with
`sudo dnf remove hyprfetch` (or `sudo rpm -e hyprfetch`).

## Arch Linux (PKGBUILD — fast install)

Download the `PKGBUILD` attached to the release (it is generated per release
with the version and the sha256 of the x86_64 tarball already pinned), then:

```bash
mkdir hyprfetch-bin && cd hyprfetch-bin
# move the downloaded PKGBUILD into this directory
makepkg -si
```

`makepkg` downloads the release tarball, verifies it against the pinned
sha256, and installs `hyprfetch-bin` with pacman (`-s` resolves dependencies,
`-i` installs). This is the fastest Arch path — no compiler needed, seconds
to install. Remove with `sudo pacman -R hyprfetch-bin`.

> Building from source instead? Clone the repo, copy the same `PKGBUILD`
> pattern with `source=("git+…#tag=v$pkgver")`, or just run
> `cargo build --release` — see below.

## Binary tarball (any Linux)

```bash
# replace <target> with x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu,
# or x86_64-unknown-linux-musl
VERSION=0.2.0 TARGET=x86_64-unknown-linux-gnu
curl -fLO "https://github.com/Local-DE-Coach/HyprFetch/releases/download/v${VERSION}/hyprfetch-${VERSION}-${TARGET}.tar.gz"
# verify (recommended)
curl -fLO "https://github.com/Local-DE-Coach/HyprFetch/releases/download/v${VERSION}/hyprfetch-${VERSION}-${TARGET}.tar.gz.sha256"
sha256sum -c "hyprfetch-${VERSION}-${TARGET}.tar.gz.sha256"

tar xzf "hyprfetch-${VERSION}-${TARGET}.tar.gz"
sudo install -Dm755 "hyprfetch-${VERSION}-${TARGET}/hyprfetch" /usr/local/bin/hyprfetch
```

The musl tarball is fully static and runs on any Linux — including minimal
containers and servers without glibc.

## Build from source

Requires Rust 1.85+ (`rustup`) and pkg-config; SQLite is bundled, and the
committed `ui/dist` means **no Node toolchain is needed** for a normal build.

```bash
git clone https://github.com/Local-DE-Coach/HyprFetch.git
cd HyprFetch
cargo build --release --locked
sudo install -Dm755 target/release/hyprfetch /usr/local/bin/hyprfetch
```

See [`development.md`](development.md) for frontend development, tests, and
code style.

## First run

```bash
hyprfetch serve            # binds 127.0.0.1:7780
hyprfetch doctor           # verify config/db/pragma state
xdg-open http://127.0.0.1:7780
```

Useful flags (all have `HYPRFETCH_*` env equivalents — see `--help`):

| Flag | Default | Meaning |
|---|---|---|
| `--bind` | `127.0.0.1:7780` | listen address (keep loopback unless you add auth) |
| `--download-dir` | `~/Downloads` | where downloads land |
| `--segments` | `8` | default segments per task |
| `--db-path` | `~/.local/share/hyprfetch/hyprfetch.db` | SQLite state file |

Downloads resume automatically across restarts: incomplete tasks are
re-probed at startup and continued from the last persisted byte offset
(remote change is detected via ETag/Last-Modified and restarts cleanly).

## Heavy-use note (file descriptor limits)

Each active segment is one HTTP connection. With many concurrent tasks, the
default soft `ulimit -n` (often 1024) can be tight. For unattended boxes:

```bash
# systemd unit override example:
[Service]
LimitNOFILE=8192
```
