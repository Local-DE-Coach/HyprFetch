// HyprFetch Media Catcher — content script (v0.6.3).
//
// The IDM-style in-page layer:
//   1. A small ⬇ pill sits on every <video>/<audio> player (and on large
//      images it appears on hover). Clicking it sends that exact media to
//      the HyprFetch daemon.
//   2. Players without a direct URL (YouTube's blob: player, DRM-less DASH)
//      open the QUALITY PANEL instead: the daemon probes the page with
//      yt-dlp and the panel lists every real format — 8K/4K/2K/1080p/720p/
//      480p … plus MP3 — one click queues the pick as a normal task.
//   3. Alt+click on any media (image, audio, video, file/PDF link) sends it
//      straight to the daemon — no menus needed.
//   4. The background worker can also open the quality panel (used by the
//      right-click "HyprFetch → video qualities" menu item).
//
// Everything talks ONLY to the background worker (runtime.sendMessage);
// the background holds the host permissions, so page CSP/CORS never sees
// the daemon. The extension downloads nothing itself — the daemon does.

const api = globalThis.chrome ?? globalThis.browser;

const DAEMON_URL = 'http://127.0.0.1:7780';

/** Minimum rendered size for an <img> to earn a hover pill (px). */
const MIN_IMG_PX = 200;

let buttonsOn = true; // storage key `mediaButtons`, default ON

// ---------------------------------------------------------------------------
// messaging helpers (callback/promise compatible across Chrome + Firefox)
// ---------------------------------------------------------------------------

function send(msg) {
  return new Promise((resolve) => {
    try {
      const r = api.runtime.sendMessage(msg, (res) => {
        void api.runtime.lastError; // swallow "receiving end" noise
        resolve(res ?? null);
      });
      if (r && typeof r.then === 'function') {
        r.then((v) => resolve(v ?? null), () => resolve(null));
      }
    } catch (_) {
      resolve(null);
    }
  });
}

function storageGet(def) {
  return new Promise((resolve) => {
    try {
      const r = api.storage.local.get(def, (v) => resolve(v ?? def));
      if (r && typeof r.then === 'function') r.then((v) => resolve(v ?? def), () => resolve(def));
    } catch (_) {
      resolve(def);
    }
  });
}

try {
  api.storage.onChanged?.addListener((changes, area) => {
    if (area === 'local' && changes.mediaButtons) {
      buttonsOn = changes.mediaButtons.newValue !== false;
      if (!buttonsOn) tearAll();
      else scan();
    }
  });
} catch (_) { /* storage events unavailable */ }

// ---------------------------------------------------------------------------
// overlay pill (shadow-DOM isolated, fixed-positioned over the media element)
// ---------------------------------------------------------------------------

/** element → {host, pill, hoverOnly} */
const pills = new Map();

function reposition(entry) {
  const { host, target } = entry;
  if (!target.isConnected) return;
  const r = target.getBoundingClientRect();
  if (r.width < 40 || r.height < 40) {
    host.style.display = 'none';
    return;
  }
  host.style.display = '';
  const x = Math.min(Math.max(r.left + r.width - 34, 4), window.innerWidth - 40);
  const y = Math.min(Math.max(r.top + 6, 4), window.innerHeight - 34);
  host.style.transform = `translate(${Math.round(x)}px, ${Math.round(y)}px)`;
}

function repositionAll() {
  for (const entry of pills.values()) reposition(entry);
}

let rafPending = false;
function scheduleReposition() {
  if (rafPending) return;
  rafPending = true;
  requestAnimationFrame(() => {
    rafPending = false;
    repositionAll();
  });
}
window.addEventListener('scroll', scheduleReposition, { passive: true, capture: true });
window.addEventListener('resize', scheduleReposition, { passive: true });

