// HyprFetch Media Catcher — background service worker / event page (MV3).
//
// IDM-style media sniffing, the cross-browser way:
// 1. Watch every response via the (passive) webRequest API.
// 2. Classify media by Content-Type + size + URL pattern. HTML/CSS/fonts/
//    tracking pixels are ignored; video, audio, big binaries and images
//    are kept per tab.
// 3. Show a count badge on the toolbar icon (like IDM's green arrow).
// 4. Heartbeat + report to the HyprFetch daemon on http://127.0.0.1:7780
//    — never to GitHub, never to any cloud. The daemon is the only
//    downloader; the extension itself downloads nothing.
//
// Chrome uses `chrome.*`; Firefox provides both — we prefer `chrome` so a
// single file serves both browsers.

const api = globalThis.chrome ?? globalThis.browser;

const DAEMON = 'http://127.0.0.1:7780';
const EXT_VERSION = '0.6.4';

// ---- classification --------------------------------------------------------

const MEDIA_CT = /^(video|audio)\//i;
const IMAGE_CT = /^image\//i;
const OCTET_CT = /^(application\/octet-stream|application\/binary)/i;
const SKIP_CT = /^(text\/html|text\/css|text\/plain|application\/javascript|text\/javascript|application\/json|image\/svg|font\/|application\/font|application\/x-font)/i;

