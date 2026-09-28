<script>
  import { onMount, onDestroy } from 'svelte'
  import {
    listTasks,
    createTasks,
    pauseTask,
    resumeTask,
    cancelTask,
    retryTask,
    deleteTask,
    deleteTaskWithFile,
    getQos,
    setQos,
    getServerInfo,
    checkUpdate,
    applyUpdate,
    restartServer,
    connectEvents,
  } from './api.js'

  // ---- state -----------------------------------------------------------
  let tasks = []            // active tasks (queued / downloading / paused)
  let completed = []        // completed + errored, collapsed panel
  let globalSpeed = 0
  let activeCount = 0
  let connected = false

  // QoS panel
  let qosEnabled = false
  let qosMbps = 5
  let qosSaving = false
  let qosNotice = ''

  // Add modal
  let showModal = false
  let urlText = ''
  let saveDir = ''
  let segments = 8
  let addError = ''
  let adding = false

  // Updates panel
  let serverVersion = ''
  let uptime = 0
  let updateInfo = null      // result of /api/update/check
  let updateBusy = false
  let updateMsg = ''

  let notice = ''

  // ---- helpers ---------------------------------------------------------
  const fmtBytes = (n) => {
    if (n == null) return '?'
    const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
    let v = n
    let u = 0
    while (v >= 1024 && u < units.length - 1) { v /= 1024; u++ }
    return `${v >= 100 || u === 0 ? Math.round(v) : v.toFixed(1)} ${units[u]}`
  }
  const fmtSpeed = (bps) => (bps > 0 ? `${fmtBytes(bps)}/s` : '—')
  const fmtUptime = (s) => {
    if (!s) return '—'
    const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60)
    return h > 0 ? `${h}h ${m}m` : m > 0 ? `${m}m` : `${s}s`
  }
  const fmtPct = (t) => {
    if (!t || !t.total_bytes) return (t.state === 'complete' ? 100 : 0)
    return Math.min(100, Math.round((t.downloaded_bytes / t.total_bytes) * 100))
  }
  const stateLabel = (s) =>
    ({ queued: 'queued', downloading: 'downloading', paused: 'paused',
       complete: 'done', error: 'error', removed: 'removed' }[s] ?? s)
  const badgeClass = (s) =>
    ({ downloading: 'badge-info', paused: 'badge-warning',
       complete: 'badge-success', error: 'badge-error' }[s] ?? 'badge-ghost')

  async function refresh() {
    try {
      tasks = await listTasks('active')
      completed = await listTasks('completed')
    } catch (e) {
      notice = `failed to load tasks: ${e.message}`
    }
  }

  async function refreshQos() {
    try {
      const q = await getQos()
      qosEnabled = q.enabled
      if (q.target_bps) qosMbps = Math.round(q.target_bps / 1_000_000 * 100) / 100
    } catch (_) { /* defaults are fine */ }
  }

  async function refreshServer() {
    try {
      const info = await getServerInfo()
      serverVersion = info.version
      uptime = info.uptime_secs
    } catch (_) { /* optional */ }
  }

  async function toggleQos() {
    qosSaving = true
    qosNotice = ''
    try {
      const bps = qosEnabled ? Math.max(1, Math.round(qosMbps * 1_000_000)) : 0
      await setQos(qosEnabled, bps)
      qosNotice = qosEnabled ? `QoS on — capped at ${fmtBytes(bps)}/s` : 'QoS off — full speed'
    } catch (e) {
      qosNotice = `error: ${e.message}`
      await refreshQos() // resync with server truth
    } finally {
      qosSaving = false
    }
  }

  async function submitAdd() {
    addError = ''
    const urls = urlText.split('\n').map((s) => s.trim()).filter(Boolean)
    if (urls.length === 0) {
      addError = 'enter at least one URL'
      return
    }
    adding = true
    try {
      await createTasks({ urls, saveDir: saveDir.trim() || undefined, segments })
      showModal = false
      urlText = ''
      await refresh()
    } catch (e) {
      addError = e.message
    } finally {
      adding = false
    }
  }

  async function act(task, action) {
    try {
      if (action === 'pause') await pauseTask(task.id)
      else if (action === 'resume') await resumeTask(task.id)
      else if (action === 'cancel') await cancelTask(task.id)
      else if (action === 'retry') await retryTask(task.id)
      else if (action === 'delete') await deleteTask(task.id)
      else if (action === 'delete-file') await deleteTaskWithFile(task.id)
      await refresh()
    } catch (e) {
      notice = `${action} failed: ${e.message}`
    }
  }

  // ---- updates ---------------------------------------------------------
  async function doCheckUpdate() {
    updateBusy = true
    updateMsg = ''
    try {
      updateInfo = await checkUpdate()
      updateMsg = updateInfo.available
        ? `version ${updateInfo.latest} is available`
        : updateInfo.latest
          ? 'you are on the latest release'
          : updateInfo.error ?? 'no release information'
    } catch (e) {
      updateMsg = `check failed: ${e.message}`
    } finally {
      updateBusy = false
    }
  }

  async function doApplyUpdate() {
    updateBusy = true
    updateMsg = 'installing… the server will restart and auto-resume downloads'
    try {
      const res = await applyUpdate(true)
      updateMsg = `installed ${res.installed} — restarting…`
    } catch (e) {
      updateMsg = `install failed: ${e.message}`
    } finally {
      updateBusy = false
    }
  }

  async function doRestart() {
    updateBusy = true
    updateMsg = 'restarting…'
    try {
      await restartServer()
      updateMsg = 'restarting — reconnecting in a few seconds'
    } catch (e) {
      updateMsg = `restart failed: ${e.message}`
    } finally {
      updateBusy = false
    }
  }

  // ---- live events ------------------------------------------------------
  let ws

  function handleEvent(ev) {
    if (ev.event === 'global:speed') {
      globalSpeed = ev.speed_bps ?? 0
      activeCount = ev.active_tasks ?? 0
      return
    }
    if (!ev.task_id) return
    const t = tasks.find((x) => x.id === ev.task_id)
    if (ev.event === 'task:progress' && t) {
      t.downloaded_bytes = ev.downloaded_bytes
      t.total_bytes = ev.total_bytes ?? t.total_bytes
      t._speed = ev.speed_bps ?? 0
      tasks = tasks // trigger reactivity
    } else if (ev.event === 'task:state') {
      // State changed — the active/completed split may have changed too.
      refresh()
    }
  }

  onMount(() => {
    refresh()
    refreshQos()
    refreshServer()
    ws = connectEvents(handleEvent)
    ws.onopen = () => (connected = true)
    ws.onclose = () => (connected = false)
  })

  onDestroy(() => {
    try { ws?.close() } catch (_) {}
  })
