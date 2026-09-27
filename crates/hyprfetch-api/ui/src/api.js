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

export async function createTasks({ urls, saveDir, segments }) {
  const res = await fetch('/api/tasks', {
    method: 'POST',
    headers: jsonHeaders,
    body: JSON.stringify({
      urls,
      save_dir: saveDir || undefined,
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
