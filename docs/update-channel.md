# Update channel — self-hosted fast mirror (istias.tech)

`hyprfetch update --check` is designed to be **one fast HTTPS GET** with no
GitHub involvement. That is the *update channel*: on every release, CI
mirrors the built archives and a small `latest.json` manifest to the
project's own server, in a **dedicated new directory** so the existing web
site on the same host is untouched.

```
Client                         Server (istias.tech / 138.197.73.65)
------                         -----------------------------------
hyprfetch update --check  -->  GET /hyprfetch/updates/latest.json
                               (static JSON, ~1 KB, no auth, no rate limit)
hyprfetch update          -->  GET /hyprfetch/updates/<version>/<archive>.tar.gz
                               sha256-verify -> atomic swap -> restart
```

If the channel is unreachable (server down, DNS, offline), the updater
silently falls back to the GitHub REST API tier and then the plain-git tier
— the behavior is identical, just slower.

## Server layout (new directory, site untouched)

```
/var/www/istias.tech/hyprfetch/          <- DEPLOY_PATH (default)
└── updates/
    ├── latest.json                      <- always points at the newest release
    ├── 0.3.3/
    │   ├── hyprfetch-0.3.3-x86_64-unknown-linux-gnu.tar.gz
    │   ├── hyprfetch-0.3.3-x86_64-unknown-linux-gnu.tar.gz.sha256
    │   ├── hyprfetch-0.3.3-aarch64-unknown-linux-gnu.tar.gz  (+ .sha256)
    │   └── hyprfetch-0.3.3-x86_64-unknown-linux-musl.tar.gz  (+ .sha256)
    └── 0.3.4/ …                          <- old versions stay downloadable
```

`https://istias.tech/` keeps serving the existing site — the channel lives
entirely under the `/hyprfetch/` URL prefix.

## One-time server setup

Run on the server once (as root, or a user with write access to the web
root):

```bash
# 1. Create the update-channel directory (NEW dir — nothing else is touched).
install -d -m 755 /var/www/istias.tech/hyprfetch/updates

# 2. Allow the deploy user to write into it (adjust user to taste).
chown -R root:root /var/www/istias.tech/hyprfetch
```

Then make nginx serve that directory under the `/hyprfetch/` URL prefix.
Add this `location` block **inside the existing `server { … }` block** for
`istias.tech` (do not touch any other block — the old project keeps
working):

```nginx
location /hyprfetch/ {
    alias /var/www/istias.tech/hyprfetch/;
    autoindex off;
    add_header Cache-Control "public, max-age=60";
}

location = /hyprfetch/updates/latest.json {
    alias /var/www/istias.tech/hyprfetch/updates/latest.json;
    default_type application/json;
    add_header Cache-Control "no-cache";
}
```

The second block makes clients always see a fresh `latest.json` while the
heavy archives stay cacheable. Reload nginx:

```bash
nginx -t && systemctl reload nginx
```

> **Not using nginx on the origin?** Create the equivalent route with your
> web server (Caddy, Apache…) or a Cloudflare Origin Rule. What matters is:
> `https://istias.tech/hyprfetch/updates/latest.json` must map to
> `/var/www/istias.tech/hyprfetch/updates/latest.json` on
> `138.197.73.65`.

> **Cloudflare note:** the domain is proxied (orange cloud) — that is fine.
> `.json` is not in Cloudflare's default cache extension list, so manifest
> checks stay live; archives are immutable per version directory, so
> caching them is harmless.

## GitHub secrets (release workflow)

| Secret         | Default (when unset)                 | Purpose                          |
| -------------- | ------------------------------------ | -------------------------------- |
| `DEPLOY_SSH_KEY` | — (**required**)                   | Private SSH key allowed to write `DEPLOY_PATH` on the server |
| `DEPLOY_HOST`  | `138.197.73.65`                      | Server the release is mirrored to |
| `DEPLOY_USER`  | `root`                               | SSH user                          |
| `DEPLOY_PORT`  | `22`                                 | SSH port                          |
| `DEPLOY_PATH`  | `/var/www/istias.tech/hyprfetch`     | Target directory on the server    |

Generate a dedicated deploy key (server + repo side):

```bash
# on your PC
ssh-keygen -t ed25519 -f hf_deploy_key -N "" -C "hyprfetch-release-deploy"
ssh-copy-id -i hf_deploy_key.pub root@138.197.73.65
# paste the contents of hf_deploy_key (PRIVATE key) into the
# DEPLOY_SSH_KEY secret: repo -> Settings -> Secrets and variables -> Actions
```

If `DEPLOY_SSH_KEY` is not set, the workflow **skips** the mirror step with
a notice — the GitHub release itself is always published regardless, and
the updater falls back to GitHub until the secret is added.

## What the client does with the manifest

`latest.json` schema (produced by CI, consumed by the updater):

```json
{
  "version": "0.3.3",
  "tag": "v0.3.3",
  "published_at": "2026-09-29T12:00:00Z",
  "notes_url": "https://github.com/Local-DE-Coach/HyprFetch/releases/tag/v0.3.3",
  "assets": {
    "x86_64-unknown-linux-gnu": {
      "url": "https://istias.tech/hyprfetch/updates/0.3.3/hyprfetch-0.3.3-x86_64-unknown-linux-gnu.tar.gz",
      "sha256": "…",
      "size": 8321005
    }
  }
}
```

- `update --check` compares `version` against the running binary and prints
  `up to date` or the newer release.
- `update` picks the asset for the machine's target triple, downloads it,
  verifies the **sha256 from the manifest** (a hash mismatch aborts the
  install before anything is swapped), extracts the `hyprfetch` binary,
  swaps it atomically (`hyprfetch.old` kept as rollback) and restarts the
  daemon when one is running.

## Client-side configuration

The channel needs no configuration (the project mirror is the built-in
default), but every layer can be overridden:

```toml
# ~/.config/hyprfetch/config.toml
[update]
channel = "https://istias.tech/hyprfetch/updates/"  # default
# channel = ""            # disable the fast tier (GitHub only)
# channel = "http://nas.local:8000/hyprfetch/"   # your own mirror
```

```bash
hyprfetch update --check --channel https://istias.tech/hyprfetch/updates/
export HYPRFETCH_UPDATE_CHANNEL=https://istias.tech/hyprfetch/updates/
hyprfetch doctor   # shows the active channel line
```

Precedence: `--channel` flag > `HYPRFETCH_UPDATE_CHANNEL` env >
`[update] channel` config > built-in default.

## Verifying the channel

```bash
# Manifest must show the newest release:
curl -fsS https://istias.tech/hyprfetch/updates/latest.json | jq

# From the client side:
hyprfetch doctor
hyprfetch update --check
```
