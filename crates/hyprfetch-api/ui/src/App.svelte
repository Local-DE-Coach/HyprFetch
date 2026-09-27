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
  const fmtPct = (t) => {
    if (!t || !t.total_bytes) return (t.state === 'complete' ? 100 : 0)
    return Math.min(100, Math.round((t.downloaded_bytes / t.total_bytes) * 100))
  }
  const stateLabel = (s) =>
    ({ queued: 'queued', downloading: 'downloading', paused: 'paused',
       complete: 'done', error: 'error', removed: 'removed' }[s] ?? s)

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
    ws = connectEvents(handleEvent)
    ws.onopen = () => (connected = true)
    ws.onclose = () => (connected = false)
  })

  onDestroy(() => {
    try { ws?.close() } catch (_) {}
  })
</script>

<header>
  <div class="brand">
    <span class="logo">⇣</span>
    <h1>HyprFetch</h1>
    <span class="pill" class:live={connected} title="websocket status">
      {connected ? 'live' : 'offline'}
    </span>
  </div>
  <div class="global">
    <span class="speed" title="aggregate download speed">{fmtSpeed(globalSpeed)}</span>
    <span class="count">{activeCount} active</span>
    <button class="primary" on:click={() => (showModal = true)}>+ Add</button>
  </div>
</header>

