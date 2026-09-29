// ESLint flat config for the HyprFetch UI (JS sources only — Svelte files are
// handled by the compiler). The one rule that MUST stay enabled:
//
//   no-use-before-define  (variables)
//
// It catches the exact bug class that shipped v0.4.1 as a blank page:
// a module initializer reading a `const` declared later in the same module
// (router.js read `PAGES` from `page`'s initializer) → temporal dead zone →
// "Cannot access '$t' before initialization" → the whole SPA fails to mount.
// Combined with scripts/ui_boot_check.mjs (runtime TDZ check on the BUILT
// bundle in release.yml), both source and artifact are guarded.
import js from '@eslint/js'

export default [
  {
    ignores: ['dist/**', 'node_modules/**', '*.config.js'],
  },
  {
    files: ['src/**/*.js'],
    languageOptions: {
      ecmaVersion: 2023,
      sourceType: 'module',
      globals: {
        // browser globals referenced by the UI sources
        window: 'readonly', document: 'readonly', location: 'readonly',
        navigator: 'readonly', WebSocket: 'readonly', fetch: 'readonly',
        console: 'readonly', setTimeout: 'readonly', clearTimeout: 'readonly',
        setInterval: 'readonly', clearInterval: 'readonly',
        localStorage: 'readonly', requestAnimationFrame: 'readonly',
      },
    },
    rules: {
      ...js.configs.recommended.rules,
      'no-use-before-define': ['error', { functions: false, classes: false, variables: true }],
      'no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_', caughtErrors: 'none' }],
    },
  },
]
