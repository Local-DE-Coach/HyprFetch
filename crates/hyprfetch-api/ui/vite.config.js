import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

// Build into ui/dist — the folder rust-embed embeds at compile time.
export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2020',
  },
  server: {
    // `npm run dev` against a locally running hyprfetch server.
    proxy: {
      '/api': 'http://127.0.0.1:7780',
      '/ws': { target: 'ws://127.0.0.1:7780', ws: true },
    },
  },
})