const MEDIA_EXT = /\.(mp4|mkv|webm|mov|avi|flv|m4v|ts|m3u8|mpd|mp3|m4a|aac|ogg|opus|wav|flac|wma)(\?|#|$)/i;
const FILE_EXT = /\.(zip|rar|7z|tar|gz|xz|bz2|iso|apk|dmg|exe|msi|deb|rpm|appimage|epub|pdf|torrent|bin|img|cab)(\?|#|$)/i;
const IMAGE_EXT = /\.(png|jpe?g|gif|webp|avif|bmp|tiff?)(\?|#|$)/i;

const IGNORE_URL = /(doubleclick|googlesyndication|google-analytics|googletagmanager|adservice|\/ads?\/|favicon\.ico|\/favicon|analytics|telemetry|pixel\.|beacon)/i;

// Size thresholds: tiny images clutter the popup; junk < 64 KiB is ignored
// unless it explicitly looks like media by extension or content-type.
const MIN_IMAGE_BYTES = 64 * 1024;
const MIN_PLAIN_BYTES = 512 * 1024;

function classify(url, contentType, contentLength) {
  if (!url || IGNORE_URL.test(url)) return null;
  if (url.startsWith('data:')) return null;

  const ct = (contentType || '').toLowerCase();
  const size = contentLength || 0;

  if (SKIP_CT.test(ct)) return null;

  // Explicit streaming media types always win.
  if (MEDIA_CT.test(ct)) return 'media';
  if (MEDIA_EXT.test(url)) return 'media';

  // Images: only meaningful ones (skip layout debris).
  if (IMAGE_CT.test(ct) || IMAGE_EXT.test(url)) {
    return size >= MIN_IMAGE_BYTES || size === 0 ? 'image' : null;
  }

  // Generic binaries: big or explicitly named.
  if (OCTET_CT.test(ct) || FILE_EXT.test(url)) {
    return size >= MIN_PLAIN_BYTES || size === 0 ? 'file' : null;
  }

  // Unknown type but clearly a chunky transfer (progressive video blobs,
  // CDN assets) — catch it, the user can ignore it in the popup.
  if (!ct && size >= 3 * 1024 * 1024) return 'file';

  return null;
}

function filenameFrom(url, contentType, headers) {
  // 1. Content-Disposition (authoritative when present).
  const cd = (headers && headers.get('content-disposition')) || '';
  const m = /filename\*?=(?:UTF-8''|")?([^";]+)/i.exec(cd);
  if (m) {
    try {
      return decodeURIComponent(m[1].replace(/"/g, ''));
    } catch (_) {
      return m[1].replace(/"/g, '');
    }
  }
  // 2. URL tail minus query, percent-decoded.
  try {
    const u = new URL(url);
    const tail = decodeURIComponent(u.pathname.split('/').pop() || '');
    if (tail) return tail;
  } catch (_) { /* not a parseable URL */ }
  // 3. Content-type-derived guess.
  const ct = (contentType || '').split(';')[0].trim();
  const ext = {
    'video/mp4': 'mp4',
    'video/webm': 'webm',
    'audio/mpeg': 'mp3',
    'image/jpeg': 'jpg',
    'image/png': 'png',
  }[ct];
  return ext ? `media.${ext}` : 'media';
}

// ---- per-tab state ---------------------------------------------------------

/** tabId → Map(url → {url, media_type, size, filename, page_url, ts, kind}) */
const tabMedia = new Map();

/** tabId → {url, title} last known page (from tabs events). */
const tabInfo = new Map();

function remember(tabId, item) {
  if (!tabMedia.has(tabId)) tabMedia.set(tabId, new Map());
  const byUrl = tabMedia.get(tabId);
  const existing = byUrl.get(item.url);
  if (existing) {
    existing.ts = item.ts;
    if (item.size) existing.size = item.size;
    return false;
  }
  byUrl.set(item.url, item);
  // Cap per-tab entries so a media-heavy page can't grow the map forever.
  if (byUrl.size > 200) {
    const firstKey = byUrl.keys().next().value;
    byUrl.delete(firstKey);
  }
  return true;
}

function updateBadge(tabId) {
  try {
    const count = tabMedia.get(tabId)?.size ?? 0;
    api.action.setBadgeText({ tabId, text: count > 0 ? String(count) : '' });
  } catch (_) { /* tab may be gone */ }
}

// ---- daemon communication --------------------------------------------------

let daemonUp = false;

async function daemonFetch(path, body) {
  try {
    const res = await fetch(`${DAEMON}${path}`, {
      method: body ? 'POST' : 'GET',
      headers: body ? { 'Content-Type': 'application/json' } : undefined,
      body: body ? JSON.stringify(body) : undefined,
    });
    daemonUp = res.ok;
    return res.ok ? res.json() : null;
  } catch (_) {
    daemonUp = false;
    return null;
  }
}

async function heartbeat() {
  await daemonFetch('/api/extension/heartbeat', { version: EXT_VERSION });
}

const flushTimers = new Map();
function scheduleFlush(tabId) {
  if (flushTimers.has(tabId)) return;
  const t = setTimeout(async () => {
    flushTimers.delete(tabId);
    await heartbeat();
    const items = [...(tabMedia.get(tabId)?.values() ?? [])];
    if (items.length > 0 && daemonUp) {
      await daemonFetch('/api/extension/media', { media: items });
    }
  }, 1200);
  flushTimers.set(tabId, t);
}

// ---- webRequest hook (registered synchronously — MV3 requirement) ----------

api.webRequest.onCompleted.addListener(
  (details) => {
    if (details.tabId < 0) return; // service-worker / prefetch traffic
    const headers = details.responseHeaders ?? [];
    const get = (name) => {
      const h = headers.find((x) => x.name.toLowerCase() === name);
      return h ? h.value : undefined;
    };
    const contentType = get('content-type');
    const lenRaw = get('content-length');
    const contentLength = lenRaw ? parseInt(lenRaw, 10) || 0 : 0;

    const kind = classify(details.url, contentType, contentLength);
    if (!kind) return;

    const page = tabInfo.get(details.tabId) ?? {};
    const added = remember(details.tabId, {
      url: details.url,
      media_type: contentType ?? null,
      size: contentLength || null,
      filename: filenameFrom(details.url, contentType),
      page_url: page.url ?? null,
      page_title: page.title ?? null,
      ts: Date.now(),
      kind,
    });
    if (added) {
      updateBadge(details.tabId);
      scheduleFlush(details.tabId);
    }
  },
  { urls: ['<all_urls>'] },
  ['responseHeaders'],
);

// ---- tab lifecycle ---------------------------------------------------------

api.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
  if (changeInfo.status === 'loading' && changeInfo.url) {
    // Fresh navigation — clear the previous page's findings.
    tabMedia.delete(tabId);
    updateBadge(tabId);
  }
  const info = { url: tab?.url ?? null, title: tab?.title ?? null };
  if (info.url || info.title) tabInfo.set(tabId, info);
});

api.tabs.onRemoved.addListener((tabId) => {
  tabMedia.delete(tabId);
  tabInfo.delete(tabId);
});

if (api.tabs.onActivated) {
  api.tabs.onActivated.addListener(({ tabId }) => updateBadge(tabId));
}

// ---- popup + content-script bridge -----------------------------------------

// Pages where a probe is worth it — identical to the popup's list.
const VIDEO_PAGE_RE = /(?:youtube\.com\/(?:watch|shorts|embed|live)|youtu\.be\/|m\.youtube\.com\/watch|vimeo\.com\/\d+|dailymotion\.com\/(?:video|embed)|twitch\.tv\/videos\/|soundcloud\.com\/[^/]+\/[^/?]+|tiktok\.com\/@[^/]+\/video|instagram\.com\/(?:reel|p)\/|facebook\.com\/watch|\/watch\/|\/video\/[\w-]{6,})/i;

// Prefetch probes make the quality list INSTANT: the moment a video page
// finishes loading we ask the daemon to extract formats in the background
// (the daemon caches the result for 15 min), so the popup / in-page panel
// render from the cache instead of waiting seconds for yt-dlp.
const prefetched = new Map(); // url → ts
function prefetchProbe(url) {
  if (!url || prefetched.has(url)) return;
  const now = Date.now();
  for (const [u, ts] of prefetched) {
    if (now - ts > 5 * 60_000) prefetched.delete(u);
  }
  prefetched.set(url, now);
  void daemonFetch('/api/media/probe', { url });
}

api.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  (async () => {
    if (msg?.type === 'getMedia') {
      const tabId = msg.tabId;
      let items = [...(tabMedia.get(tabId)?.values() ?? [])];
      if (items.length === 0 && msg.includeAll) {
        for (const byUrl of tabMedia.values()) {
          items = items.concat([...byUrl.values()]);
        }
      }
      sendResponse({ items, daemonUp });
    } else if (msg?.type === 'download') {
      const body = {
        url: msg.url,
        filename: msg.filename ?? null,
        page_url: msg.page_url ?? null,
      };
      const res = await daemonFetch('/api/extension/download', body);
      sendResponse({ ok: !!res, task: res });
    } else if (msg?.type === 'probeMedia') {
      // Ask the daemon's media engine (yt-dlp) what formats this page really
      // has — the 8K→480p ladder lives on the daemon, not in the extension.
      const res = await daemonFetch('/api/media/probe', { url: msg.url });
      sendResponse({ ok: !!res, data: res });
    } else if (msg?.type === 'mediaDownload') {
      // Quality-picked media task (POST /api/media/download).
      const res = await daemonFetch('/api/media/download', {
        url: msg.url,
        quality: msg.quality ?? null,
        audio_only: !!msg.audio_only,
        filename: msg.filename ?? null,
      });
      sendResponse({ ok: !!res, task: res });
    } else {
      sendResponse({ ok: false });
    }
  })();
  return true; // async sendResponse
});

// Fire the prefetch as soon as a video page settles (daemon-side cache does
// the heavy lifting; repeated loads within the cooldown are free).
api.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
  const url = changeInfo?.url || (changeInfo?.status === 'complete' ? tab?.url : null) || '';
  if (/^https?:/i.test(url) && VIDEO_PAGE_RE.test(url)) prefetchProbe(url);
});

