<script>
  // Settings — save folders (base + per-category overrides), auto-sort,
  // concurrency, QoS bandwidth cap, security, appearance (5 theme styles ×
  // dark/light) and the update channel that powers the in-app updater.
  import { onMount } from 'svelte'
  import { fmtBytes } from '../lib/format.js'
  import {
    settings, categories, saveSettings, getQos, setQos, notify,
    resourceUsage, wakeUp, enterBackgroundMode,
  } from '../lib/store.js'
  import {
    THEME_STYLES, themeStyle, themeMode, setThemeStyle, setThemeMode,
  } from '../lib/theme.js'
  import { getServerInfo } from '../api.js'

  import {
    getWidgetStatus, installWidget, uninstallWidget,
    getYtdlpStatus, installYtdlp,
  } from '../api.js'

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

  // ---- app & background (v0.4.6) ----
  // show_resource_usage — footer RAM/CPU widget (this app only)
  // keep_alive_in_background — show the ⏾ close-to-background controls
  let showUsage = true
  let keepAlive = true
  let savingApp = false
  let quietState = false

  $: if ($settings?.show_resource_usage !== undefined && showUsage) {
    showUsage = $settings.show_resource_usage !== 'false'
  }
  $: if ($settings?.keep_alive_in_background !== undefined && keepAlive) {
    keepAlive = $settings.keep_alive_in_background !== 'false'
  }

  onMount(async () => {
    try { quietState = (await getServerInfo()).quiet ?? false } catch (_) { /* ignore */ }
    try { widget = await getWidgetStatus() } catch (_) { /* widget card stays hidden */ }
    try { ytdlp = await getYtdlpStatus() } catch (_) { /* media card shows not installed */ }
  })

  // ---- media engine (yt-dlp, v0.6.1) ----
  // Powers "download any media from any URL" + YouTube quality picking.
  // Auto-installs on first media download; this card shows the state and
  // lets the user update it on demand (YouTube breaks extractors often).
  let ytdlp = null
  let ytdlpBusy = false

  async function doYtdlpInstall() {
    ytdlpBusy = true
    try {
      ytdlp = await installYtdlp()
      notify(`yt-dlp ${ytdlp.version ?? ''} ready — media downloads unlocked ✓`)
    } catch (e) {
      notify(`yt-dlp install failed: ${e.message}`)
    } finally {
      ytdlpBusy = false
    }
  }

  // Cookies-from-browser: fixes YouTube's "confirm you're not a bot" wall
  // for signed-in browsers (same trick IDM uses with browser sessions).
  let cookiesBrowser = ''
  $: if ($settings?.ytdlp_cookies_browser !== undefined) {
    cookiesBrowser = $settings.ytdlp_cookies_browser ?? ''
  }
  let savingCookies = false
  async function saveCookies() {
    savingCookies = true
    try {
      await saveSettings({ ytdlp_cookies_browser: cookiesBrowser.trim() })
      notify('media engine cookies saved ✓')
    } catch (e) {
      notify(`save failed: ${e.message}`)
    } finally {
      savingCookies = false
    }
  }

  // ---- desktop widget (Quickshell sidebar tab, v0.5.1) ----
  // One card to set up the illogical-impulse sidebar widget: install the
  // QML files into
  // ~/.config/quickshell/ii/modules/ii/sidebarLeft/downloadManager, wire
  // the "Downloads" tab into SidebarLeftContent.qml and set the ii policy
  // — all under $HOME, no terminal. Also cleans up the old v0.5.0 bar
  // widget automatically.
  let widget = null
  let widgetBusy = false

  async function doWidgetInstall() {
    widgetBusy = true
    try {
      widget = await installWidget()
      notify(widget.integrated
        ? 'desktop widget installed — the Downloads tab is in your sidebar ✓ — reload the shell to see it'
        : 'desktop widget installed — check the note in the card')
    } catch (e) {
      notify(`widget install failed: ${e.message}`)
    } finally {
      widgetBusy = false
    }
  }

  async function doWidgetUninstall() {
    widgetBusy = true
    try {
      widget = await uninstallWidget()
      notify('desktop widget removed — reload your shell')
    } catch (e) {
      notify(`widget uninstall failed: ${e.message}`)
    } finally {
      widgetBusy = false
    }
  }

  async function saveApp() {
    savingApp = true
    try {
      await saveSettings({
        show_resource_usage: showUsage ? 'true' : 'false',
        keep_alive_in_background: keepAlive ? 'true' : 'false',
      })
      notify('app settings saved ✓')
    } catch (e) {
      notify(`save failed: ${e.message}`)
    } finally {
      savingApp = false
    }
  }
  // ---- appearance (themes) ----
  // 5 styles × dark/light, all static CSS — switching costs nothing.

  // Representative daisyUI colors per style for the picker swatches
  // (matches the custom palettes in tailwind.config.js, v0.5.0).
  const SWATCH = {
    slate: { primary: '#8d7bfa', base: '#191831', accent: '#35d0e8' },
    ocean: { primary: '#4cc3fa', base: '#0c1b2c', accent: '#2dd4bf' },
    forest: { primary: '#52d983', base: '#0f1f16', accent: '#a3e635' },
    coffee: { primary: '#ffb224', base: '#211710', accent: '#c084fc' },
    cyber: { primary: '#e879f9', base: '#1b1038', accent: '#22d3ee' },
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

  <!-- App & background (v0.4.6) -->
  <section class="card border border-base-300 bg-base-200 shadow-sm">
    <div class="card-body gap-3 p-5">
      <h2 class="card-title text-base">App &amp; background</h2>

      <label class="flex w-fit cursor-pointer items-center gap-2 text-sm">
        <input type="checkbox" class="toggle toggle-primary" bind:checked={showUsage} />
        Show resource usage <span class="opacity-50">(RAM &amp; CPU of this app only, in the footer)</span>
      </label>
      {#if showUsage && $resourceUsage}
        <div class="flex flex-wrap gap-2 font-mono text-xs opacity-80">
          <span class="rounded-box border border-base-300 px-2 py-1">RAM {fmtBytes($resourceUsage.rss_bytes)}</span>
          <span class="rounded-box border border-base-300 px-2 py-1" title="Peak resident memory">peak {fmtBytes($resourceUsage.peak_rss_bytes)}</span>
          <span class="rounded-box border border-base-300 px-2 py-1" title="Across all cores (100% = every core busy)">CPU {$resourceUsage.cpu_percent.toFixed(1)}%</span>
          <span class="rounded-box border border-base-300 px-2 py-1">{$resourceUsage.threads} threads</span>
        </div>
      {/if}

      <label class="flex w-fit cursor-pointer items-center gap-2 text-sm">
        <input type="checkbox" class="toggle toggle-primary" bind:checked={keepAlive} />
        Keep the app alive in the background <span class="opacity-50">(⏾ close button stays available)</span>
      </label>
      <p class="text-xs opacity-60">
        Closing the UI does not stop HyprFetch: it stays resident at a few MiB,
        downloads keep running, and it wakes back up instantly — run
        <code class="font-mono">hyprfetch open</code> (starts it if needed and
        opens your browser) or just visit <code class="font-mono">http://127.0.0.1:7780</code>.
      </p>

      {#if quietState}
        <div class="alert alert-success flex items-center gap-2 py-2 text-sm">
          <span class="grow">⏾ Background mode is on — the app minimizes its own activity (downloads keep running).</span>
          <button class="btn btn-outline btn-xs" on:click={async () => { await wakeUp(); quietState = false }}>Wake up</button>
        </div>
      {:else}
        <div class="card-actions justify-end">
          <button
            class="btn btn-outline btn-sm"
            title="Minimize the app's own activity now — downloads continue, reopen with: hyprfetch open"
            on:click={async () => { await enterBackgroundMode(); quietState = true }}
          >⏾ Enter background mode now</button>
        </div>
      {/if}

      <div class="card-actions justify-end">
        <button class="btn btn-primary btn-sm" disabled={savingApp} on:click={saveApp}>
          {savingApp ? 'Saving…' : 'Save app settings'}
        </button>
      </div>
    </div>
  </section>

  <!-- Desktop widget (v0.5.1): set up the Quickshell sidebar tab in one click -->
  {#if widget}
    <section class="card border border-base-300 bg-base-200 shadow-sm lg:col-span-2">
      <div class="card-body gap-3 p-5">
        <h2 class="card-title text-base">
          Desktop widget
          <span class="badge badge-sm badge-ghost">Hyprland sidebar tab · light RAM</span>
        </h2>
        <p class="text-xs opacity-60">
          A full <a class="link" href="https://ii.clsty.link/en/dev/project-contrib/" target="_blank" rel="noreferrer">illogical-impulse</a> sidebar tab:
          paste a URL, confirm the save path, watch live progress with speed
          and ETA, open or remove finished downloads. Reads a tiny status file
          once a second — no extra processes.
        </p>

        <div class="flex flex-wrap items-center gap-2 text-xs">
          <span class="badge {widget.qs_found ? 'badge-success' : 'badge-error'} badge-outline">
            ii config {widget.qs_found ? 'found' : 'missing'}
          </span>
          <span class="badge {widget.quickshell_found ? 'badge-success' : 'badge-warning'} badge-outline">
            quickshell {widget.quickshell_found ? 'found' : 'not on PATH'}
          </span>
          <span class="badge {widget.hyprfetch_found ? 'badge-success' : 'badge-warning'} badge-outline">
            hyprfetch CLI {widget.hyprfetch_found ? 'found' : 'not on PATH'}
          </span>
          {#if widget.legacy_bar_widget_found}
            <span class="badge badge-warning badge-outline">old bar widget found — reinstall cleans it</span>
          {/if}
          {#if widget.installed}
            <span class="badge {widget.integrated ? 'badge-success' : 'badge-warning'} badge-outline">
              {widget.integrated ? 'Downloads tab wired ✓' : 'files only — not wired'}
            </span>
            <span class="badge badge-ghost badge-outline">v{widget.version ?? '?'}</span>
          {/if}
        </div>

        {#if widget.note}
          <div class="alert alert-warning py-2 px-3 text-xs" role="alert">
            <span>{widget.note}</span>
          </div>
        {/if}

        <div class="flex flex-wrap items-center gap-2">
          {#if widget.installed && widget.up_to_date && widget.integrated}
            <span class="text-xs opacity-60">installed at <code class="font-mono">{widget.widget_dir}</code></span>
          {:else}
            <button class="btn btn-primary btn-sm" disabled={widgetBusy} on:click={doWidgetInstall}>
              {widgetBusy ? 'Installing…'
                : widget.installed ? (widget.up_to_date ? 'Reinstall' : 'Update widget') : 'Install widget'}
            </button>
          {/if}
          {#if widget.installed}
            <button class="btn btn-ghost btn-sm" disabled={widgetBusy} on:click={doWidgetUninstall}>Remove</button>
          {/if}
        </div>

        {#if widget.installed}
          <p class="text-xs opacity-60">
            Apply now by reloading the shell:
            <code class="font-mono select-all">{widget.reload_hint}</code>
          </p>
        {/if}
      </div>
    </section>
  {/if}

  <!-- Appearance: 5 theme styles × dark/light, synced on the server -->
  <section class="card border border-base-300 bg-base-200 shadow-sm lg:col-span-2">
    <div class="card-body gap-3 p-5">
      <h2 class="card-title text-base">Appearance <span class="badge badge-sm badge-ghost">5 styles · dark &amp; light · synced</span></h2>
      <p class="text-xs opacity-60">
        Pick a color style, then switch between dark and light mode (the ☀️/🌙
        button in the header does the same). Your choice is stored on the
        SERVER — every browser and device that opens this UI gets the same
        theme automatically (v0.4.6). Themes are plain CSS variables, so
        switching costs zero extra RAM.
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

  <!-- Media engine (yt-dlp) -->
  <section class="card border border-base-300 bg-base-200 shadow-sm lg:col-span-2">
    <div class="card-body gap-3 p-5">
      <h2 class="card-title text-base">
        Media engine
        <span class="badge badge-sm {ytdlp?.installed ? 'badge-success' : 'badge-ghost'}">
          {ytdlp?.installed ? `yt-dlp ${ytdlp.version ?? ''}` : 'not installed'}
        </span>
      </h2>
      <p class="text-xs opacity-60">
        Downloads <strong>any media from any link</strong> — videos, songs, images behind weird URLs — plus YouTube &amp;
        1000+ sites in every quality (like IDM/FDM). The engine (<code class="font-mono">yt-dlp</code>) auto-installs
        from your own update channel the first time you grab a video; no manual setup. Updating it now and then keeps
        YouTube working.
      </p>
      <div class="flex flex-wrap items-center gap-2">
        {#if ytdlp?.installed}
          <span class="font-mono text-xs opacity-50">{ytdlp.path}</span>
          <span class="badge badge-sm {ytdlp?.ffmpeg ? 'badge-success' : 'badge-warning'}">
            {ytdlp?.ffmpeg ? 'ffmpeg ✓ (all qualities)' : 'ffmpeg missing (basic qualities)'}
          </span>
          <span class="badge badge-sm {ytdlp?.deno ? 'badge-success' : 'badge-warning'}">
            {ytdlp?.deno ? 'JS runtime ✓ (full speed)' : 'JS runtime missing (slow on YouTube)'}
          </span>
        {/if}
        <span class="grow" />
        <button class="btn btn-primary btn-sm" disabled={ytdlpBusy} on:click={doYtdlpInstall}>
          {ytdlpBusy ? 'Working…' : ytdlp?.installed ? 'Update engine' : 'Install engine'}
        </button>
      </div>
      <div class="flex flex-wrap items-center gap-2 border-t border-base-300 pt-3">
        <span class="text-xs opacity-70">
          YouTube says “confirm you're not a bot”? Use your browser's cookies
          (works when you're signed in to YouTube):
        </span>
        <select class="select select-bordered select-sm" bind:value={cookiesBrowser}>
          <option value="">No cookies</option>
          <option value="firefox">Firefox</option>
          <option value="chromium">Chromium</option>
          <option value="chrome">Chrome</option>
          <option value="brave">Brave</option>
          <option value="edge">Edge</option>
          <option value="vivaldi">Vivaldi</option>
          <option value="opera">Opera</option>
        </select>
        <button class="btn btn-sm" disabled={savingCookies} on:click={saveCookies}>Save</button>
      </div>
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
