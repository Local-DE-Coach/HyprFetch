// HyprFetch Media Catcher — popup: list what this tab caught, hand picks
// to the daemon, and show the REAL format list for video pages (probed by
// the daemon's yt-dlp engine — 8K → 480p + MP3, exactly like the app's
// Add-dialog quality picker).

const api = globalThis.chrome ?? globalThis.browser;
const DAEMON_URL = 'http://127.0.0.1:7780';

const listEl = document.getElementById('list');
const emptyEl = document.getElementById('empty');
const connEl = document.getElementById('conn');
const pageEl = document.getElementById('page');
const toastEl = document.getElementById('toast');

// Pages where a probe is always worth it (the daemon gets every format
// the site serves: 8K/4K/2K/1080p/720p/480p, MP3, …).
const VIDEO_PAGE_RE = /(?:youtube\.com\/(?:watch|shorts|embed|live)|youtu\.be\/|m\.youtube\.com\/watch|vimeo\.com\/\d+|dailymotion\.com\/(?:video|embed)|twitch\.tv\/videos\/|soundcloud\.com\/[^/]+\/[^/?]+|tiktok\.com\/@[^/]+\/video|instagram\.com\/(?:reel|p)\/|facebook\.com\/watch|\/watch\/|\/video\/[\w-]{6,})/i;

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
  const m = Math.floor(s / 60);
  const r = s % 60;
  return `${m}:${String(r).padStart(2, '0')}`;
}

function kindLabel(item) {
  const ct = (item.media_type || '').toLowerCase();
  if (ct.startsWith('video/')) return 'video';
  if (ct.startsWith('audio/')) return 'audio';
  if (ct.startsWith('image/') || item.kind === 'image') return 'image';
  return 'file';
}

function toast(msg, ok) {
  toastEl.textContent = msg;
  toastEl.className = `toast ${ok ? 'ok' : 'err'}`;
  clearTimeout(toast._t);
  toast._t = setTimeout(() => toastEl.classList.add('hidden'), 2200);
}

async function sendToHyprfetch(item, btn) {
  btn.disabled = true;
  btn.textContent = '…';
  const res = await api.runtime.sendMessage({
    type: 'download',
    url: item.url,
    filename: item.filename,
    page_url: item.page_url,
  });
  if (res?.ok) {
    toast('Sent to HyprFetch ✓', true);
    btn.textContent = 'Sent ✓';
  } else {
    toast('HyprFetch daemon not reachable', false);
    btn.disabled = false;
    btn.textContent = 'Download';
  }
}

function renderItem(item) {
  const row = document.createElement('div');
  row.className = 'item';

  const top = document.createElement('div');
  top.className = 'row1';

  const name = document.createElement('span');
  name.className = 'name';
  name.textContent = item.filename || item.url.split('/').pop() || 'media';
  name.title = item.url;
  top.appendChild(name);

  const tag = document.createElement('span');
  tag.className = `tag ${kindLabel(item)}`;
  tag.textContent = kindLabel(item);
  top.appendChild(tag);

  const size = document.createElement('span');
  size.className = 'size';
  const s = fmtSize(item.size);
  if (s) size.textContent = s;
  top.appendChild(size);

  row.appendChild(top);

  const row2 = document.createElement('div');
  row2.className = 'row2';
  const url = document.createElement('span');
  url.className = 'url';
  url.textContent = item.url;
  row2.appendChild(url);

  const btn = document.createElement('button');
  btn.className = 'btn mini';
  btn.textContent = 'Download';
  btn.addEventListener('click', () => sendToHyprfetch(item, btn));
  row2.appendChild(btn);

  row.appendChild(row2);
  return row;
}

// ---- video formats section --------------------------------------------------

const videoSec = document.getElementById('videoSec');
const videoBody = document.getElementById('videoBody');
let currentTabUrl = '';

function videoMsg(html, spin) {
  videoBody.innerHTML = `<div class="vmsg">${spin ? '<span class="spin"></span>' : ''}${html}</div>`;
  videoSec.classList.remove('hidden');
}

