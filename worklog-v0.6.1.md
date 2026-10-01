# Worklog — v0.6.1 "Media engine + Browser extension"

Dedicated worklog for the v0.6.1 feature train, as requested: this version
is **about the download itself** (v0.4.9–v0.5.2 already fixed updates,
widget and UI). Companion reading: `worklog.md` (session log),
`worktasks.md` (task board), `CHANGELOG.md` (user-facing changes).

**Sandbox:** agent workspace, Rust 1.98 · node 20 · UI build in
`crates/hyprfetch-api/ui/dist` (rust-embed) — every UI change is committed
built, same as previous versions.

---

## The problem being solved

1. **Media that "is not a file" failed to download.** The user's repro: a
   LinkedIn image URL with **no extension in the path**
   (`media.licdn.com/dms/image/v2/...?e=…&v=beta&t=…`) — real content is
   `image/jpeg` behind query params, but the planner only ever saw a URL.
   Worse: *streaming* media (YouTube & friends) has no direct URL at all —
   the real bytes live behind DASH/HLS manifests with rotating signed
   URLs, which a segmented HTTP downloader can never fetch.
2. **No quality choice.** Other managers (IDM, FDM) list every quality the
   source offers and let the user pick; HyprFetch had only "download the
   thing".
3. **Browser integration was missing.** The user asked for the IDM
   experience: an extension that shows an icon/badge on tabs with media,
   lists what it found, and hands picks to the app — plus a WebUI page
   that distributes the extension from **our server** (never GitHub) and
   shows what came in through it, with a filter in the main task list.

## Research (how the other apps do it)

- **IDM** ships a browser extension that passively watches `webRequest`
  responses, classifies media by `Content-Type`/size/URL pattern, shows a
  per-tab **badge count**, and offers a popup listing "videos in this
  page"; picks go to the local app over `127.0.0.1` HTTP. The app itself
  owns multiple extraction engines (one for HLS/DASH, one for plain HTTP).
- **FDM** does the same with a local daemon + extension pairing.
- **yt-dlp** is the de-facto standard extraction engine for YouTube and
  1000+ sites; `-J` dumps format metadata (id, ext, height, fps, vcodec,
  acodec, filesize, tbr) and `A+B` selectors + `--merge-output-format mp4`
  produce the MP4 users expect.

Conclusion: don't reinvent extraction — drive yt-dlp as a managed
subprocess (like IDM partners its engines), keep the native segmented
downloader for direct files, and build the extension exactly like IDM's.

## What was built

### Backend (Rust)

| Piece | Where | Notes |
|---|---|---|
| DB migration | `crates/hyprfetch-db/migrations/003_source_and_media.sql` | `tasks.source` (`app`/`extension`/`media`) + `tasks.media_meta` JSON + index |
| Media engine | `crates/hyprfetch-core/src/media.rs` | yt-dlp locate/auto-install (channel mirror first), `-J` probe, `quality_ladder` dedupe (ONE option per height, MP4-preferred, audio-only MP3 last), progress-line parser, argv builder, `run_ytdlp_coordinator` |
| Engine branch | `crates/hyprfetch-core/src/engine.rs` | `spawn_coordinator` routes `source == "media"` tasks to the yt-dlp coordinator; same `TaskCommand` pause/cancel protocol, same events/DB persistence |
| Task rows | `crates/hyprfetch-db/src/repo.rs` | `list_filtered(state, source)`, source/media_meta on TaskRow |
| API | `crates/hyprfetch-api/src/media.rs` | `POST /api/media/probe` (unified: file vs media), `POST /api/media/download`, `GET /api/media/ytdlp`, `POST /api/media/ytdlp/install` |
| Extension bridge | `crates/hyprfetch-api/src/extension.rs` | heartbeat / media report+list+clear / download (source=extension) / status; scoped CORS shim for `chrome-extension://` + `moz-extension://` origins |
| Routes | `routes.rs` | TaskDto `source`+`media`, `?source=` filter, `create_extension_task`/`create_media_task` helpers |
| CLI | `crates/hyprfetch/src/{main,background}.rs` | `hyprfetch add <url> --quality 1080p` / `--audio` → media engine |
| Widget status | `widget_status.rs` | `source` field in `status.json` entries (backward-compatible) |
| Release | `.github/workflows/release.yml` | extension zip+xpi packaged+gated, yt-dlp binary mirrored, deploy steps for `extension/` + `bin/yt-dlp/` on istias.tech |

