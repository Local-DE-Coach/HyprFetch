// Global shared state for all pages: live task lists, server info,
// settings, category folders and one shared WebSocket connection.

import { writable } from 'svelte/store'
import {
  listTasks,
  createTasks,
  pauseTask,
  resumeTask,
  cancelTask,
  retryTask,
  deleteTask,
  deleteTaskWithFile,
  getQos,
  setQos,
  getServerInfo,
  getSettings,
  patchSettings,
  getCategories,
  connectEvents,
  openTaskFile,
  revealTaskFolder,
} from '../api.js'

// ---- live state ----------------------------------------------------------
export const active = writable([])       // queued / downloading / paused
export const finished = writable([])     // complete / error
export const wsConnected = writable(false)
export const globalSpeed = writable(0)
export const activeCount = writable(0)
export const serverInfo = writable({})
export const settings = writable({})
export const categories = writable({ base: '', categorize: true, categories: [] })
export const toast = writable('')
export const showAdd = writable(false)

// Floating download progress panel (IDM-style transfer monitor).
// 'show' | 'min' (bubble) | 'hide' — persisted so the choice survives reloads.
const FLOAT_KEY = 'hf_floatbar'
function initialFloat() {
  try {
    const v = localStorage.getItem(FLOAT_KEY)
    if (v === 'show' || v === 'hide' || v === 'min') return v
  } catch (_) { /* storage unavailable */ }
  return 'show'
}
export const floatPanel = writable(initialFloat())
export function setFloatPanel(v) {
  floatPanel.set(v)
  try { localStorage.setItem(FLOAT_KEY, v) } catch (_) { /* ignore */ }
}

let toastTimer
export function notify(msg) {
  toast.set(msg)
  clearTimeout(toastTimer)
  toastTimer = setTimeout(() => toast.set(''), 6000)
}

// ---- refreshers ----------------------------------------------------------
export async function refreshTasks() {
  try {
    active.set(await listTasks('active'))
    finished.set(await listTasks('completed'))
  } catch (e) {
    notify(`failed to load tasks: ${e.message}`)
  }
}

export async function refreshServer() {
  try { serverInfo.set(await getServerInfo()) } catch (_) { /* optional */ }
}

export async function refreshSettings() {
  try { settings.set(await getSettings()) } catch (_) { /* defaults */ }
}

export async function refreshCategories() {
  try { categories.set(await getCategories()) } catch (_) { /* defaults */ }
}

export async function saveSettings(patch) {
  const next = await patchSettings(patch)
  settings.set(next)
  // The categories layout may have changed with the new settings.
  await refreshCategories()
  return next
}

export async function addDownload({ urls, category, saveDir, filename, segments }) {
  await createTasks({ urls, category, saveDir, filename, segments })
  await refreshTasks()
}

// ---- task actions --------------------------------------------------------
export async function doTaskAction(task, action) {
  try {
    if (action === 'pause') await pauseTask(task.id)
    else if (action === 'resume') await resumeTask(task.id)
    else if (action === 'cancel') await cancelTask(task.id)
    else if (action === 'retry') await retryTask(task.id)
    else if (action === 'delete') await deleteTask(task.id)
    else if (action === 'delete-file') await deleteTaskWithFile(task.id)
    await refreshTasks()
    await refreshServer()
  } catch (e) {
    notify(`${action} failed: ${e.message}`)
  }
}

// ---- file actions (desktop integration) ----------------------------------
export async function openFile(task) {
  try {
    await openTaskFile(task.id)
    notify(`opening ${task.filename}…`)
  } catch (e) {
    notify(`open failed: ${e.message}`)
  }
}

export async function revealFile(task) {
  try {
    await revealTaskFolder(task.id)
    notify(`showing folder of ${task.filename}…`)
  } catch (e) {
    notify(`go failed: ${e.message}`)
  }
}

export { getQos, setQos }

// ---- live events (one WS for the whole app) -------------------------------
let ws

function handleEvent(ev) {
  if (ev.event === 'global:speed') {
    globalSpeed.set(ev.speed_bps ?? 0)
    activeCount.set(ev.active_tasks ?? 0)
    return
  }
  if (!ev.task_id) return
  if (ev.event === 'task:progress') {
    // Update in place — writable.set always notifies for objects.
    active.update((list) => {
      const t = list.find((x) => x.id === ev.task_id)
      if (t) {
        t.downloaded_bytes = ev.downloaded_bytes
        t.total_bytes = ev.total_bytes ?? t.total_bytes
        t._speed = ev.speed_bps ?? 0
      }
      return list
    })
  } else if (ev.event === 'task:state') {
    refreshTasks()
  }
}

export function initApp() {
  refreshTasks()
  refreshServer()
  refreshSettings()
  refreshCategories()
  try { ws?.close() } catch (_) { /* first init */ }
  ws = connectEvents(handleEvent)
  ws.onopen = () => wsConnected.set(true)
  ws.onclose = () => wsConnected.set(false)
}