async function probeTab(url) {
  currentTabUrl = url;
  videoMsg('asking the media engine for formats…', true);
  const res = await api.runtime.sendMessage({ type: 'probeMedia', url });
  if (url !== currentTabUrl) return; // stale (user rescanned)
  if (!res?.ok || !res?.data) {
    videoMsg('HyprFetch daemon is offline — start the app, then hit ↻', false);
    return;
  }
  const data = res.data;
  if (data.kind === 'file') {
    const f = data.file ?? {};
    videoBody.innerHTML = '';
    const row = document.createElement('div');
    row.className = 'qfile';
    const btn = document.createElement('button');
    btn.className = 'btn mini';
    btn.textContent = 'Download file';
    btn.addEventListener('click', async () => {
      btn.disabled = true;
      btn.textContent = '…';
      const r = await api.runtime.sendMessage({
        type: 'download', url, filename: f.filename ?? null, page_url: url,
      });
      toast(r?.ok ? 'Queued ✓' : 'daemon offline', !!r?.ok);
      btn.disabled = false;
      btn.textContent = 'Download file';
    });
    const name = document.createElement('span');
    name.className = 'qfile-name';
    name.textContent = f.filename || 'direct file on this page';
    row.append(name, btn);
    videoBody.appendChild(row);
    videoSec.classList.remove('hidden');
    return;
  }

  const media = data.media ?? {};
  const qualities = media.qualities ?? [];
  if (media.is_live) {
    videoMsg('live stream — downloading live media isn’t supported yet', false);
    return;
  }
  if (qualities.length === 0) {
    videoMsg('no downloadable formats found on this page', false);
    return;
  }

  videoBody.innerHTML = '';
  const head = document.createElement('div');
  head.className = 'vhead';
  head.textContent = `${media.title || 'video'}${media.duration ? ' · ' + fmtDur(media.duration) : ''} · ${qualities.length} formats`;
  head.title = media.title || '';
  videoBody.appendChild(head);

  const grid = document.createElement('div');
  grid.className = 'qgrid';
  for (const q of qualities) {
    const chip = document.createElement('button');
    chip.className = `qchip${q.audio_only ? ' audio' : ''}`;
    const meta = [
      q.container?.toUpperCase(),
      q.size_bytes ? `${q.size_est ? '~ ' : ''}${fmtSize(q.size_bytes)}` : '',
    ]
      .filter(Boolean)
      .join(' · ');
    chip.innerHTML = `<span class="ql">${q.label}</span><span class="qm">${meta}</span>`;
    chip.title = q.note ? `${q.label} — ${q.note}` : q.label;
    chip.addEventListener('click', async () => {
      for (const c of grid.querySelectorAll('.qchip')) c.classList.add('busy');
      chip.querySelector('.ql').textContent = 'queueing…';
      const r = await api.runtime.sendMessage({
        type: 'mediaDownload',
        url: media.webpage_url || url,
        quality: q.id,
        audio_only: !!q.audio_only,
      });
      toast(
        r?.ok ? `queued: ${media.title || 'video'} — ${q.label} ✓` : 'daemon refused the download',
        !!r?.ok,
      );
      for (const c of grid.querySelectorAll('.qchip')) c.classList.remove('busy');
      chip.querySelector('.ql').textContent = q.label;
    });
    grid.appendChild(chip);
  }
  videoBody.appendChild(grid);
  videoSec.classList.remove('hidden');
}

document.getElementById('videoRescan').addEventListener('click', () => {
  if (currentTabUrl) void probeTab(currentTabUrl);
});

async function init() {
  const [tab] = await api.tabs.query({ active: true, currentWindow: true });
  const res = await api.runtime.sendMessage({
    type: 'getMedia',
    tabId: tab?.id,
    includeAll: false,
  });

  connEl.className = `dot ${res?.daemonUp ? 'on' : 'off'}`;
  connEl.title = res?.daemonUp ? 'HyprFetch daemon connected' : 'HyprFetch daemon not running';
  if (tab?.title) pageEl.textContent = tab.title;

  // Video pages: show the daemon-probed format ladder automatically.
  const tabUrl = tab?.url || '';
  if (/^https?:/i.test(tabUrl) && !tabUrl.startsWith(DAEMON_URL)) {
    if (VIDEO_PAGE_RE.test(tabUrl)) {
      void probeTab(tabUrl);
    }
  }

  const items = (res?.items ?? []).slice().reverse(); // newest first
  if (items.length > 0) {
    emptyEl.style.display = 'none';
    for (const item of items) listEl.appendChild(renderItem(item));
  } else {
    emptyEl.style.display = '';
  }
}

document.getElementById('openApp').addEventListener('click', () => {
  api.tabs.create({ url: DAEMON_URL });
});

// ---- toggles ----------------------------------------------------------------

const autoEl = document.getElementById('autoCapture');

autoEl.addEventListener('change', () => {
  api.storage.local.set({ autoCapture: autoEl.checked });
  toast(autoEl.checked ? 'Auto-capture ON — downloads go to HyprFetch' : 'Auto-capture OFF — browser handles downloads', true);
});

new Promise((resolve) => {
  const r = api.storage.local.get({ autoCapture: true }, (v) => resolve(v ?? { autoCapture: true }));
  if (r && typeof r.then === 'function') r.then(resolve, () => resolve({ autoCapture: true }));
}).then((v) => {
  autoEl.checked = v.autoCapture !== false;
});

const mediaBtnEl = document.getElementById('mediaButtons');

mediaBtnEl.addEventListener('change', () => {
  api.storage.local.set({ mediaButtons: mediaBtnEl.checked });
  toast(mediaBtnEl.checked ? 'In-page ⬇ buttons ON' : 'In-page ⬇ buttons OFF', true);
});

new Promise((resolve) => {
  const r = api.storage.local.get({ mediaButtons: true }, (v) => resolve(v ?? { mediaButtons: true }));
  if (r && typeof r.then === 'function') r.then(resolve, () => resolve({ mediaButtons: true }));
}).then((v) => {
  mediaBtnEl.checked = v.mediaButtons !== false;
});

init();