// ---- context menu (right-click → Download with HyprFetch) -------------------

const MENU_PARENT = 'hf-root';

function buildMenus() {
  if (!api.contextMenus) return;
  try {
    api.contextMenus.removeAll(() => {
      const add = (opts) => {
        try {
          api.contextMenus.create(opts);
        } catch (_) { /* duplicate id on rapid restarts */ }
      };
      add({ id: MENU_PARENT, title: 'Download with HyprFetch', contexts: ['image', 'video', 'audio', 'link', 'page'] });
      add({ id: 'hf-image', parentId: MENU_PARENT, title: 'Download image', contexts: ['image'] });
      add({ id: 'hf-video', parentId: MENU_PARENT, title: 'Download video', contexts: ['video'] });
      add({ id: 'hf-audio', parentId: MENU_PARENT, title: 'Download audio', contexts: ['audio'] });
      add({ id: 'hf-link', parentId: MENU_PARENT, title: 'Download link', contexts: ['link'] });
      add({ id: 'hf-page', parentId: MENU_PARENT, title: 'Video qualities on this page…', contexts: ['page'] });
    });
  } catch (_) { /* contextMenus unavailable */ }
}

buildMenus();

if (api.contextMenus?.onClicked) {
  api.contextMenus.onClicked.addListener((info, tab) => {
    if (info.menuItemId === 'hf-page') {
      // Ask the page's content script to open the quality panel (probe →
      // yt-dlp format list → one click queues the pick).
      const url = info.pageUrl || tab?.url || '';
      if (/^https?:/i.test(url) && tab?.id != null && tab.id >= 0) {
        void pcall(api.tabs.sendMessage.bind(api.tabs), tab.id, {
          type: 'showQualityPanel', url,
        });
      }
      return;
    }
    const urlByMenu = {
      'hf-image': info.srcUrl,
      'hf-video': info.srcUrl,
      'hf-audio': info.srcUrl,
      'hf-link': info.linkUrl,
    };
    const url = urlByMenu[info.menuItemId];
    if (url && /^https?:/i.test(url)) {
      void daemonFetch('/api/extension/download', {
        url,
        filename: filenameFrom(url),
        page_url: info.pageUrl ?? null,
      }).then((res) => flashBadge(res ? '↓' : '!'));
    }
  });
}

