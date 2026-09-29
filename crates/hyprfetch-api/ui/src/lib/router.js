// Hash-based router: #/dashboard #/tasks #/settings #/updates
// Keeps the app a single-page build while giving every area its own URL,
// deep links (the server falls back to index.html for extension-less
// paths) and browser back/forward support.

import { readable } from 'svelte/store'

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

export const PAGES = [
  { id: 'dashboard', label: 'Dashboard', icon: '⌂', title: 'Overview' },
  { id: 'tasks', label: 'Tasks', icon: '☰', title: 'All downloads' },
  { id: 'settings', label: 'Settings', icon: '⚙', title: 'Save folders & options' },
  { id: 'updates', label: 'Updates', icon: '↑', title: 'Version & updates' },
]