{#if notice}
  <div class="notice" role="alert" on:click={() => (notice = '')}>{notice} ✕</div>
{/if}

<main>
  <section>
    <h2>Active</h2>
    {#if tasks.length === 0}
      <p class="empty">Nothing downloading. Hit <b>+ Add</b> to fetch something.</p>
    {:else}
      {#each tasks as t (t.id)}
        <article class="task">
          <div class="row">
            <span class="name" title={t.url}>{t.filename}</span>
            <span class="badge {t.state}">{stateLabel(t.state)}</span>
            <span class="grow" />
            <span class="speed">{t._speed ? fmtSpeed(t._speed) : ''}</span>
            <span class="bytes">{fmtBytes(t.downloaded_bytes)} / {fmtBytes(t.total_bytes)}</span>
          </div>
          <div class="bar">
            <div class="fill {t.state}" style={`width:${fmtPct(t)}%`} />
          </div>
          <div class="row actions">
            {#if t.state === 'downloading' || t.state === 'queued'}
              <button on:click={() => act(t, 'pause')}>Pause</button>
            {:else if t.state === 'paused'}
              <button on:click={() => act(t, 'resume')}>Resume</button>
            {:else if t.state === 'error'}
              <button on:click={() => act(t, 'retry')}>Retry</button>
            {/if}
            {#if t.state !== 'complete'}
              <button class="danger" on:click={() => act(t, 'cancel')}>Cancel</button>
            {/if}
            {#if t.error}
              <span class="err" title={t.error}>{t.error}</span>
            {/if}
          </div>
        </article>
      {/each}
    {/if}
  </section>

  <section>
    <h2>Finished</h2>
    {#if completed.length === 0}
      <p class="empty">No finished downloads yet.</p>
    {:else}
      {#each completed as t (t.id)}
        <article class="task compact">
          <div class="row">
            <span class="name" title={t.url}>{t.filename}</span>
            <span class="badge {t.state}">{stateLabel(t.state)}</span>
            <span class="grow" />
            <span class="bytes">{fmtBytes(t.downloaded_bytes)}</span>
            {#if t.state === 'error'}
              <button on:click={() => act(t, 'retry')}>Retry</button>
            {/if}
            <button class="danger" title="remove task + downloaded file" on:click={() => act(t, 'delete-file')}>✕</button>
          </div>
        </article>
      {/each}
    {/if}
  </section>

  <section>
    <h2>QoS bandwidth cap</h2>
    <div class="qos">
      <label class="switch">
        <input type="checkbox" bind:checked={qosEnabled} on:change={toggleQos} disabled={qosSaving} />
        <span class="slider" />
        <span>{qosEnabled ? 'On' : 'Off'}</span>
      </label>
      <label class="rate">
        Limit
        <input type="number" min="0.1" step="0.5" bind:value={qosMbps} on:change={toggleQos} disabled={!qosEnabled || qosSaving} />
        MiB/s
      </label>
      {#if qosNotice}<span class="qos-note">{qosNotice}</span>{/if}
    </div>
    <p class="hint">The cap applies to the <em>total</em> of all running downloads (one shared token bucket).</p>
  </section>
</main>

{#if showModal}
  <div class="overlay" on:click|self={() => (showModal = false)}>
    <form class="modal" on:submit|preventDefault={submitAdd}>
      <h3>Add download</h3>
      <label>
        URLs <span class="hint">(one per line)</span>
        <textarea rows="4" bind:value={urlText} placeholder="https://example.com/file.iso" />
      </label>
      <label>
        Save directory <span class="hint">(blank = server default)</span>
        <input type="text" bind:value={saveDir} placeholder="/home/you/Downloads" />
      </label>
      <label>
        Segments
        <input type="number" min="1" max="32" bind:value={segments} />
      </label>
      {#if addError}<p class="err">{addError}</p>{/if}
      <div class="row">
        <button type="button" on:click={() => (showModal = false)}>Cancel</button>
        <span class="grow" />
        <button type="submit" class="primary" disabled={adding}>
          {adding ? 'Adding…' : 'Download'}
        </button>
      </div>
    </form>
  </div>
{/if}

<style>
  :global(body) {
    margin: 0;
    background: #0f1014;
    color: #e6e6eb;
    font: 15px/1.45 system-ui, -apple-system, 'Segoe UI', sans-serif;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 22px;
    border-bottom: 1px solid #23252e;
    position: sticky;
    top: 0;
    background: rgba(15, 16, 20, 0.92);
    backdrop-filter: blur(6px);
  }
  .brand { display: flex; align-items: center; gap: 10px; }
  .logo { font-size: 20px; color: #7aa2f7; }
  h1 { font-size: 17px; margin: 0; letter-spacing: 0.4px; }
  .pill {
    font-size: 11px; padding: 2px 8px; border-radius: 999px;
    background: #2a2c36; color: #9aa0b0; text-transform: uppercase;
  }
  .pill.live { background: #17321f; color: #58d68d; }
  .global { display: flex; align-items: center; gap: 14px; }
  .speed { font-variant-numeric: tabular-nums; color: #9dd6ff; }
  .count { color: #9aa0b0; font-size: 13px; }

  main { max-width: 760px; margin: 0 auto; padding: 20px 22px 60px; }
  h2 {
    font-size: 12px; text-transform: uppercase; letter-spacing: 1.2px;
    color: #8b91a3; margin: 26px 0 10px;
  }
  .empty { color: #6c7280; font-size: 14px; }

  .task {
    background: #16181f;
    border: 1px solid #23252e;
    border-radius: 10px;
    padding: 12px 14px;
    margin-bottom: 10px;
  }
  .task.compact { padding: 8px 14px; }
  .row { display: flex; align-items: center; gap: 10px; }
  .grow { flex: 1; }
  .name { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 40%; }
  .bytes, .speed { font-variant-numeric: tabular-nums; font-size: 13px; color: #b9bece; }
  .err { color: #ff7b7b; font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .badge {
    font-size: 11px; padding: 2px 8px; border-radius: 999px; text-transform: uppercase;
    background: #262a35; color: #aab0c0;
  }
  .badge.downloading { background: #16324a; color: #7cc7ff; }
  .badge.paused { background: #3a3320; color: #ffd479; }
  .badge.complete { background: #17321f; color: #58d68d; }
  .badge.error { background: #3a1f22; color: #ff7b7b; }

  .bar {
    height: 6px; background: #23252e; border-radius: 999px; margin: 10px 0 8px; overflow: hidden;
  }
  .fill { height: 100%; background: linear-gradient(90deg, #7aa2f7, #9dd6ff); border-radius: 999px; transition: width 0.4s ease; }
  .fill.paused { background: #ffd479; }
  .fill.error { background: #ff7b7b; }

  .actions button { margin-right: 8px; }

  button {
    background: #22242e; color: #e6e6eb; border: 1px solid #2f3240;
    padding: 6px 12px; border-radius: 8px; font-size: 13px; cursor: pointer;
  }
  button:hover { background: #2a2d39; }
  button.primary { background: #3d5fc4; border-color: #3d5fc4; }
  button.primary:hover { background: #4a6fd8; }
  button.danger { border-color: #4a2b2f; color: #ff9b9b; }
  button:disabled { opacity: 0.5; cursor: default; }

  .qos { display: flex; align-items: center; gap: 18px; }
  .qos-note { color: #58d68d; font-size: 13px; }
  .hint { color: #6c7280; font-size: 13px; }
  .rate input {
    width: 70px; margin: 0 6px; background: #16181f; border: 1px solid #2f3240;
    color: #e6e6eb; border-radius: 6px; padding: 5px 8px;
  }

  .switch { display: inline-flex; align-items: center; gap: 8px; cursor: pointer; }
  .switch input { display: none; }
  .slider {
    width: 40px; height: 22px; background: #2a2c36; border-radius: 999px;
    position: relative; transition: background 0.2s;
  }
  .slider::after {
    content: ''; position: absolute; top: 3px; left: 3px; width: 16px; height: 16px;
    border-radius: 50%; background: #9aa0b0; transition: transform 0.2s, background 0.2s;
  }
  .switch input:checked + .slider { background: #3d5fc4; }
  .switch input:checked + .slider::after { transform: translateX(18px); background: #fff; }

  .notice {
    max-width: 760px; margin: 12px auto 0; padding: 8px 14px; cursor: pointer;
    background: #3a3320; color: #ffd479; border-radius: 8px; font-size: 13px;
  }

  .overlay {
    position: fixed; inset: 0; background: rgba(0, 0, 0, 0.55);
    display: flex; align-items: center; justify-content: center; z-index: 10;
  }
  .modal {
    width: min(480px, 90vw); background: #16181f; border: 1px solid #2f3240;
    border-radius: 14px; padding: 20px; display: grid; gap: 12px;
  }
  .modal h3 { margin: 0; }
  .modal label { display: grid; gap: 5px; font-size: 13px; color: #b9bece; }
  .modal textarea, .modal input[type='text'] {
    background: #0f1014; border: 1px solid #2f3240; color: #e6e6eb;
    border-radius: 8px; padding: 8px 10px; font: inherit;
  }
  .modal input[type='number'] {
    background: #0f1014; border: 1px solid #2f3240; color: #e6e6eb;
    border-radius: 8px; padding: 6px 10px; width: 90px;
  }
  .modal .err { margin: 0; }
</style>
