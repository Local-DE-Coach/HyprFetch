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
const EXT_VERSION = '0.6.1';

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

// ---- popup + icon ----------------------------------------------------------

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
    } else {
      sendResponse({ ok: false });
    }
  })();
  return true; // async sendResponse
});

if (api.action?.setBadgeBackgroundColor) {
  api.action.setBadgeBackgroundColor({ color: '#7C5CFF' });
}

heartbeat();
setInterval(heartbeat, 30_000);