if (api.action?.setBadgeBackgroundColor) {
  api.action.setBadgeBackgroundColor({ color: '#7C5CFF' });
}

heartbeat();
setInterval(heartbeat, 30_000);

// ---- auto-capture: browser downloads → HyprFetch (IDM-style takeover) ------
//
// Every download the BROWSER starts is intercepted here:
//   1. The URL is handed to the daemon first (POST /api/extension/download) —
//      the daemon, not the browser, becomes the downloader.
//   2. Only after the daemon ACCEPTS the task is the browser's own download
//      cancelled and erased from the shelf, so nothing is fetched twice and
//      the file lands in HyprFetch's folder, not the browser's.
//   3. If the daemon is unreachable the browser download proceeds untouched
//      — taking over must never cost the user a download.
//
// Skipped on purpose: non-http(s) URLs (blob:/data:/file: can't be re-fetched
// by the daemon), traffic to the daemon itself (the WebUI's own downloads),
// ad/telemetry hosts, and a per-URL cooldown that stops the cancel/retry
// ping-pong (Chrome re-fires onCreated when a cancelled item is retried).
// Toggle lives in the popup (chrome.storage.local 'autoCapture', default ON).

let autoCapture = true;

function storageGetLocal(def) {
  return new Promise((resolve) => {
    try {
      const r = api.storage.local.get(def, (v) => resolve(v ?? def));
      if (r && typeof r.then === 'function') r.then((v) => resolve(v ?? def), () => resolve(def));
    } catch (_) {
      resolve(def);
    }
  });
}

storageGetLocal({ autoCapture: true }).then((v) => {
  autoCapture = v.autoCapture !== false;
});

if (api.storage.onChanged) {
  api.storage.onChanged.addListener((changes, area) => {
    if (area === 'local' && changes.autoCapture) {
      autoCapture = changes.autoCapture.newValue !== false;
    }
  });
}

const capturedCooldown = new Map(); // url → ts of last takeover
function recentlyTaken(url) {
  const now = Date.now();
  for (const [u, ts] of capturedCooldown) {
    if (now - ts > 60_000) capturedCooldown.delete(u);
  }
  return capturedCooldown.has(url);
}

function baseName(p) {
  if (!p) return null;
  const i = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'));
  return i >= 0 ? p.slice(i + 1) : p;
}

/** Callback-style + promise-style compat for downloads.* calls. */
function pcall(fn, ...args) {
  return new Promise((resolve) => {
    try {
      const r = fn(...args, () => resolve(true));
      if (r && typeof r.then === 'function') {
        r.then(() => resolve(true), () => resolve(false));
      }
    } catch (_) {
      resolve(false);
    }
  });
}

async function flashBadge(text) {
  try {
    api.action.setBadgeText({ text });
    setTimeout(() => {
      try {
        api.action.setBadgeText({ text: '' });
      } catch (_) { /* noop */ }
    }, 3500);
  } catch (_) { /* noop */ }
}

if (api.downloads?.onCreated) {
  api.downloads.onCreated.addListener((item) => {
    void takeOverDownload(item);
  });
}

async function takeOverDownload(item) {
  if (!autoCapture) return;
  const url = item.url || '';
  if (!/^https?:/i.test(url)) return; // blob:/data:/file: — daemon can't re-fetch
  if (url.startsWith(DAEMON)) return; // our own WebUI / daemon traffic
  if (IGNORE_URL.test(url)) return; // ads / telemetry junk
  if (recentlyTaken(url)) return; // cancel→retry loop guard

  capturedCooldown.set(url, Date.now());

  const res = await daemonFetch('/api/extension/download', {
    url,
    filename: baseName(item.filename),
    page_url: item.referrer || tabInfo.get(item.tabId ?? -1)?.url || null,
  });
  if (!res) return; // daemon offline → browser download continues untouched

  // Daemon accepted: stop + hide the browser's copy (HyprFetch owns it now).
  await pcall(api.downloads.cancel.bind(api.downloads), item.id);
  await pcall(api.downloads.erase.bind(api.downloads), { id: item.id });
  await flashBadge('↓');
}
