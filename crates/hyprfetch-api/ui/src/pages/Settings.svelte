<script>
  // Settings — save folders (base + per-category overrides), auto-sort,
  // concurrency, QoS bandwidth cap, security, appearance (5 theme styles ×
  // dark/light) and the update channel that powers the in-app updater.
  import { onMount } from 'svelte'
  import { fmtBytes } from '../lib/format.js'
  import { settings, categories, saveSettings, getQos, setQos, notify } from '../lib/store.js'
  import { THEME_STYLES, themeStyle, themeMode, setThemeStyle, setThemeMode } from '../lib/theme.js'

  // ---- save folders ----
  let baseDir = ''
  let categorize = true
  let catDirs = {}          // name -> dir string
  let loadedBase = ''
  let savingFolders = false

  $: if ($categories) {
    if ($categories.base && !loadedBase) {
      baseDir = $categories.base
      loadedBase = $categories.base
    }
    if ($categories.categorize !== undefined && categorize !== $categories.categorize && firstCatSync) {
      categorize = $categories.categorize
    }
  }
  let firstCatSync = true

  $: if ($categories?.categories) {
    const next = {}
    for (const c of $categories.categories) next[c.name] = c.dir
    if (!Object.keys(catDirs).length) catDirs = next
  }

  async function saveFolders() {
    savingFolders = true
    try {
      const patch = {}
      if (baseDir.trim() && baseDir.trim() !== loadedBase) patch.download_dir = baseDir.trim()
      if (patch.download_dir) patch.categorize = categorize ? 'true' : 'false'
      else if (categorize !== $categories.categorize) patch.categorize = categorize ? 'true' : 'false'
      for (const c of $categories.categories) {
        if (catDirs[c.name] !== c.dir) patch[`category_dir_${c.name}`] = catDirs[c.name]
      }
      if (!Object.keys(patch).length) { notify('no folder changes to save'); return }
      await saveSettings(patch)
      loadedBase = ''
      catDirs = {}
      firstCatSync = false
      notify('folders saved ✓ — directory layout rebuilt')
    } catch (e) {
      notify(`save failed: ${e.message}`)
    } finally {
      savingFolders = false
    }
  }

  // ---- concurrency ----
  let segmentsDefault = 8
  let maxConcurrent = 3
  let savingQueue = false
  $: if ($settings?.segments_default && segmentsDefault === 8) {
    segmentsDefault = parseInt($settings.segments_default) || 8
  }
  $: if ($settings?.max_concurrent_tasks && maxConcurrent === 3) {
    maxConcurrent = parseInt($settings.max_concurrent_tasks) || 3
  }
  async function saveQueue() {
    savingQueue = true
    try {
      await saveSettings({
        segments_default: String(segmentsDefault),
        max_concurrent_tasks: String(maxConcurrent),
      })
      notify('queue settings saved ✓')
    } catch (e) {
      notify(`save failed: ${e.message}`)
    } finally {
      savingQueue = false
    }
  }

  // ---- QoS ----
  let qosEnabled = false
  let qosMbps = 5
  let qosSaving = false
  onMount(async () => {
    try {
      const q = await getQos()
      qosEnabled = q.enabled
      if (q.target_bps) qosMbps = Math.round((q.target_bps / 1_000_000) * 100) / 100
    } catch (_) { /* defaults */ }
  })
  async function saveQos() {
    qosSaving = true
    try {
      const bps = qosEnabled ? Math.max(1, Math.round(qosMbps * 1_000_000)) : 0
      await setQos(qosEnabled, bps)
      notify(qosEnabled ? `QoS on — capped at ${fmtBytes(bps)}/s` : 'QoS off — full speed')
    } catch (e) {
      notify(`error: ${e.message}`)
    } finally {
      qosSaving = false
    }
  }

  // ---- security ----
  let ssrfBlock = true
  let savingSec = false
  $: if ($settings?.ssrf_block_private !== undefined && ssrfBlock) {
    ssrfBlock = $settings.ssrf_block_private !== 'false'
  }
  async function saveSecurity() {
    savingSec = true
    try {
      await saveSettings({ ssrf_block_private: ssrfBlock ? 'true' : 'false' })
      notify('security setting saved ✓')
    } catch (e) {
      notify(`save failed: ${e.message}`)
    } finally {
      savingSec = false
    }
  }
  // ---- appearance (themes) ----
  // 5 styles × dark/light, all static CSS — switching costs nothing.

  // Representative daisyUI colors per style for the picker swatches.
  const SWATCH = {
    slate: { primary: '#22c55e', base: '#1d232a', accent: '#71ccdf' },
    ocean: { primary: '#38bdf8', base: '#0f172a', accent: '#60a5fa' },
    forest: { primary: '#4ade80', base: '#171d1a', accent: '#2f7461' },
    coffee: { primary: '#fbbd23', base: '#291c16', accent: '#d19a66' },
    cyber: { primary: '#e879f9', base: '#1a1032', accent: '#7c3aed' },
  }

