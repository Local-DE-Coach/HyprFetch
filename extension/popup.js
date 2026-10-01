// HyprFetch Media Catcher — popup: list what this tab caught, hand picks
// to the daemon.

const api = globalThis.chrome ?? globalThis.browser;
const DAEMON_URL = 'http://127.0.0.1:7780';

const listEl = document.getElementById('list');
const emptyEl = document.getElementById('empty');
const connEl = document.getElementById('conn');
const pageEl = document.getElementById('page');
const toastEl = document.getElementById('toast');

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

init();
