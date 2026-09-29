// Boot-check built UI bundle(s) in Node — TDZ-at-boot detector.
//
// WHY: v0.4.1 shipped a blank UI. `router.js` read the `PAGES` const from
// `page`'s module initializer before `PAGES` was initialized — a
// temporal-dead-zone crash ("Cannot access '$t' before initialization")
// that killed the whole SPA at boot. Nothing caught it, so it shipped.
//
// HOW: import the bundle in Node and fail on TDZ errors. A plain import is
// NOT enough — Node dies on the first browser-API access (e.g. `location`
// is not defined) BEFORE the TDZ line is ever reached, which masks the bug.
// So this installs minimal browser stubs (location/window/navigator/
// document) that let module evaluation proceed far enough for every module
// initializer to actually run. Any "Cannot access X before initialization"
// anywhere at module-eval time then surfaces and fails the release.
// Deep mount-time TypeErrors from the fake DOM are expected and pass —
// they cannot blank the page the way a boot-time TDZ does (mounting happens
// after a successful module evaluation; v0.4.1 never even got there).
//
// Usage (release.yml, after `vite build`; shell expands the glob):
//   node scripts/ui_boot_check.mjs crates/hyprfetch-api/ui/dist/assets/index-*.js
// Exit 0 = safe to embed; exit 1 = TDZ boot bug; exit 2 = usage error.
import { pathToFileURL } from 'node:url'
import { resolve } from 'node:path'

// ---- minimal browser environment (just enough for module evaluation) -----
const noop = () => {}
// Falsy-safe fake element: truthy object, chainable, benign attribute values.
const fakeEl = {
  style: {},
  relList: { supports: () => false },
  sheet: { cssRules: [], insertRule: noop, deleteRule: noop },
  classList: { add: noop, remove: noop, contains: () => false, toggle: noop },
  attributes: [],
  children: [],
  firstChild: null,
  parentNode: null,
  appendChild: (c) => c,
  removeChild: noop,
  remove: noop,
  setAttribute: noop,
  getAttribute: () => null,
  removeAttribute: noop,
  addEventListener: noop,
  removeEventListener: noop,
  attachShadow: () => fakeEl,
  querySelector: () => null,
  querySelectorAll: () => [],
  cloneNode: () => fakeEl,
  contains: () => false,
}
const documentStub = {
  createElement: () => ({ ...fakeEl }),
  createElementNS: () => ({ ...fakeEl }),
  createTextNode: () => ({}),
  createDocumentFragment: () => ({ ...fakeEl }),
  getElementById: () => null,
  querySelector: () => null,
  querySelectorAll: () => [],
  getElementsByTagName: () => [],
  getElementsByClassName: () => [],
  head: { ...fakeEl, appendChild: noop },
  body: { ...fakeEl },
  documentElement: { ...fakeEl, style: { setProperty: noop } },
  addEventListener: noop,
  removeEventListener: noop,
  visibilityState: 'visible',
  hasFocus: () => true,
}
if (typeof globalThis.location === 'undefined') {
  globalThis.location = {
    hash: '', protocol: 'http:', host: '127.0.0.1:7780', hostname: '127.0.0.1',
    port: '7780', href: 'http://127.0.0.1:7780/', origin: 'http://127.0.0.1:7780',
    pathname: '/', search: '', replace: noop, assign: noop,
  }
}
if (typeof globalThis.navigator === 'undefined') {
  globalThis.navigator = { userAgent: 'ui-boot-check' }
}
if (typeof globalThis.window === 'undefined') {
  globalThis.window = globalThis
}
if (typeof globalThis.document === 'undefined') {
  globalThis.document = documentStub
}
// Remaining globals Svelte 4 internals + Vite helpers touch at eval time.
if (typeof globalThis.MutationObserver === 'undefined') {
  globalThis.MutationObserver = class {
    observe() {} disconnect() {} takeRecords() { return [] }
  }
}
if (typeof globalThis.ResizeObserver === 'undefined') {
  globalThis.ResizeObserver = class { observe() {} disconnect() {} unobserve() {} }
}
if (typeof globalThis.IntersectionObserver === 'undefined') {
  globalThis.IntersectionObserver = class { observe() {} disconnect() {} unobserve() {} }
}
if (typeof globalThis.requestAnimationFrame === 'undefined') {
  globalThis.requestAnimationFrame = (cb) => setTimeout(() => cb(0), 0)
  globalThis.cancelAnimationFrame = (id) => clearTimeout(id)
}
if (typeof globalThis.getComputedStyle === 'undefined') {
  globalThis.getComputedStyle = () => ({ getPropertyValue: () => '' })
}
if (typeof globalThis.matchMedia === 'undefined') {
  globalThis.matchMedia = () => ({ matches: false, media: '', addEventListener: noop, removeEventListener: noop, addListener: noop, removeListener: noop })
}
if (typeof globalThis.requestIdleCallback === 'undefined') {
  globalThis.requestIdleCallback = (cb) => setTimeout(() => cb({ didTimeout: false, timeRemaining: () => 0 }), 0)
  globalThis.cancelIdleCallback = (id) => clearTimeout(id)
}
if (typeof globalThis.Event === 'undefined') {
  globalThis.Event = class { constructor(t) { this.type = t } preventDefault() {} stopPropagation() {} }
}
if (typeof globalThis.CustomEvent === 'undefined') {
  globalThis.CustomEvent = class { constructor(t) { this.type = t } preventDefault() {} stopPropagation() {} }
}
if (typeof globalThis.CSSStyleSheet === 'undefined') {
  globalThis.CSSStyleSheet = class { insertRule() { return 0 } deleteRule() {} cssRules = [] }
}
if (typeof globalThis.getComputedStyle === 'undefined') {
  globalThis.getComputedStyle = () => ({ getPropertyValue: () => '' })
}

// ---- load the bundles ------------------------------------------------------
const files = process.argv.slice(2)
if (files.length === 0) {
  console.error('usage: node ui_boot_check.mjs <bundle.js> [more.js...]')
  process.exit(2)
}

let failed = false
for (const file of files) {
  const url = pathToFileURL(resolve(file)).href
  try {
    await import(url)
    console.log(`OK       ${file} (module graph fully evaluated)`)
  } catch (e) {
    if (/before initialization/i.test(e?.message ?? '')) {
      console.error(`TDZ BUG  ${file}: ${e.message}`)
      console.error('         a module initializer reads a binding declared later —')
      console.error('         the SPA dies at boot and renders a blank page. Fix the')
      console.error('         declaration order before releasing.')
      failed = true
    } else {
      console.log(`OK       ${file} (${e?.constructor?.name}: ${String(e?.message).slice(0, 60)} — not a boot TDZ)`)
    }
  }
}
process.exit(failed ? 1 : 0)