function buildPill(entry) {
  const host = document.createElement('div');
  host.style.cssText =
    'position:fixed;left:0;top:0;z-index:2147483646;width:0;height:0;pointer-events:none;';
  const shadow = host.attachShadow({ mode: 'open' });
  const style = document.createElement('style');
  style.textContent = `
    .pill {
      pointer-events: auto;
      display: flex; align-items: center; gap: 4px;
      padding: 3px 8px;
      border: 0; border-radius: 999px;
      background: rgba(20, 19, 26, 0.82);
      color: #fff; font: 700 11px/1.3 system-ui, sans-serif;
      cursor: pointer; opacity: 0.72;
      transition: opacity .15s, background .15s;
      box-shadow: 0 2px 8px rgba(0,0,0,.45);
      white-space: nowrap;
    }
    .pill:hover { opacity: 1; background: #7c5cff; }
    .pill.busy { opacity: 1; background: #262433; cursor: default; }
    .pill.done  { background: #1f9d70; }
    .arrow { font-size: 12px; line-height: 1; }
  `;
  shadow.appendChild(style);
  const pill = document.createElement('button');
  pill.className = 'pill';
  pill.title = 'Download with HyprFetch';
  pill.innerHTML = '<span class="arrow">⬇</span>';
  shadow.appendChild(pill);
  entry.host = host;
  entry.pill = pill;
  pill.addEventListener('click', (ev) => {
    ev.preventDefault();
    ev.stopPropagation();
    void onPillClick(entry);
  });
  document.documentElement.appendChild(host);
  reposition(entry);
}

function attachPill(el, hoverOnly) {
  if (pills.has(el)) return;
  const entry = { target: el, hoverOnly, host: null, pill: null };
  buildPill(entry);
  pills.set(el, entry);
  if (hoverOnly) {
    const show = () => {
      if (!buttonsOn) return;
      entry.host.style.display = '';
      reposition(entry);
    };
    const hide = () => { entry.host.style.display = 'none'; };
    entry.show = show;
    entry.hide = hide;
    el.addEventListener('mouseenter', show);
    el.addEventListener('mouseleave', hide);
    hide();
  }
}

function tearAll() {
  for (const entry of pills.values()) {
    entry.host?.remove();
  }
  pills.clear();
}

function flashPill(entry, mark, text) {
  const pill = entry.pill;
  if (!pill) return;
  const prev = pill.innerHTML;
  pill.classList.add(mark === '✓' ? 'done' : 'busy');
  pill.innerHTML = mark === '✓' ? '<span class="arrow">✓</span>' : '<span class="arrow">…</span>';
  if (text) pill.title = text;
  setTimeout(() => {
    pill.classList.remove('done', 'busy');
    pill.innerHTML = prev;
    pill.title = 'Download with HyprFetch';
  }, 1800);
}

/** What happens when an in-page pill is clicked. */
async function onPillClick(entry) {
  const el = entry.target;
  const src = el.currentSrc || el.src || '';
  if (/^https?:/i.test(src)) {
    flashPill(entry, '…');
    const res = await send({
      type: 'download',
      url: src,
      filename: null,
      page_url: location.href,
    });
    if (res?.ok) flashPill(entry, '✓', 'Queued in HyprFetch ✓');
    else { flashPill(entry, '…', 'HyprFetch daemon offline'); openPanel(el); }
    return;
  }
  // blob:/srcObject/no src (YouTube et al.) → probe the page for formats.
  openPanel(el);
}

// ---------------------------------------------------------------------------
// quality panel (IDM-style format list, rendered in our own shadow root)
// ---------------------------------------------------------------------------

let panel = null; // {host, root, els}

function closePanel() {
  panel?.host.remove();
  panel = null;
}

