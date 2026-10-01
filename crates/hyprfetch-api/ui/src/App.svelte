<script>
  // HyprFetch shell: navbar with hash routing, global toast, IDM-style
  // two-step "Add download" confirm popup and the floating progress panel.
  // Pages: Dashboard / Tasks / Settings / Updates (see lib/router.js).
  import { onMount, onDestroy } from 'svelte'
  import { page, PAGES, nav } from './lib/router.js'
  import { fmtBytes, fmtSpeed, fmtUptime } from './lib/format.js'
  import { themeMode, setThemeMode } from './lib/theme.js'
  import {
    active, finished, globalSpeed, activeCount, serverInfo, wsConnected,
    toast, showAdd, initApp, addDownload, refreshServer, categories,
    floatPanel, setFloatPanel, notify,
    settings, resourceUsage, refreshUsage, enterBackgroundMode,
  } from './lib/store.js'
  import { inspectUrl, probeMedia, mediaDownload } from './api.js'
  import Dashboard from './pages/Dashboard.svelte'
  import Tasks from './pages/Tasks.svelte'
  import Settings from './pages/Settings.svelte'
  import Updates from './pages/Updates.svelte'
  import Extension from './pages/Extension.svelte'
  import FloatBar from './lib/FloatBar.svelte'

  let ws
  onMount(() => {
    initApp()
    const t = setInterval(refreshServer, 10_000)
    return () => clearInterval(t)
  })
  onDestroy(() => { try { ws?.close() } catch (_) {} })

  // Resource-usage widget: poll only while it's switched on (Settings → App).
  let usageTimer
  $: showUsage = $settings.show_resource_usage !== 'false'
  $: {
    clearInterval(usageTimer)
    if (showUsage) {
      refreshUsage()
      usageTimer = setInterval(refreshUsage, 3000)
    } else {
      resourceUsage.set(null)
    }
  }
  onDestroy(() => clearInterval(usageTimer))

  // ---- Add modal state (two steps, IDM-style) ----
  // step 1: source (urls + destination + options)
  // step 2: confirm — quality picker for stream pages (YouTube & friends),
  //          file-info cards for direct files
  let step = 1
  let urlText = ''
  let category = 'auto'
  let saveDir = ''
  let filename = ''
  let segments = 8
  let addError = ''
  let adding = false
  let inspecting = false
  let confirmed = []   // inspect results for the confirm step
  // Media-picker state (single stream URL → quality list).
  let mediaProbe = null       // { media: {...} } from /api/media/probe
  let chosenQuality = 'best'
  let audioOnly = false

  function resetAdd() {
    step = 1
    urlText = ''
    saveDir = ''
    filename = ''
    category = 'auto'
    addError = ''
    adding = false
    inspecting = false
    confirmed = []
    mediaProbe = null
    chosenQuality = 'best'
    audioOnly = false
  }

  function urlsList() {
    return urlText.split('\n').map((s) => s.trim()).filter(Boolean)
  }

  async function goConfirm() {
    addError = ''
    const urls = urlsList()
    if (urls.length === 0) {
      addError = 'enter at least one URL'
      return
    }
    inspecting = true
    try {
      // Unified probe (v0.6.1): one call answers "direct file" (native
      // engine — show the IDM-style confirm card) or "media page"
      // (yt-dlp ladder — show the quality picker).
      const probe = await probeMedia(urls[0]).catch((e) => ({ probe_error: e.message }))
      if (probe?.kind === 'media' && urls.length === 1) {
        mediaProbe = probe
        const quals = probe.media.qualities ?? []
        chosenQuality = quals.find((q) => !q.audio_only)?.id ?? 'best'
        audioOnly = false
        step = 2
        return
      }

      // Direct files (or multi-URL batches): the classic flow.
      const results = await Promise.all(urls.slice(0, 20).map(async (url) => {
        try {
          const p = await probeMedia(url)
          if (p.kind === 'file' && p.file) return p.file
          // A stream page inside a batch — fall back to its basic info.
          return { url, final_url: url, filename: p.media?.suggested_filename ?? url, total_bytes: null, accept_ranges: false, category: 'video', save_dir: '', save_path: '', content_type: 'video/stream' }
        } catch (e) {
          // Legacy fallback: plain HTTP inspect (daemon older than 0.6 or probe hiccup).
          try {
            return await inspectUrl({ url, category: category === 'auto' ? undefined : category, saveDir: saveDir.trim() || undefined, filename: filename.trim() || undefined })
          } catch (_) {
            return { url, probe_error: e.message }
          }
        }
      }))
      confirmed = results
      step = 2
    } finally {
      inspecting = false
    }
  }

  async function submitMedia() {
    addError = ''
    adding = true
    try {
      const q = (mediaProbe?.media?.qualities ?? []).find((x) => x.id === chosenQuality)
      await mediaDownload({
        url: urlsList()[0],
        quality: audioOnly ? undefined : chosenQuality,
        audioOnly,
        filename: filename.trim() || undefined,
        saveDir: saveDir.trim() || undefined,
        category: category === 'auto' ? undefined : category,
      })
      showAdd.set(false)
      resetAdd()
      notify(`download started ✓ ${q ? '· ' + q.label : ''}`)
    } catch (e) {
      addError = e.message
    } finally {
      adding = false
    }
  }

  async function submitAdd() {
    addError = ''
    adding = true
    try {
      await addDownload({
        urls: urlsList(),
        category: category === 'auto' ? undefined : category,
        saveDir: saveDir.trim() || undefined,
        filename: filename.trim() || undefined,
        segments,
      })
      showAdd.set(false)
      resetAdd()
      notify('download started ✓')
    } catch (e) {
      addError = e.message
    } finally {
      adding = false
    }
  }

  const catIcons = { video: '🎬', pictures: '🖼', music: '🎵', compress: '📦', documents: '📄', apps: '💽', other: '📁' }
