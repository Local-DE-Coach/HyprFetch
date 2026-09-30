// Theme system: 5 styles × dark/light = 10 daisyUI themes.
//
// Themes are static CSS variable sets compiled once by Tailwind; switching
// just flips `data-theme` on <html>. The chosen theme is stored on the
// SERVER (settings `ui_theme_style` / `ui_theme_mode`, v0.4.6) so every
// browser and device that opens the UI shows the SAME theme; localStorage
// only mirrors the last known value to avoid a flash on reload.

import { writable } from 'svelte/store'
import { patchSettings } from '../api.js'

export const THEME_STYLES = [
  { id: 'slate', label: 'Indigo', desc: 'vivid indigo · the colorful default', dark: 'dim', light: 'light' },
  { id: 'ocean', label: 'Ocean', desc: 'sky blue on deep sea', dark: 'night', light: 'winter' },
  { id: 'forest', label: 'Forest', desc: 'fresh emerald green', dark: 'forest', light: 'garden' },
  { id: 'coffee', label: 'Sunset', desc: 'warm amber & cream', dark: 'coffee', light: 'autumn' },
  { id: 'cyber', label: 'Neon', desc: 'fuchsia & violet glow', dark: 'synthwave', light: 'valentine' },
]

const STYLE_KEY = 'hf_theme_style'
const MODE_KEY = 'hf_theme_mode'

function initialStyle() {
  try {
    const saved = localStorage.getItem(STYLE_KEY)
    if (saved && THEME_STYLES.some((t) => t.id === saved)) return saved
  } catch (_) { /* storage unavailable */ }
  return 'slate'
}

function initialMode() {
  try {
    const saved = localStorage.getItem(MODE_KEY)
    if (saved === 'dark' || saved === 'light') return saved
    // First visit: follow the browser preference.
    if (window.matchMedia?.('(prefers-color-scheme: light)').matches) return 'light'
  } catch (_) { /* storage unavailable */ }
  return 'dark'
}

export const themeStyle = writable(initialStyle())
export const themeMode = writable(initialMode())

// Current values (kept outside the stores to avoid redundant server writes).
let currentStyle = initialStyle()
let currentMode = initialMode()

function apply(style, mode) {
  const pair = THEME_STYLES.find((t) => t.id === style) ?? THEME_STYLES[0]
  const theme = mode === 'light' ? pair.light : pair.dark
  document.documentElement.setAttribute('data-theme', theme)
  try {
    localStorage.setItem(STYLE_KEY, style)
    localStorage.setItem(MODE_KEY, mode)
  } catch (_) { /* storage unavailable */ }
}

// Push a theme change to the server so ALL browsers see it (fire-and-forget:
// the server is the source of truth, but the UI must never block on it).
function persistToServer(style, mode) {
  patchSettings({ ui_theme_style: style, ui_theme_mode: mode }).catch(() => {
    /* server unreachable — the local choice still applies for this tab */
  })
}

export function setThemeStyle(id) {
  if (!THEME_STYLES.some((t) => t.id === id) || id === currentStyle) return
  currentStyle = id
  themeStyle.set(id)
  apply(id, currentMode)
  persistToServer(id, currentMode)
}

export function setThemeMode(mode) {
  if ((mode !== 'dark' && mode !== 'light') || mode === currentMode) return
  currentMode = mode
  themeMode.set(mode)
  apply(currentStyle, mode)
  persistToServer(currentStyle, mode)
}

// Apply once at module load so the first paint already has the right theme.
apply(currentStyle, currentMode)

// Server → client sync: called whenever settings are (re)loaded. The server
// wins when it has a stored theme — that's what makes a NEW browser open
// with the same look. When the server has no preference yet (fresh install),
// whatever this browser already uses is pushed up once so the sync begins.
export function syncThemeFromServer(map) {
  if (!map) return
  const style = map.ui_theme_style
  const mode = map.ui_theme_mode
  const haveStyle = style && THEME_STYLES.some((t) => t.id === style)
  const haveMode = mode === 'dark' || mode === 'light'

  if (haveStyle || haveMode) {
    const nextStyle = haveStyle ? style : currentStyle
    const nextMode = haveMode ? mode : currentMode
    if (nextStyle !== currentStyle || nextMode !== currentMode) {
      currentStyle = nextStyle
      currentMode = nextMode
      themeStyle.set(currentStyle)
      themeMode.set(currentMode)
      apply(currentStyle, currentMode)
    }
    return
  }

  // Server has no theme yet — seed it once per browser so every other
  // browser/device opens with the same look from now on.
  let seeded = false
  try { seeded = localStorage.getItem('hf_theme_synced') === '1' } catch (_) { /* ignore */ }
  if (!seeded) {
    persistToServer(currentStyle, currentMode)
    try { localStorage.setItem('hf_theme_synced', '1') } catch (_) { /* ignore */ }
  }
}
