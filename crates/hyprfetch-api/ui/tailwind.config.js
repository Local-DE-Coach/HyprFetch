/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{svelte,js}'],
  plugins: [require('daisyui')],
  daisyui: {
    // 5 theme styles × dark/light. Themes are pure CSS variable sets
    // (~30 rules each) — static CSS, zero runtime cost, RAM-friendly.
    themes: [
      // Slate — neutral gray + green (the original look)
      'dim', 'light',
      // Ocean — blue
      'night', 'winter',
      // Forest — green
      'forest', 'garden',
      // Coffee — warm brown/amber
      'coffee', 'autumn',
      // Cyber — violet/pink neon
      'synthwave', 'valentine',
    ],
    logs: false,
  },
}
