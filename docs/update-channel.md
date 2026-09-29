# Update channel — the one and only update source (istias.tech)

`hyprfetch update` talks to **exactly one source**: the project's own
update channel. On every release, CI mirrors the built archives and a
small `latest.json` manifest to the owner's server, in a **dedicated
directory** so the existing web site on the same host is untouched.
GitHub is **never contacted** by the updater — no API, no rate limits, no
tokens, and it works regardless of whether the source repo is private.

```
Client                         Server (istias.tech / 138.197.73.65)
------                         -----------------------------------
hyprfetch update --check  -->  GET /hyprfetch/updates/latest.json
                               (static JSON, ~1 KB, no auth, no rate limit)
hyprfetch update          -->  GET /hyprfetch/updates/<version>/<archive>.tar.gz
                               sha256-verify -> atomic swap -> restart
```

If the channel is unreachable (server down, DNS, offline), the updater
says so and points at <https://istias.tech/hyprfetch/updates> for manual
steps. There are **no fallbacks** — that is by design: the update path
stays fast, private and independent of GitHub.

## Server layout (dedicated directory, site untouched)

```
/var/www/istias.tech/hyprfetch/          <- DEPLOY_PATH (default)
└── updates/
    ├── latest.json                      <- always points at the newest release
    ├── 0.4.0/
    │   ├── hyprfetch-0.4.0-linux-x64.tar.gz          (+ .sha256)
    │   ├── hyprfetch-0.4.0-linux-arm64.tar.gz        (+ .sha256)
    │   └── hyprfetch-0.4.0-linux-musl-x64.tar.gz     (+ .sha256)
    └── 0.5.0/ …                          <- old versions stay downloadable
```

`https://istias.tech/` keeps serving the Docs portal — the channel lives
entirely under the `/hyprfetch/updates/` URL prefix, and the human-facing
pages (`/hyprfetch`, `/hyprfetch/updates`) are part of that portal.

## Routing (automated — no manual server setup)

The [Docs repo](https://github.com/Local-DE-Coach/Docs) owns the server
configuration and **self-heals the route on every deploy**:

1. it creates `/var/www/istias.tech/hyprfetch/updates` (world-readable),
2. it installs the nginx locations **inside the existing 443 server
   block**:

```nginx
# /hyprfetch/updates  (the PAGE)  -> proxied to the Next.js portal
location = /hyprfetch/updates   { proxy_pass http://127.0.0.1:3000; }
# /hyprfetch/updates/<files>     -> STATIC, served by nginx (zero app RAM)
location ^~ /hyprfetch/updates/ {
    alias /var/www/istias.tech/hyprfetch/updates/;
    autoindex off;
    add_header Access-Control-Allow-Origin *;
    add_header Cache-Control "public, max-age=60";
    try_files $uri =404;
}
```

nginx serving the archives from disk costs the Next.js app nothing —
important on the 500 MB-RAM VPS. `latest.json` is revalidated every
minute; the per-version archives are immutable.

> **Cloudflare note:** the domain is proxied (orange cloud) — that is fine.
> JSON is not in Cloudflare's default cache extension list, so manifest
> checks stay live; caching the immutable per-version archives is harmless.

## GitHub secrets (release workflow)

| Secret         | Default (when unset)                 | Purpose                          |
| -------------- | ------------------------------------ | -------------------------------- |
| `DEPLOY_SSH_KEY` | — (**required**)                   | Private SSH key allowed to write `DEPLOY_PATH` on the server |
| `DEPLOY_HOST`  | `138.197.73.65`                      | Server the release is mirrored to |
| `DEPLOY_USER`  | `root`                               | SSH user                          |
| `DEPLOY_PORT`  | `22`                                 | SSH port                          |
| `DEPLOY_PATH`  | `/var/www/istias.tech/hyprfetch`     | Target directory on the server    |

One-time setup (the Docs repo's org secret is NOT shared with the
HyprFetch repo — add the key once):

```bash
# on your PC — reuse the Docs deploy key or generate a dedicated one
ssh-keygen -t ed25519 -f hf_deploy_key -N "" -C "hyprfetch-release-deploy"
ssh-copy-id -i hf_deploy_key.pub root@138.197.73.65
# paste the contents of hf_deploy_key (PRIVATE key) into the
# DEPLOY_SSH_KEY secret:
#   github.com/Local-DE-Coach/HyprFetch -> Settings -> Secrets and variables -> Actions
```

If `DEPLOY_SSH_KEY` is not set, the release workflow **skips** the mirror
step with a notice — the GitHub release itself is always published, and
the updater keeps answering from whatever the channel last served.

## What the client does with the manifest

`latest.json` schema (produced by CI, consumed by the updater):

```json
{
  "version": "0.4.0",
  "tag": "v0.4.0",
  "published_at": "2026-09-29T12:00:00Z",
  "notes_url": "https://istias.tech/hyprfetch/updates",
  "assets": {
    "x86_64-unknown-linux-gnu": {
      "url": "https://istias.tech/hyprfetch/updates/0.4.0/hyprfetch-0.4.0-linux-x64.tar.gz",
      "sha256": "…",
      "size": 8321005
    },
    "aarch64-unknown-linux-gnu": { "url": "…", "sha256": "…", "size": 0 },
    "x86_64-unknown-linux-musl": { "url": "…", "sha256": "…", "size": 0 }
  }
}
```

- CI maps the **clean archive names** (`linux-x64`, `linux-arm64`,
  `linux-musl-x64`) back to target triples for the manifest keys, so the
  file names stay human-friendly while the client still finds its asset.
- `update --check` compares `version` against the running binary and prints
  `up to date` or the newer release.
- `update` picks the asset for the machine's target triple, downloads it,
  verifies the **sha256 from the manifest** (a mismatch aborts the install
  before anything is swapped), extracts the `hyprfetch` binary, swaps it
  atomically (`hyprfetch.old` kept as rollback) and restarts the daemon
  when one is running.

## Client-side overrides

```bash
hyprfetch update --channel <url>    # one-off override (tests, mirrors)
HYPRFETCH_UPDATE_CHANNEL=<url>      # env override
# ~/.config/hyprfetch/config.toml:
#   [update]
#   channel = "https://istias.tech/hyprfetch/updates/"   # default
#   channel = ""                                         # disables the updater
```

## Verify the channel

```bash
curl -s https://istias.tech/hyprfetch/updates/latest.json | jq .version
hyprfetch doctor      # shows the channel the binary will use
hyprfetch update --check
```
