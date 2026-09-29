// Shared formatters for bytes, speeds, dates and task states.

export function fmtBytes(n) {
  if (n == null) return '?'
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
  let v = n
  let u = 0
  while (v >= 1024 && u < units.length - 1) { v /= 1024; u++ }
  return `${v >= 100 || u === 0 ? Math.round(v) : v.toFixed(1)} ${units[u]}`
}

export function fmtSpeed(bps) {
  return bps > 0 ? `${fmtBytes(bps)}/s` : '—'
}

export function fmtUptime(s) {
  if (!s) return '—'
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  return h > 0 ? `${h}h ${m}m` : m > 0 ? `${m}m` : `${s}s`
}

export function fmtPct(t) {
  if (!t || !t.total_bytes) return t?.state === 'complete' ? 100 : 0
  return Math.min(100, Math.round((t.downloaded_bytes / t.total_bytes) * 100))
}

export function fmtDate(ms) {
  if (!ms) return '—'
  const d = new Date(ms)
  return d.toLocaleString(undefined, {
    month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit',
  })
}

export function stateLabel(s) {
  return { queued: 'queued', downloading: 'downloading', paused: 'paused',
    complete: 'done', error: 'error', removed: 'removed' }[s] ?? s
}

export function badgeClass(s) {
  return { downloading: 'badge-info', paused: 'badge-warning',
    complete: 'badge-success', error: 'badge-error' }[s] ?? 'badge-ghost'
}

export function categoryIcon(cat) {
  return { video: '🎬', pictures: '🖼', music: '🎵', compress: '📦',
    documents: '📄', apps: '💽', other: '📁' }[cat] ?? '📁'
}
