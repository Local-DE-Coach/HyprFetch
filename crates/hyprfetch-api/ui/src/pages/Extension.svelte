<script>
  // Extension — the browser-extension control page (v0.6.1).
  //
  // 1. Connection card: is the extension heartbeating the daemon right now?
  // 2. Install cards: Firefox (.xpi) + Chromium (.zip) packages, downloaded
  //    from the self-hosted channel (istias.tech) — never from GitHub.
  // 3. Captured media: everything the extension spotted while you browsed,
  //    one click to push into the download queue.
  import { onMount, onDestroy } from 'svelte'
  import { extensionStatus, extensionMedia, extensionClearMedia, extensionDownload } from '../api.js'
  import { fmtBytes } from '../lib/format.js'
  import { notify, serverInfo } from '../lib/store.js'

  const CHANNEL = 'https://istias.tech/hyprfetch'

  let status = null
  let items = []
  let loading = true
  let busyUrl = ''
  let timer

  const extVersion = $serverInfo.version || '0.6.1'

  async function refresh() {
    try {
      const [s, m] = await Promise.all([extensionStatus(), extensionMedia()])
      status = s
      items = m.items ?? []
    } catch (_) {
      /* daemon restarting — keep the last view */
    } finally {
      loading = false
    }
  }

  onMount(() => {
    refresh()
    timer = setInterval(refresh, 5000)
  })
  onDestroy(() => clearInterval(timer))

  async function clearAll() {
    await extensionClearMedia()
    notify('captured media cleared')
    await refresh()
  }

  async function grab(item) {
    busyUrl = item.url
    try {
      await extensionDownload(item.url, item.filename)
      notify(`queued: ${item.filename || 'download'} ✓`)
      await refresh()
    } catch (e) {
      notify(e.message)
    } finally {
      busyUrl = ''
    }
  }

  function kindBadge(item) {
    const ct = (item.media_type || '').toLowerCase()
    if (ct.startsWith('video/')) return { label: 'video', cls: 'badge-primary' }
    if (ct.startsWith('audio/')) return { label: 'audio', cls: 'badge-success' }
    if (ct.startsWith('image/')) return { label: 'image', cls: 'badge-warning' }
    return { label: 'file', cls: 'badge-ghost' }
  }

  function when(ts) {
    if (!ts) return ''
    const s = Math.max(0, Math.round((Date.now() - ts) / 1000))
    if (s < 60) return 'just now'
    if (s < 3600) return `${Math.floor(s / 60)} min ago`
    if (s < 86400) return `${Math.floor(s / 3600)} h ago`
    return `${Math.floor(s / 86400)} d ago`
  }

  const chromeBrowsers = 'Chrome · Brave · Edge · Vivaldi · Opera'
</script>

<div class="mx-auto max-w-3xl">
  <div class="mb-4 flex items-center gap-3">
    <h2 class="text-xl font-semibold">Browser extension</h2>
    <span class="badge {$status?.connected ? 'badge-success' : 'badge-ghost'} uppercase">
      {$status?.connected ? 'connected' : 'not connected'}
    </span>
  </div>

  <!-- connection + how it works -->
  <div class="card mb-4 border border-base-300 bg-base-200 shadow-sm">
    <div class="card-body gap-3 p-4">
      <div class="flex flex-wrap items-center gap-2 text-sm">
        <span class="font-medium">Extension status:</span>
        {#if loading}
          <span class="opacity-60">checking…</span>
        {:else if $status?.connected}
          <span class="text-success">connected ✓</span>
          <span class="opacity-60">
            v{$status?.version ?? '?'} · last seen {when($status?.last_seen)} · {$status?.media_count} media seen
          </span>
        {:else}
          <span class="opacity-70">
            Install the extension below, then keep this page open — it shows “connected” as soon as the extension talks to the daemon.
          </span>
        {/if}
      </div>
      <p class="text-xs opacity-60">
        Like IDM on Windows: a small badge on the toolbar icon counts the media found on the current tab. Click the icon to see
        videos, audio and big files, then send any of them straight into HyprFetch. Everything stays on your machine —
        the extension talks only to <code class="font-mono">127.0.0.1</code>.
      </p>
    </div>
  </div>

  <!-- install cards -->
  <div class="mb-4 grid gap-3 sm:grid-cols-2">
    <div class="card border border-base-300 bg-base-200 shadow-sm">
      <div class="card-body gap-2 p-4">
        <h3 class="font-semibold">Firefox</h3>
        <p class="text-xs opacity-60">
          Download the <code class="font-mono">.xpi</code>, then: <span class="font-mono">about:addons</span> → gear icon →
          <em>Install Add-on From File…</em>
        </p>
        <a class="btn btn-primary btn-sm" href="{CHANNEL}/extension/hyprfetch-extension-{extVersion}-firefox.xpi" download>
          ↓ Firefox (.xpi)
        </a>
      </div>
    </div>
    <div class="card border border-base-300 bg-base-200 shadow-sm">
      <div class="card-body gap-2 p-4">
        <h3 class="font-semibold">Chromium</h3>
        <p class="text-xs opacity-60">
          {chromeBrowsers}. Unzip, then: <span class="font-mono">chrome://extensions</span> →
          enable <em>Developer mode</em> → <em>Load unpacked</em> → pick the folder.
        </p>
        <a class="btn btn-primary btn-sm" href="{CHANNEL}/extension/hyprfetch-extension-{extVersion}-chrome.zip" download>
          ↓ Chromium (.zip)
        </a>
      </div>
    </div>
  </div>

  <!-- captured media -->
  <div class="card border border-base-300 bg-base-200 shadow-sm">
    <div class="card-body gap-3 p-4">
      <div class="flex items-center gap-2">
        <h3 class="font-semibold">Media seen via the extension</h3>
        <span class="badge badge-sm badge-ghost">{items.length}</span>
        <span class="grow" />
        {#if items.length > 0}
          <button class="btn btn-ghost btn-xs" on:click={clearAll}>Clear</button>
        {/if}
      </div>

      {#if items.length === 0}
        <p class="text-sm opacity-60">
          {loading ? 'loading…' : 'Nothing yet — browse a page with media while the extension is installed and it shows up here.'}
        </p>
      {:else}
        <div class="grid gap-2">
          {#each items as item (item.url)}
            <div class="rounded-box border border-base-300 bg-base-100 p-3 text-sm">
              <div class="flex min-w-0 flex-wrap items-center gap-2">
                <span class="min-w-0 max-w-[46%] truncate font-medium" title={item.filename ?? item.url}>
                  {item.filename || item.url.split('/').pop() || 'media'}
                </span>
                <span class="badge badge-sm {kindBadge(item).cls}">{kindBadge(item).label}</span>
                {#if item.size}
                  <span class="font-mono text-xs opacity-70">{fmtBytes(item.size)}</span>
                {/if}
                <span class="text-xs opacity-50">{when(item.ts)}</span>
                <span class="grow" />
                <button class="btn btn-primary btn-xs" disabled={busyUrl === item.url} on:click={() => grab(item)}>
                  {busyUrl === item.url ? '…' : 'Download'}
                </button>
              </div>
              <div class="truncate font-mono text-xs opacity-50" title={item.url}>{item.url}</div>
              {#if item.page_url}
                <div class="truncate text-xs opacity-40" title={item.page_url}>from {item.page_url}</div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</div>
