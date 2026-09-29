// Theme system: 5 styles × dark/light = 10 daisyUI themes.
//
// Purely client-side: themes are static CSS variable sets compiled once by
// Tailwind; switching just flips `data-theme` on <html>. Preference lives in
// localStorage — no server round-trip, no extra RAM.

import { writable } from 'svelte/store'

export const THEME_STYLES = [
  { id: 'slate', label: 'Slate', desc: 'neutral gray · the classic look', dark: 'dim', light: 'light' },
  { id: 'ocean', label: 'Ocean', desc: 'calm blue', dark: 'night', light: 'winter' },
  { id: 'forest', label: 'Forest', desc: 'deep green', dark: 'forest', light: 'garden' },
  { id: 'coffee', label: 'Coffee', desc: 'warm brown & amber', dark: 'coffee', light: 'autumn' },
  { id: 'cyber', label: 'Cyber', desc: 'violet neon', dark: 'synthwave', light: 'valentine' },
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

function apply() {
  let style = 'slate'
  let mode = 'dark'
  themeStyle.subscribe((v) => (style = v))()
  themeMode.subscribe((v) => (mode = v))()
  const pair = THEME_STYLES.find((t) => t.id === style) ?? THEME_STYLES[0]
  const theme = mode === 'light' ? pair.light : pair.dark
  document.documentElement.setAttribute('data-theme', theme)
  try {
    localStorage.setItem(STYLE_KEY, style)
    localStorage.setItem(MODE_KEY, mode)
  } catch (_) { /* storage unavailable */ }
}

export function setThemeStyle(id) {
  themeStyle.set(id)
  apply()
}

export function setThemeMode(mode) {
  themeMode.set(mode)
  apply()
}

// Apply once at module load so the first paint already has the right theme.
apply()
