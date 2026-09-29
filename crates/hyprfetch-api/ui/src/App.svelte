<script>
  // HyprFetch shell: navbar with hash routing, global toast + Add modal.
  // Pages: Dashboard / Tasks / Settings / Updates (see lib/router.js).
  import { onMount, onDestroy } from 'svelte'
  import { page, PAGES, nav } from './lib/router.js'
  import { fmtBytes, fmtSpeed, fmtUptime } from './lib/format.js'
  import {
    active, finished, globalSpeed, activeCount, serverInfo, wsConnected,
    toast, showAdd, initApp, addDownload, refreshServer, categories,
  } from './lib/store.js'
  import Dashboard from './pages/Dashboard.svelte'
  import Tasks from './pages/Tasks.svelte'
  import Settings from './pages/Settings.svelte'
  import Updates from './pages/Updates.svelte'

  let ws
  onMount(() => {
    initApp()
    const t = setInterval(refreshServer, 10_000)
    return () => clearInterval(t)
  })
  onDestroy(() => { try { ws?.close() } catch (_) {} })

  // ---- Add modal state ----
  let urlText = ''
  let category = 'auto'
  let saveDir = ''
  let filename = ''
  let segments = 8
  let addError = ''
  let adding = false

  async function submitAdd() {
    addError = ''
    const urls = urlText.split('\n').map((s) => s.trim()).filter(Boolean)
    if (urls.length === 0) {
      addError = 'enter at least one URL'
      return
    }
    adding = true
    try {
      await addDownload({
        urls,
        category: category || undefined,
        saveDir: saveDir.trim() || undefined,
        filename: filename.trim() || undefined,
        segments,
      })
      showAdd.set(false)
      urlText = ''
      saveDir = ''
      filename = ''
      category = 'auto'
    } catch (e) {
      addError = e.message
    } finally {
      adding = false
    }
  }

  const catIcons = { video: '🎬', pictures: '🖼', music: '🎵', compress: '📦', documents: '📄', apps: '💽', other: '📁' }
</script>

<header class="navbar sticky top-0 z-20 h-14 min-h-0 border-b border-base-300 bg-base-100/90 backdrop-blur">
  <div class="flex items-center gap-2 px-2">
    <span class="text-xl text-primary">⇣</span>
    <h1 class="text-base font-semibold tracking-wide">HyprFetch</h1>
    <span
      class="badge badge-sm {wsConnected ? 'badge-success' : 'badge-ghost'} uppercase"
      title="websocket status"
    >{wsConnected ? 'live' : 'offline'}</span>
  </div>

  <!-- page nav -->
  <nav class="flex items-center gap-1 px-2">
    {#each PAGES as p (p.id)}
      <button
        class="btn btn-ghost btn-sm {$page === p.id ? 'btn-active' : ''}"
        on:click={() => nav(p.id)}
      >{p.icon} {p.label}</button>
    {/each}
  </nav>

  <div class="flex items-center gap-3 px-2">
    <span class="font-mono text-sm text-secondary" title="aggregate download speed">{fmtSpeed($globalSpeed)}</span>
    <span class="text-xs opacity-60">{$activeCount} active</span>
    <button class="btn btn-primary btn-sm" on:click={() => showAdd.set(true)}>+ Add</button>
  </div>
</header>

{#if $toast}
  <div class="alert alert-success mx-auto mt-3 max-w-3xl cursor-pointer text-sm shadow" role="alert" on:click={() => toast.set('')}>
    <span>{$toast} ✕</span>
  </div>
{/if}

<main class="mx-auto max-w-5xl px-5 pb-16 pt-4">
  {#if $page === 'dashboard'}
    <Dashboard />
  {:else if $page === 'tasks'}
    <Tasks />
  {:else if $page === 'settings'}
    <Settings />
  {:else if $page === 'updates'}
    <Updates />
  {/if}
</main>

<footer class="border-t border-base-300 py-4 text-center text-xs opacity-50">
  HyprFetch v{$serverInfo.version || '…'} — minimal-RAM download manager ·
  uptime {fmtUptime($serverInfo.uptime_secs)}
</footer>

{#if $showAdd}
  <div class="modal modal-open" on:click|self={() => showAdd.set(false)}>
    <form class="modal-box max-w-md" on:submit|preventDefault={submitAdd}>
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
        <button type="submit" class="btn btn-primary btn-sm" disabled={adding}>
          {adding ? 'Adding…' : 'Download'}
        </button>
      </div>
    </form>
  </div>
{/if}
