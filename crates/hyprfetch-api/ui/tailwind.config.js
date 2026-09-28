/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{svelte,js}'],
  plugins: [require('daisyui')],
  daisyui: {
    // Single dark theme keeps the compiled CSS tiny (RAM/bundle friendly).
    themes: ['dim'],
    logs: false,
  },
}