</script>

<header class="navbar sticky top-0 z-20 h-14 min-h-0 border-b border-base-300 bg-base-100/90 px-2 backdrop-blur">
  <div class="flex min-w-0 items-center gap-1.5">
    <span class="text-xl text-primary">⇣</span>
    <h1 class="hidden min-[420px]:inline text-base font-semibold tracking-wide">HyprFetch</h1>
    <span
      class="badge badge-sm {wsConnected ? 'badge-success' : 'badge-ghost'} uppercase"
      title="websocket status"
    >{wsConnected ? 'live' : 'off'}</span>
  </div>

  <!-- page nav: icons only on narrow screens -->
  <nav class="flex items-center gap-0.5 overflow-x-auto px-1 sm:gap-1 sm:px-2">
    {#each PAGES as p (p.id)}
      <button
        class="btn btn-ghost btn-sm px-2 {$page === p.id ? 'btn-active' : ''}"
        title={p.label}
        on:click={() => nav(p.id)}
      >{p.icon} <span class="hidden md:inline">{p.label}</span></button>
    {/each}
  </nav>

  <div class="flex items-center gap-1.5 sm:gap-3">
    <span class="hidden font-mono text-sm text-secondary md:inline" title="aggregate download speed">{fmtSpeed($globalSpeed)}</span>
    <span class="hidden text-xs opacity-60 sm:inline">{$activeCount} active</span>

    <!-- floating progress panel quick toggle -->
    {#if $active.length > 0}
      <button
        class="btn btn-ghost btn-sm px-2"
        title={$floatPanel === 'hide' ? 'Show download progress panel' : 'Hide download progress panel'}
        on:click={() => setFloatPanel($floatPanel === 'hide' ? 'show' : 'hide')}
      >⇣<span class="badge badge-sm badge-primary">{$active.length}</span></button>
    {/if}

    <!-- dark / light quick toggle -->
    <button
      class="btn btn-ghost btn-sm px-2"
      title={$themeMode === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
      on:click={() => setThemeMode($themeMode === 'dark' ? 'light' : 'dark')}
    >{$themeMode === 'dark' ? '☀️' : '🌙'}</button>

    <!-- close-to-background: app stays alive at minimal usage -->
    {#if $settings.keep_alive_in_background !== 'false'}
      <button
        class="btn btn-ghost btn-sm px-2"
        title="Close to background — HyprFetch keeps running (downloads continue), reopen with: hyprfetch open"
        on:click={enterBackgroundMode}
      >⏾</button>
    {/if}

    <button class="btn btn-primary btn-sm" on:click={() => { resetAdd(); showAdd.set(true) }}>+ Add</button>
  </div>
</header>

{#if $toast}
  <div class="alert alert-success mx-auto mt-3 max-w-3xl cursor-pointer text-sm shadow" role="alert" on:click={() => toast.set('')}>
    <span>{$toast} ✕</span>
  </div>
{/if}

<main class="mx-auto w-full max-w-6xl px-3 pb-16 pt-4 sm:px-5">
  {#if $page === 'dashboard'}
    <Dashboard />
  {:else if $page === 'tasks'}
    <Tasks />
  {:else if $page === 'extension'}
    <Extension />
  {:else if $page === 'settings'}
    <Settings />
  {:else if $page === 'updates'}
    <Updates />
  {/if}
</main>

<footer class="border-t border-base-300 py-4 text-center text-xs opacity-50">
  <span>HyprFetch v{$serverInfo.version || '…'} — minimal-RAM download manager ·
  uptime {fmtUptime($serverInfo.uptime_secs)}</span>
  {#if showUsage && $resourceUsage}
    <span class="mx-1">·</span>
    <span
      class="font-mono"
      title="What this app uses right now — RAM resident set and CPU across all cores"
    >RAM {fmtBytes($resourceUsage.rss_bytes)} · CPU {$resourceUsage.cpu_percent.toFixed(1)}%</span>
  {/if}
  {#if $serverInfo.quiet}
    <span class="mx-1">·</span>
    <span class="text-success" title="Background mode: the app minimizes its own activity. Downloads keep running. Wake from Settings → App.">⏾ background</span>
  {/if}
</footer>

<!-- floating per-download progress (IDM-style transfer monitor) -->
<FloatBar />

{#if $showAdd}
  <div class="modal modal-open" on:click|self={() => showAdd.set(false)}>
    <div class="modal-box max-h-[92vh] max-w-lg overflow-y-auto">
      {#if step === 1}
        <form on:submit|preventDefault={goConfirm}>
          <h3 class="mb-3 text-lg font-semibold">Add download</h3>
          <div class="grid gap-3">
            <label class="grid gap-1.5 text-sm">
              <span>URLs <span class="opacity-50">(one per line)</span></span>
              <textarea rows="4" class="textarea textarea-bordered" bind:value={urlText} placeholder="https://example.com/movie.mkv" />
            </label>
            <label class="grid gap-1.5 text-sm">
              <span>Save into</span>
              <select class="select select-bordered" bind:value={category}>
                <option value="auto">Auto-sort by file type (recommended)</option>
                <option value="none">Base folder only (no subfolder)</option>
                {#each $categories.categories as c (c.name)}
                  <option value={c.name}>{catIcons[c.name]} {c.name} — {c.dir}</option>
                {/each}
              </select>
            </label>
            <label class="grid gap-1.5 text-sm">
              <span>Direct save folder <span class="opacity-50">(optional — overrides the category above)</span></span>
              <input type="text" class="input input-bordered font-mono" bind:value={saveDir} placeholder="~/Downloads" />
            </label>
            <label class="grid gap-1.5 text-sm">
              <span>Filename <span class="opacity-50">(optional)</span></span>
              <input type="text" class="input input-bordered" bind:value={filename} placeholder="from the URL" />
            </label>
            <label class="grid gap-1.5 text-sm">
              <span>Segments</span>
              <input type="number" min="1" max="32" class="input input-bordered w-28" bind:value={segments} />
            </label>
            {#if addError}<p class="text-sm text-error">{addError}</p>{/if}
          </div>
          <div class="modal-action">
            <button type="button" class="btn btn-sm" on:click={() => showAdd.set(false)}>Cancel</button>
            <button type="submit" class="btn btn-primary btn-sm" disabled={inspecting}>
              {inspecting ? 'Checking…' : 'Next — confirm ▸'}
            </button>
          </div>
        </form>
      {:else if mediaProbe}
        <!-- STEP 2 (media): quality picker for stream pages -->
        <div>
          <h3 class="mb-1 text-lg font-semibold">Choose quality</h3>
          <p class="mb-3 truncate text-xs opacity-60" title={mediaProbe.media.title ?? ''}>
            {mediaProbe.media.title} · {mediaProbe.media.extractor}
            {#if mediaProbe.media.duration != null}
              · {Math.round(mediaProbe.media.duration / 60)} min
            {/if}
          </p>
          <div class="grid gap-1.5">
            {#each mediaProbe.media.qualities as q (q.id)}
              <label
                class="flex cursor-pointer items-center gap-3 rounded-box border px-3 py-2 text-sm
                  {q.audio_only ? (audioOnly ? 'border-primary bg-primary/10' : 'border-base-300') : (!audioOnly && chosenQuality === q.id ? 'border-primary bg-primary/10' : 'border-base-300')}"
              >
                <input
                  type="radio"
                  class="radio radio-primary radio-sm"
                  name="quality"
                  checked={audioOnly ? q.audio_only : (!q.audio_only && chosenQuality === q.id)}
                  on:change={() => { if (q.audio_only) { audioOnly = true } else { audioOnly = false; chosenQuality = q.id } }}
                />
                <span class="font-medium">{q.label}</span>
                {#if q.note}<span class="badge badge-sm badge-ghost">{q.note}</span>{/if}
                <span class="badge badge-sm badge-ghost uppercase">{q.container}</span>
                <span class="grow" />
                <span class="font-mono text-xs opacity-60">
                  {q.size_bytes != null ? fmtBytes(q.size_bytes) : ''}
                </span>
              </label>
            {/each}
          </div>
          {#if mediaProbe.media.ffmpeg === false}
            <p class="mt-2 text-xs text-warning">
              ffmpeg not found — DASH merges and MP3 extraction are hidden. Install <code class="font-mono">ffmpeg</code> (it's one package) and every quality appears.
            </p>
          {/if}
          <p class="mt-2 text-xs opacity-50">
            One entry per resolution — duplicate formats (webm/mkv of the same quality) are filtered out, MP4 preferred.
          </p>
          <label class="mt-3 grid gap-1.5 text-sm">
            <span>Filename <span class="opacity-50">(optional)</span></span>
            <input type="text" class="input input-bordered" bind:value={filename} placeholder={mediaProbe.media.suggested_filename} />
          </label>
          {#if addError}<p class="mt-2 text-sm text-error">{addError}</p>{/if}
          <div class="modal-action">
            <button type="button" class="btn btn-sm" on:click={() => (step = 1)} disabled={adding}>◂ Back</button>
            <button type="button" class="btn btn-primary btn-sm" disabled={adding} on:click={submitMedia}>
              {adding ? 'Starting…' : 'Start download ▶'}
            </button>
          </div>
        </div>
      {:else}
        <!-- STEP 2: confirm before anything starts (like IDM's file-info dialog) -->
        <div>
          <h3 class="mb-1 text-lg font-semibold">Confirm download</h3>
          <p class="mb-3 text-xs opacity-60">
            {confirmed.length} file{confirmed.length === 1 ? '' : 's'} · check the name, size and exact save path, then start.
          </p>
          <div class="grid gap-2">
            {#each confirmed as c, i (i)}
              <div class="rounded-box border border-base-300 bg-base-200/60 p-3 text-sm">
                {#if c.probe_error}
                  <div class="mb-1 truncate font-medium" title={c.url}>{c.url}</div>
                  <div class="text-xs text-warning">couldn't probe: {c.probe_error} — it will still download normally</div>
                {:else}
                  <div class="mb-1 flex items-center gap-2">
                    <span class="text-base">{catIcons[c.category] ?? '📁'}</span>
                    <span class="min-w-0 flex-1 truncate font-medium" title={c.filename}>{c.filename}</span>
                    <span class="badge badge-sm badge-ghost">{c.category}</span>
                  </div>
                  <div class="grid gap-0.5 font-mono text-xs opacity-70">
                    <div class="truncate" title={c.url}>from {c.final_url}</div>
                    <div class="truncate" title={c.save_path}>to&nbsp;&nbsp;{c.save_path}</div>
                    <div>
                      size {c.total_bytes != null ? fmtBytes(c.total_bytes) : 'unknown'}
                      · {c.accept_ranges ? 'multi-segment ✓' : 'single stream'}
                    </div>
                    {#if c.content_type}
                      <div class="text-success" title="The extension is taken from the server's Content-Type when the URL has none">
                        type {c.content_type} · extension detected ✓
                      </div>
                    {/if}
                  </div>
                {/if}
              </div>
            {/each}
            {#if addError}<p class="text-sm text-error">{addError}</p>{/if}
          </div>
          <div class="modal-action">
            <button type="button" class="btn btn-sm" on:click={() => (step = 1)} disabled={adding}>◂ Back</button>
            <button type="button" class="btn btn-primary btn-sm" disabled={adding} on:click={submitAdd}>
              {adding ? 'Starting…' : 'Start download ▶'}
            </button>
          </div>
        </div>
      {/if}
    </div>
  </div>
{/if}