function fmtSize(bytes) {
  if (!bytes || bytes <= 0) return '';
  const units = ['B', 'KB', 'MB', 'GB'];
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v >= 10 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

function fmtDur(sec) {
  if (!sec || sec <= 0) return '';
  const s = Math.round(sec);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = s % 60;
  return h > 0
    ? `${h}:${String(m).padStart(2, '0')}:${String(r).padStart(2, '0')}`
    : `${m}:${String(r).padStart(2, '0')}`;
}

const PANEL_CSS = `
  * { box-sizing: border-box; margin: 0; padding: 0; }
  .wrap { position: fixed; inset: 0; z-index: 2147483647; pointer-events: none;
          font: 13px/1.45 system-ui, -apple-system, 'Segoe UI', sans-serif; }
  .card {
    pointer-events: auto;
    position: absolute; left: 50%; top: 18px;
    transform: translateX(-50%);
    width: min(560px, calc(100vw - 24px));
    max-height: min(70vh, 560px);
    display: flex; flex-direction: column;
    background: #14131a; color: #eceaf4;
    border: 1px solid #2b2a36; border-radius: 14px;
    box-shadow: 0 12px 48px rgba(0,0,0,.6);
    overflow: hidden;
  }
  .head { display: flex; align-items: center; gap: 10px;
          padding: 12px 14px; background: #1d1c25; border-bottom: 1px solid #2b2a36; }
  .logo { width: 18px; height: 18px; border-radius: 5px; flex: none;
          background: linear-gradient(135deg,#7c5cff,#38e0b0); }
  .ttl { flex: 1; min-width: 0; font-weight: 700; font-size: 13px;
         overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sub { color: #9a97ab; font-size: 11px; flex: none; }
  .x { border: 0; background: none; color: #9a97ab; font-size: 17px;
       cursor: pointer; padding: 0 2px; line-height: 1; }
  .x:hover { color: #fff; }
  .body { padding: 12px 14px; overflow-y: auto; }
  .msg { color: #9a97ab; padding: 14px 4px; text-align: center; font-size: 12.5px; }
  .msg.err { color: #ff6b81; }
  .spin { display: inline-block; width: 14px; height: 14px; vertical-align: -2px;
          border: 2px solid #7c5cff; border-top-color: transparent; border-radius: 50%;
          animation: hfspin .8s linear infinite; margin-right: 6px; }
  @keyframes hfspin { to { transform: rotate(360deg); } }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 8px; }
  .q {
    display: flex; flex-direction: column; align-items: flex-start; gap: 2px;
    border: 1px solid #2b2a36; background: #1d1c25; color: #eceaf4;
    border-radius: 10px; padding: 8px 10px; cursor: pointer; text-align: left;
  }
  .q:hover { border-color: #7c5cff; background: #232030; }
  .q.busy { opacity: .6; cursor: default; }
  .q.audio { border-color: rgba(56,224,176,.5); }
  .q.audio:hover { background: rgba(56,224,176,.08); }
  .ql { font-weight: 700; font-size: 12.5px; }
  .q.audio .ql { color: #38e0b0; }
  .qm { color: #9a97ab; font-size: 10.5px; }
  .foot { padding: 8px 14px; border-top: 1px solid #2b2a36; background: #1d1c25;
          color: #9a97ab; font-size: 10.5px; display: flex; justify-content: space-between; gap: 8px; }
  .lk { color: #b7a5ff; cursor: pointer; text-decoration: underline; }
`;

function ensurePanel() {
  if (panel) return panel;
  const host = document.createElement('div');
  host.style.cssText = 'position:fixed;z-index:2147483647;';
  const root = host.attachShadow({ mode: 'open' });
  const style = document.createElement('style');
  style.textContent = PANEL_CSS;
  root.appendChild(style);
  const wrap = document.createElement('div');
  wrap.className = 'wrap';
  root.appendChild(wrap);
  document.documentElement.appendChild(host);
  panel = { host, root, wrap };
  return panel;
}

function panelMsg(text, err, spinning) {
  const p = ensurePanel();
  p.wrap.innerHTML = `
    <div class="card">
      <div class="head"><span class="logo"></span>
        <span class="ttl">HyprFetch</span>
        <button class="x" title="close">✕</button></div>
      <div class="body"><div class="msg ${err ? 'err' : ''}">
        ${spinning ? '<span class="spin"></span>' : ''}${text}</div></div>
    </div>`;
  p.wrap.querySelector('.x').addEventListener('click', closePanel);
  return p;
}

/** Open the quality panel; `anchor` is only used for the loading label. */
function openPanel(anchor) {
  const pageUrl = location.href;
  const p = panelMsg(
    /\/(watch|shorts|embed|video|status|reel)/i.test(pageUrl)
      ? 'asking the HyprFetch daemon for available formats…'
      : 'scanning this page with the media engine…',
    false,
    true,
  );
  p.wrap.dataset.state = 'loading';
  p.wrap.dataset.anchor = anchor ? '1' : '';

  send({ type: 'probeMedia', url: pageUrl }).then((res) => {
    if (!panel || panel.wrap.dataset.state !== 'loading') return; // closed meanwhile
    if (!res?.ok || !res?.data) {
      panelMsg('HyprFetch daemon is not reachable.<br>Start the app, then try again.', true, false);
      return;
    }
    renderProbe(res.data);
  });
}

function renderProbe(data) {
  if (data.kind === 'file') {
    const f = data.file ?? {};
    const p = panelMsg('Direct file on this page — one click queues it.', false, false);
    const body = p.wrap.querySelector('.body');
    body.innerHTML = '';
    const grid = document.createElement('div');
    grid.className = 'grid';
    const btn = document.createElement('button');
    btn.className = 'q';
    btn.innerHTML = `<span class="ql">Download file</span>
      <span class="qm">${(f.filename || data.url || '').slice(0, 60)}</span>`;
    btn.addEventListener('click', async () => {
      btn.classList.add('busy');
      const res = await send({ type: 'download', url: data.url, filename: f.filename ?? null, page_url: location.href });
      closePanel();
      toastBadge(res?.ok);
    });
    grid.appendChild(btn);
    body.appendChild(grid);
    return;
  }

  const media = data.media ?? {};
  const qualities = media.qualities ?? [];
  if (media.is_live) {
    panelMsg('This is a live stream — HyprFetch can’t download live media yet.', true, false);
    return;
  }
  if (qualities.length === 0) {
    panelMsg('No downloadable formats found on this page.', true, false);
    return;
  }

  const p = ensurePanel();
  const dur = fmtDur(media.duration);
  p.wrap.innerHTML = `
    <div class="card">
      <div class="head"><span class="logo"></span>
        <span class="ttl">${escapeHtml(media.title || document.title || 'video')}</span>
        <span class="sub">${dur ? dur + ' · ' : ''}${qualities.length} formats</span>
        <button class="x" title="close">✕</button></div>
      <div class="body"><div class="grid"></div></div>
      <div class="foot"><span>queued straight into HyprFetch — pick a quality</span>
        <span class="lk">open app</span></div>
    </div>`;
  p.wrap.querySelector('.x').addEventListener('click', closePanel);
  p.wrap.querySelector('.lk').addEventListener('click', () => {
    window.open(DAEMON_URL, '_blank');
    closePanel();
  });
  const grid = p.wrap.querySelector('.grid');

  for (const q of qualities) {
    const btn = document.createElement('button');
    btn.className = `q${q.audio_only ? ' audio' : ''}`;
    const meta = [q.container?.toUpperCase(), fmtSize(q.size_bytes), q.note]
      .filter(Boolean)
      .join(' · ');
    btn.innerHTML = `<span class="ql">${escapeHtml(q.label)}</span>
      <span class="qm">${escapeHtml(meta)}</span>`;
    btn.addEventListener('click', async () => {
      for (const b of grid.querySelectorAll('.q')) b.classList.add('busy');
      btn.querySelector('.ql').textContent = 'queueing…';
      const res = await send({
        type: 'mediaDownload',
        url: media.webpage_url || location.href,
        quality: q.id,
        audio_only: !!q.audio_only,
      });
      closePanel();
      toastBadge(res?.ok);
    });
    grid.appendChild(btn);
  }
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}

/** Tiny corner toast — the panel is gone, but the user deserves feedback. */
function toastBadge(ok) {
  const host = document.createElement('div');
  host.style.cssText = 'position:fixed;z-index:2147483647;';
  const root = host.attachShadow({ mode: 'open' });
  const st = document.createElement('style');
  st.textContent = `
    .t { position: fixed; right: 18px; bottom: 18px; pointer-events: none;
         background: #1d1c25; color: #eceaf4; border: 1px solid ${ok ? '#38e0b0' : '#ff6b81'};
         border-radius: 10px; padding: 9px 14px; font: 600 12.5px system-ui, sans-serif;
         box-shadow: 0 8px 30px rgba(0,0,0,.5); transition: opacity .25s; }`;
  root.appendChild(st);
  root.innerHTML += `<div class="t">${ok ? '✓ sent to HyprFetch — download started' : '✕ HyprFetch daemon not reachable'}</div>`;
  document.documentElement.appendChild(host);
  setTimeout(() => {
    const t = root.querySelector('.t');
    if (t) t.style.opacity = '0';
    setTimeout(() => host.remove(), 400);
  }, 2600);
}

// ---------------------------------------------------------------------------
// background → content script (context menu "video qualities on this page")
// ---------------------------------------------------------------------------

try {
  api.runtime.onMessage.addListener((msg) => {
    if (msg?.type === 'showQualityPanel') {
      closePanel();
      openPanel(null);
    }
    return undefined;
  });
} catch (_) { /* noop */ }

// ---------------------------------------------------------------------------
// alt+click anywhere on media → straight to the daemon
// ---------------------------------------------------------------------------

const FILE_HINT = /\.(mp4|mkv|webm|mov|avi|flv|m4v|ts|mp3|m4a|aac|ogg|opus|wav|flac|zip|rar|7z|tar|gz|xz|bz2|iso|apk|dmg|exe|msi|deb|rpm|appimage|epub|pdf|torrent)(\?|#|$)/i;

window.addEventListener(
  'click',
  (ev) => {
    if (!ev.altKey || !buttonsOn) return;
    const el = ev.target.closest?.('img, video, audio, a');
    if (!el) return;
    const href = el.href || el.currentSrc || el.src || '';
    const isLink = el.tagName === 'A';
    // Links: only take over real media/file links, not ordinary navigation.
    if (isLink && !(el.download || FILE_HINT.test(href || ''))) return;
    if (!href) {
      if (el.tagName === 'VIDEO' || el.tagName === 'AUDIO') {
        ev.preventDefault();
        ev.stopPropagation();
        openPanel(el);
      }
      return;
    }
    if (!/^https?:/i.test(href)) {
      if (el.tagName === 'VIDEO' || el.tagName === 'AUDIO') {
        ev.preventDefault();
        ev.stopPropagation();
        openPanel(el);
      }
      return;
    }
    ev.preventDefault();
    ev.stopPropagation();
    void send({ type: 'download', url: href, filename: null, page_url: location.href }).then(
      (res) => toastBadge(res?.ok),
    );
  },
  true, // capture: beat the page's own handlers
);

// ---------------------------------------------------------------------------
// scanning — attach pills to <video>/<audio> (+ big images on hover)
// ---------------------------------------------------------------------------

function scan() {
  if (!buttonsOn) return;
  for (const el of document.querySelectorAll('video, audio')) {
    attachPill(el, false);
  }
  for (const img of document.querySelectorAll('img')) {
    if (pills.has(img)) continue;
    const r = img.getBoundingClientRect();
    if (r.width >= MIN_IMG_PX && r.height >= MIN_IMG_PX && /^https?:/.test(img.currentSrc || img.src || '')) {
      attachPill(img, true);
    }
  }
  repositionAll();
}

let scanTimer = null;
function scheduleScan() {
  if (scanTimer) return;
  scanTimer = setTimeout(() => {
    scanTimer = null;
    scan();
  }, 600);
}

let lastUrl = location.href;
function tick() {
  // SPA navigations (YouTube keeps the <video> element across "pages").
  if (location.href !== lastUrl) {
    lastUrl = location.href;
    closePanel();
  }
  // Drop pills whose element left the DOM; reposition the rest.
  for (const [el, entry] of pills) {
    if (!el.isConnected) {
      entry.host.remove();
      pills.delete(el);
    }
  }
  scan();
}

storageGet({ mediaButtons: true }).then((v) => {
  buttonsOn = v.mediaButtons !== false;
  scan();
  setInterval(tick, 1500);
  const mo = new MutationObserver(scheduleScan);
  mo.observe(document.documentElement, { childList: true, subtree: true });
});
