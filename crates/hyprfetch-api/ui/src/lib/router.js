// Hash-based router: #/dashboard #/tasks #/settings #/updates
// Keeps the app a single-page build while giving every area its own URL,
// deep links (the server falls back to index.html for extension-less
// paths) and browser back/forward support.
//
// NOTE: `PAGES` MUST stay ABOVE `current()`/`page`. `page`'s initializer
// calls `current()` at module-evaluation time, and `current()` reads
// `PAGES`. With `PAGES` below, that read hits the temporal dead zone:
//   "Uncaught ReferenceError: Cannot access 'PAGES' before initialization"
// (minified as "Cannot access '$t' before initialization") — the whole
// SPA then fails to mount and the page renders blank. That is exactly the
// v0.4.1 regression this ordering fixes; keep the declaration order and
// the smoke test (scripts/smoke_ui.sh) guards it.

import { readable } from 'svelte/store'

export const PAGES = [
  { id: 'dashboard', label: 'Dashboard', icon: '⌂', title: 'Overview' },
  { id: 'tasks', label: 'Tasks', icon: '☰', title: 'All downloads' },
  { id: 'settings', label: 'Settings', icon: '⚙', title: 'Save folders & options' },
  { id: 'updates', label: 'Updates', icon: '↑', title: 'Version & updates' },
]

function current() {
  const h = (location.hash || '').replace(/^#\/?/, '')
  const id = h.split('?')[0] || 'dashboard'
  return PAGES.some((p) => p.id === id) ? id : 'dashboard'
}

export const page = readable(current(), (set) => {
  const onChange = () => set(current())
  window.addEventListener('hashchange', onChange)
  return () => window.removeEventListener('hashchange', onChange)
})

export function nav(id) {
  location.hash = `#/${id}`
}