### Browser extension (`extension/`)

- `manifest-chrome.json` (MV3 service worker) + `manifest-firefox.json`
  (MV3 event page, gecko id `hyprfetch@istias.tech`) — same JS for both.
- `background.js`: `webRequest.onCompleted` sniffer → classify (skip
  HTML/CSS/fonts/ads; video/audio always, images ≥ 64 KiB, binaries
  ≥ 512 KiB or known extensions) → per-tab map → **toolbar badge count**
  → debounced batch report + 30 s heartbeat to `127.0.0.1:7780`. Tab
  navigation clears that tab's findings.
- `popup.*`: connection dot, per-tab media list (kind badges, sizes),
  one-click **Send to HyprFetch**, "Open HyprFetch" button.
- `scripts/build_extension.sh`: packages `hyprfetch-extension-<v>-chrome.zip`
  + `-firefox.xpi` and asserts both contain a valid manifest; icons via
  `scripts/gen_extension_icons.py` (Pillow gradient + download arrow).

### WebUI (`crates/hyprfetch-api/ui`)

- New **Extension page** (`pages/Extension.svelte`): connection status,
  Firefox/Chromium install cards linking the channel packages, captured
  media list with Download/Clear, auto-refresh every 5 s.
- **Add dialog quality picker**: `probeMedia` first — direct files keep the
  IDM-style confirm card, stream pages show the quality ladder (radio,
  sizes, containers, audio-only MP3), then `mediaDownload`.
- **Tasks page**: source chips (All / Manual / ⇪ Extension / ▶ Media);
  task rows show `⇪ ext` / `▶ 1080p` badges.
- **Settings**: "Media engine" card — yt-dlp version, path, Install/Update.
- `api.js`: `probeMedia`, `mediaDownload`, `getYtdlpStatus`,
  `installYtdlp`, `extensionStatus/Media/ClearMedia/Download`.

## Quality-gate decisions (the "no duplicates" contract)

`quality_ladder` guarantees (unit-tested in `media.rs::tests`):
- one option per **height** — `mp4` 1080p and `webm` 1080p collapse into
  one entry (score: container mp4 > webm, h264 bonus, then fps, then tbr);
- DASH video-only entries pair with bestaudio via `A+B` selector and
  `--merge-output-format mp4` so the file lands as one MP4;
- sizes merge video+audio; 60fps variants win and surface in the note;
- ladder is height-descending with `Audio only (MP3)` last;
- `title_to_filename` flattens `/`→`-` BEFORE sanitize so `AC/DC` titles
  survive (sanitize alone treats `/` as "last segment wins").

## Test evidence (filled during the session)

- `cargo test` - **217 green** (core 134 incl. 13 media tests, API 63,
  db 16, ws_events 2, bin 2).
- `cargo fmt --all` + `cargo clippy --all-targets -- -D warnings` — clean.
- `npm run build` + `npm run lint` — clean; UI committed built.
- Live E2E (`scripts/e2e_v061.sh`, fresh daemon): **19/19 PASS** -
  extension bridge (heartbeat, report+dedupe, CORS preflight, download
  with `source=extension`), tasks source filter, unified probe (LinkedIn
  URL class, direct-MP4 dispatch), yt-dlp auto-install, **real SoundCloud
  quality-picked download with the file on disk**, and the widget
  `status.json` schema check (now with `source`).
- Extension packages: built + zip-validated; JS syntax-gated with
  `node --check`.

## Known limits (honest list)

- Live streams (`is_live`) are not supported — yt-dlp can, but pause/eta
  semantics don't map to task rows; probe returns the ladder and the
  download works, yet the widget ETA stays `--:--:--`. Documented, not
  hidden.
- yt-dlp needs periodic updates for YouTube; the daemon does NOT auto-
  update the binary itself (v0.6.1) — Settings has the Update button and
  auto-update is a v0.6.2 candidate.
- The extension is distributed as a load-unpacked zip / file-install xpi
  (no store listing yet) — store packaging is a separate task.