</script>

<header class="navbar sticky top-0 z-20 h-14 min-h-0 border-b border-base-300 bg-base-100/90 backdrop-blur">
  <div class="flex items-center gap-2 px-2">
    <span class="text-xl text-primary">⇣</span>
    <h1 class="text-base font-semibold tracking-wide">HyprFetch</h1>
    <span
      class="badge badge-sm {connected ? 'badge-success' : 'badge-ghost'} uppercase"
      title="websocket status"
    >{connected ? 'live' : 'offline'}</span>
  </div>
  <div class="flex items-center gap-3 px-2">
    <span class="font-mono text-sm text-secondary" title="aggregate download speed">{fmtSpeed(globalSpeed)}</span>
    <span class="text-xs opacity-60">{activeCount} active</span>
    <button class="btn btn-primary btn-sm" on:click={() => (showModal = true)}>+ Add</button>
  </div>
</header>

{#if notice}
  <div class="alert alert-warning mx-auto mt-3 max-w-3xl cursor-pointer text-sm shadow" role="alert" on:click={() => (notice = '')}>
    <span>{notice} ✕</span>
  </div>
{/if}

<main class="mx-auto max-w-3xl px-5 pb-16 pt-4">
  <!-- Active -->
  <section>
    <h2 class="mb-2 mt-4 text-xs font-semibold uppercase tracking-widest opacity-50">Active</h2>
    {#if tasks.length === 0}
      <p class="rounded-box bg-base-200/40 p-4 text-sm opacity-60">
        Nothing downloading. Hit <b>+ Add</b> to fetch something.
      </p>
    {:else}
      {#each tasks as t (t.id)}
        <article class="card mb-2 border border-base-300 bg-base-200 shadow-sm">
          <div class="card-body gap-2 p-4">
            <div class="flex flex-wrap items-center gap-2">
              <span class="max-w-[40%] truncate font-semibold" title={t.url}>{t.filename}</span>
              <span class="badge badge-sm {badgeClass(t.state)} uppercase">{stateLabel(t.state)}</span>
              <span class="grow" />
              <span class="font-mono text-xs text-secondary">{t._speed ? fmtSpeed(t._speed) : ''}</span>
              <span class="font-mono text-xs opacity-70">{fmtBytes(t.downloaded_bytes)} / {fmtBytes(t.total_bytes)}</span>
            </div>
            <progress
              class="progress {t.state === 'error' ? 'progress-error' : t.state === 'paused' ? 'progress-warning' : 'progress-primary'} h-1.5"
              value={fmtPct(t)} max="100"
            />
            <div class="flex flex-wrap items-center gap-2">
              {#if t.state === 'downloading' || t.state === 'queued'}
                <button class="btn btn-xs" on:click={() => act(t, 'pause')}>Pause</button>
              {:else if t.state === 'paused'}
                <button class="btn btn-xs btn-primary" on:click={() => act(t, 'resume')}>Resume</button>
              {:else if t.state === 'error'}
                <button class="btn btn-xs" on:click={() => act(t, 'retry')}>Retry</button>
              {/if}
              {#if t.state !== 'complete'}
                <button class="btn btn-xs btn-outline btn-error" on:click={() => act(t, 'cancel')}>Cancel</button>
              {/if}
              {#if t.error}
                <span class="truncate text-xs text-error" title={t.error}>{t.error}</span>
              {/if}
            </div>
          </div>
        </article>
      {/each}
    {/if}
  </section>

  <!-- Finished -->
  <section>
    <h2 class="mb-2 mt-6 text-xs font-semibold uppercase tracking-widest opacity-50">Finished</h2>
    {#if completed.length === 0}
      <p class="rounded-box bg-base-200/40 p-4 text-sm opacity-60">No finished downloads yet.</p>
    {:else}
      {#each completed as t (t.id)}
        <article class="card mb-1.5 border border-base-300 bg-base-200/70 shadow-sm">
          <div class="card-body flex-row items-center gap-2 p-3">
            <span class="max-w-[40%] truncate text-sm font-medium" title={t.url}>{t.filename}</span>
            <span class="badge badge-sm {badgeClass(t.state)} uppercase">{stateLabel(t.state)}</span>
            <span class="grow" />
            <span class="font-mono text-xs opacity-70">{fmtBytes(t.downloaded_bytes)}</span>
            {#if t.state === 'error'}
              <button class="btn btn-xs" on:click={() => act(t, 'retry')}>Retry</button>
            {/if}
            <button
              class="btn btn-xs btn-outline btn-error"
              title="remove task + downloaded file"
              on:click={() => act(t, 'delete-file')}>✕</button>
          </div>
        </article>
      {/each}
    {/if}
  </section>

  <!-- QoS -->
  <section>
    <h2 class="mb-2 mt-6 text-xs font-semibold uppercase tracking-widest opacity-50">QoS bandwidth cap</h2>
    <div class="card border border-base-300 bg-base-200 shadow-sm">
      <div class="card-body p-4">
        <div class="flex flex-wrap items-center gap-5">
          <label class="flex cursor-pointer items-center gap-2">
            <input
              type="checkbox"
              class="toggle toggle-primary"
              bind:checked={qosEnabled}
              on:change={toggleQos}
              disabled={qosSaving}
            />
            <span class="text-sm">{qosEnabled ? 'On' : 'Off'}</span>
          </label>
          <label class="flex items-center gap-2 text-sm">
            Limit
            <input
              type="number" min="0.1" step="0.5"
              class="input input-bordered input-sm w-24"
              bind:value={qosMbps}
              on:change={toggleQos}
              disabled={!qosEnabled || qosSaving}
            />
            MiB/s
          </label>
          {#if qosNotice}<span class="text-sm text-success">{qosNotice}</span>{/if}
        </div>
        <p class="text-xs opacity-50">The cap applies to the <em>total</em> of all running downloads (one shared token bucket).</p>
      </div>
    </div>
  </section>

  <!-- Updates -->
  <section>
    <h2 class="mb-2 mt-6 text-xs font-semibold uppercase tracking-widest opacity-50">Updates</h2>
    <div class="card border border-base-300 bg-base-200 shadow-sm">
      <div class="card-body gap-3 p-4">
        <div class="flex flex-wrap items-center gap-3 text-sm">
          <span class="font-mono">v{serverVersion || '…'}</span>
          <span class="text-xs opacity-50">up {fmtUptime(uptime)}</span>
          <span class="grow" />
          <button class="btn btn-sm" disabled={updateBusy} on:click={doCheckUpdate}>Check for updates</button>
          {#if updateInfo?.available}
            <button class="btn btn-sm btn-primary" disabled={updateBusy} on:click={doApplyUpdate}>
              Install {updateInfo.latest} & restart
            </button>
          {/if}
          <button class="btn btn-sm btn-ghost" disabled={updateBusy} on:click={doRestart}>Restart server</button>
        </div>
        {#if updateMsg}<p class="text-xs opacity-70">{updateMsg}</p>{/if}
        <p class="text-xs opacity-50">
          Updates are pulled straight from GitHub releases, sha256-verified, and applied with an
          atomic binary swap. Active downloads are paused and auto-resumed after the restart.
        </p>
      </div>
    </div>
  </section>
</main>

{#if showModal}
  <div class="modal modal-open" on:click|self={() => (showModal = false)}>
    <form class="modal-box max-w-md" on:submit|preventDefault={submitAdd}>
      <h3 class="mb-3 text-lg font-semibold">Add download</h3>
      <div class="grid gap-3">
        <label class="grid gap-1.5 text-sm">
          <span>URLs <span class="opacity-50">(one per line)</span></span>
          <textarea rows="4" class="textarea textarea-bordered" bind:value={urlText} placeholder="https://example.com/file.iso" />
        </label>
        <label class="grid gap-1.5 text-sm">
          <span>Save directory <span class="opacity-50">(blank = server default)</span></span>
          <input type="text" class="input input-bordered" bind:value={saveDir} placeholder="/home/you/Downloads" />
        </label>
        <label class="grid gap-1.5 text-sm">
          <span>Segments</span>
          <input type="number" min="1" max="32" class="input input-bordered w-28" bind:value={segments} />
        </label>
        {#if addError}<p class="text-sm text-error">{addError}</p>{/if}
      </div>
      <div class="modal-action">
        <button type="button" class="btn btn-sm" on:click={() => (showModal = false)}>Cancel</button>
        <button type="submit" class="btn btn-primary btn-sm" disabled={adding}>
          {adding ? 'Adding…' : 'Download'}
        </button>
      </div>
    </form>
  </div>
{/if}
