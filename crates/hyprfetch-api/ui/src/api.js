// Tiny REST + WS client for the HyprFetch API.

const jsonHeaders = { 'Content-Type': 'application/json' }

async function handle(res) {
  if (!res.ok) {
    let detail = `${res.status}`
    try {
      const body = await res.json()
      if (body && body.error) detail = body.error
    } catch (_) {
      /* body wasn't JSON; keep status code */
    }
    throw new Error(detail)
  }
  if (res.status === 204) return null
  return res.json()
}

export async function listTasks(state = 'active') {
  const res = await fetch(`/api/tasks?state=${encodeURIComponent(state)}`)
  const body = await handle(res)
  return body.tasks ?? []
}

export async function createTasks({ urls, saveDir, category, filename, segments }) {
  const res = await fetch('/api/tasks', {
    method: 'POST',
    headers: jsonHeaders,
    body: JSON.stringify({
      urls,
      save_dir: saveDir || undefined,
      category: category || undefined,
      filename: filename || undefined,
      segments: segments || undefined,
    }),
  })
  return handle(res)
}

export async function pauseTask(id) {
  return handle(await fetch(`/api/tasks/${id}/pause`, { method: 'POST' }))
}

export async function resumeTask(id) {
  return handle(await fetch(`/api/tasks/${id}/resume`, { method: 'POST' }))
}

export async function cancelTask(id) {
  return handle(await fetch(`/api/tasks/${id}/cancel`, { method: 'POST' }))
}

export async function retryTask(id) {
  return handle(await fetch(`/api/tasks/${id}/retry`, { method: 'POST' }))
}

export async function deleteTask(id) {
  return handle(await fetch(`/api/tasks/${id}`, { method: 'DELETE' }))
}

export async function deleteTaskWithFile(id) {
  return handle(await fetch(`/api/tasks/${id}?delete_file=true`, { method: 'DELETE' }))
}

// ---- file actions (desktop integration) ----------------------------------

/// Open the downloaded file with the system default app ("Open" button).
export async function openTaskFile(id) {
  return handle(await fetch(`/api/tasks/${id}/open`, { method: 'POST' }))
}

/// Open the containing folder in the system file manager ("GO" button).
export async function revealTaskFolder(id) {
  return handle(await fetch(`/api/tasks/${id}/reveal`, { method: 'POST' }))
}

/// Open one of HyprFetch's save folders in the file manager (clickable
/// folder cards on the Dashboard). Only save-folder paths are allowed.
export async function openFolder(path) {
  const res = await fetch('/api/open-folder', {
    method: 'POST',
    headers: jsonHeaders,
    body: JSON.stringify({ path }),
  })
  return handle(res)
}

/// RAM + CPU used by THIS app only (footer widget / Settings card).
export async function getUsage() {
  return handle(await fetch('/api/system/usage'))
}

/// Enter background (low-usage) mode — the app keeps running (downloads
/// continue) but wakes up far less. Reopen with `hyprfetch open`.
export async function powerQuiet() {
  return handle(await fetch('/api/power/quiet', { method: 'POST' }))
}

/// Leave background mode (back to normal 1s activity ticks).
export async function powerWake() {
  return handle(await fetch('/api/power/wake', { method: 'POST' }))
}

// ---- download info probe (confirm dialog) --------------------------------

/// Probe a URL before creating a task: final URL, size, accept-ranges and
/// the resolved save path. Powers the IDM-style confirm popup.
export async function inspectUrl({ url, category, saveDir, filename }) {
  const res = await fetch('/api/inspect', {
    method: 'POST',
    headers: jsonHeaders,
    body: JSON.stringify({
      url,
      category: category || undefined,
      save_dir: saveDir || undefined,
      filename: filename || undefined,
    }),
  })
  return handle(res)
}

export async function getQos() {
  return handle(await fetch('/api/qos'))
}

export async function setQos(enabled, targetBps) {
  const res = await fetch('/api/qos', {
    method: 'PUT',
    headers: jsonHeaders,
    body: JSON.stringify({ enabled, target_bps: targetBps }),
  })
  return handle(res)
}

// ---- server info + in-app updates --------------------------------------

export async function getServerInfo() {
  return handle(await fetch('/api/server'))
}

export async function getCategories() {
  return handle(await fetch('/api/categories'))
}

export async function getSettings() {
  return handle(await fetch('/api/settings'))
}

export async function patchSettings(patch) {
  const res = await fetch('/api/settings', {
    method: 'PATCH',
    headers: jsonHeaders,
    body: JSON.stringify(patch),
  })
  return handle(res)
}

export async function checkUpdate() {
  return handle(await fetch('/api/update/check'))
}

export async function applyUpdate(restart = true) {
  return handle(
    await fetch(`/api/update/apply?restart=${restart ? 'true' : 'false'}`, { method: 'POST' }),
  )
}

// One-time one-click update setup: opens a terminal window where the user
// enters their password ONCE; afterwards in-app updates run silently
// (narrow sudoers rule + root-owned helper). Requires a pending staged
// update (applyUpdate answered needs_password first).
export async function authorizeUpdate() {
  return handle(await fetch('/api/update/authorize', { method: 'POST' }))
}

// Poll the one-time setup status: { running, terminal, done, restarted, error }.
export async function authorizeStatus() {
  return handle(await fetch('/api/update/authorize/status'))
}

export async function restartServer() {
  return handle(await fetch('/api/update/restart', { method: 'POST' }))
}

// Remove stale shadowing hyprfetch copies found on PATH (e.g. an old
// install.sh build in /usr/local/bin keeping the old UI alive after an
// update). Returns { removed, failed, owned, message }.
export async function fixStaleCopies() {
  return handle(await fetch('/api/update/stale-copies/fix', { method: 'POST' }))
}

export function connectEvents(onEvent) {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  const ws = new WebSocket(`${proto}://${location.host}/ws`)
  ws.onmessage = (m) => {
    try {
      onEvent(JSON.parse(m.data))
    } catch (_) {
      /* malformed frame — ignore */
    }
  }
  return ws
}