</script>

<div class="grid gap-5 lg:grid-cols-2">
  <!-- Save folders -->
  <section class="card border border-base-300 bg-base-200 shadow-sm lg:col-span-2">
    <div class="card-body gap-3 p-5">
      <h2 class="card-title text-base">Save folders</h2>
      <p class="text-xs opacity-60">
        Downloads are sorted by file type into these folders. Folders are created
        automatically — here or on disk — the moment you save. Leave a category
        folder unchanged to use the default <code class="font-mono">&lt;base&gt;/&lt;category&gt;</code>.
      </p>

      <label class="grid max-w-xl gap-1.5 text-sm">
        <span>Base download folder <span class="opacity-50">(main location)</span></span>
        <input type="text" class="input input-bordered font-mono" bind:value={baseDir}
          placeholder="~/Downloads" />
      </label>

      <label class="flex w-fit cursor-pointer items-center gap-2 text-sm">
        <input type="checkbox" class="toggle toggle-primary" bind:checked={categorize} />
        Auto-sort downloads by type {categorize ? '(on)' : '(off)'}
      </label>

      <div class="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
        {#each $categories.categories as c (c.name)}
          <label class="grid gap-1 text-xs">
            <span class="capitalize opacity-70">{c.name}</span>
            <input type="text" class="input input-bordered input-sm font-mono"
              bind:value={catDirs[c.name]} />
          </label>
        {/each}
      </div>

      <div class="card-actions justify-end">
        <button class="btn btn-primary btn-sm" disabled={savingFolders} on:click={saveFolders}>
          {savingFolders ? 'Saving…' : 'Save folders'}
        </button>
      </div>
    </div>
  </section>

  <!-- Queue -->
  <section class="card border border-base-300 bg-base-200 shadow-sm">
    <div class="card-body gap-3 p-5">
      <h2 class="card-title text-base">Queue</h2>
      <label class="grid gap-1.5 text-sm">
        <span>Default segments per download <span class="opacity-50">(1–32)</span></span>
        <input type="number" min="1" max="32" class="input input-bordered w-28" bind:value={segmentsDefault} />
      </label>
      <label class="grid gap-1.5 text-sm">
        <span>Max simultaneous downloads</span>
        <input type="number" min="1" max="16" class="input input-bordered w-28" bind:value={maxConcurrent} />
      </label>
      <div class="card-actions justify-end">
        <button class="btn btn-primary btn-sm" disabled={savingQueue} on:click={saveQueue}>
          {savingQueue ? 'Saving…' : 'Save queue'}
        </button>
      </div>
    </div>
  </section>

  <!-- QoS + security -->
  <section class="card border border-base-300 bg-base-200 shadow-sm">
    <div class="card-body gap-3 p-5">
      <h2 class="card-title text-base">Bandwidth &amp; security</h2>
      <div class="flex flex-wrap items-center gap-4">
        <label class="flex cursor-pointer items-center gap-2 text-sm">
          <input type="checkbox" class="toggle toggle-primary" bind:checked={qosEnabled} on:change={saveQos} disabled={qosSaving} />
          QoS cap {qosEnabled ? 'on' : 'off'}
        </label>
        <label class="flex items-center gap-2 text-sm">
          <input type="number" min="0.1" step="0.5" class="input input-bordered input-sm w-24"
            bind:value={qosMbps} on:change={saveQos} disabled={!qosEnabled || qosSaving} />
          MiB/s <span class="opacity-50">(total, all tasks)</span>
        </label>
      </div>
      <label class="flex w-fit cursor-pointer items-center gap-2 text-sm">
        <input type="checkbox" class="toggle toggle-warning" bind:checked={ssrfBlock} />
        Block private/loopback download URLs (SSRF protection)
      </label>
      <div class="card-actions justify-end">
        <button class="btn btn-primary btn-sm" disabled={savingSec} on:click={saveSecurity}>
          {savingSec ? 'Saving…' : 'Save security'}
        </button>
      </div>
    </div>
  </section>

  <!-- Appearance: 5 theme styles × dark/light -->
  <section class="card border border-base-300 bg-base-200 shadow-sm lg:col-span-2">
    <div class="card-body gap-3 p-5">
      <h2 class="card-title text-base">Appearance <span class="badge badge-sm badge-ghost">5 styles · dark &amp; light</span></h2>
      <p class="text-xs opacity-60">
        Pick a color style, then switch between dark and light mode (the ☀️/🌙
        button in the header does the same). Themes are plain CSS variables —
        switching costs zero extra RAM and your choice is remembered in this browser.
      </p>

      <div class="flex flex-wrap items-center gap-2">
        {#each THEME_STYLES as t (t.id)}
          <button
            class="btn btn-outline btn-sm gap-2 {$themeStyle === t.id ? 'btn-primary' : ''}"
            on:click={() => setThemeStyle(t.id)}
            title="{t.desc} — dark: {t.dark}, light: {t.light}"
          >
            <span class="flex items-center -space-x-1">
              <span class="h-4 w-4 rounded-full border border-base-300" style="background:{SWATCH[t.id].base}" />
              <span class="h-4 w-4 rounded-full border border-base-300" style="background:{SWATCH[t.id].primary}" />
              <span class="h-4 w-4 rounded-full border border-base-300" style="background:{SWATCH[t.id].accent}" />
            </span>
            {t.label}
            {#if $themeStyle === t.id}<span class="text-xs opacity-60">· active</span>{/if}
          </button>
        {/each}
      </div>

      <label class="flex w-fit cursor-pointer items-center gap-2 text-sm">
        <input
          type="checkbox"
          class="toggle toggle-primary"
          checked={$themeMode === 'light'}
          on:change={(e) => setThemeMode(e.currentTarget.checked ? 'light' : 'dark')}
        />
        Light mode {$themeMode === 'light' ? '(on — bright)' : '(off — dark)'}
      </label>
    </div>
  </section>

  <!-- Update channel -->
  <section class="card border border-base-300 bg-base-200 shadow-sm lg:col-span-2">
    <div class="card-body gap-3 p-5">
      <h2 class="card-title text-base">Update channel <span class="badge badge-sm badge-ghost">updates</span></h2>
      <p class="text-xs opacity-60">
        Updates are served exclusively by the project's own server —
        <code class="font-mono">https://istias.tech/hyprfetch/updates/</code> — as
        sha256-verified archives swapped in atomically. No GitHub account or
        token is needed, and GitHub is never contacted. Override the URL via
        <code class="font-mono">HYPRFETCH_UPDATE_CHANNEL</code>, the
        <code class="font-mono">[update] channel</code> config key, or
        <code class="font-mono">hyprfetch update --channel &lt;url&gt;</code>
        (set it to <code class="font-mono">""</code> to disable the updater).
      </p>
      <a class="btn btn-ghost btn-sm w-fit" href="https://istias.tech/hyprfetch/updates" target="_blank" rel="noreferrer">
        Update steps &amp; downloads ↗
      </a>
    </div>
  </section>
</div>
