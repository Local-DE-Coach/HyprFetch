<script>
  // Tasks — every download task with status filters, search and per-task
  // controls. Responsive rows (not a wide table) so nothing is ever cut
  // off or unreadable on any screen size:
  //   row line 1: icon + filename + [GO] [Open] + category/state + actions
  //   row line 2: save path + added date (mono, truncated, full text on hover)
  //   row line 3: progress + size + speed
  import { fmtBytes, fmtSpeed, fmtPct, fmtDate, stateLabel, badgeClass, categoryIcon } from '../lib/format.js'
  import { active, finished, doTaskAction, showAdd } from '../lib/store.js'
  import FileActions from '../lib/FileActions.svelte'

  const FILTERS = [
    { id: 'all', label: 'All' },
    { id: 'active', label: 'Active' },
    { id: 'downloading', label: 'Downloading' },
    { id: 'complete', label: 'Done' },
    { id: 'error', label: 'Errors' },
  ]

  let filter = 'all'
  let query = ''

  $: all = [...$active, ...$finished].sort(
    (a, b) => b.created_at - a.created_at)

  $: filtered = all.filter((t) => {
    if (filter === 'active' && !['queued', 'downloading', 'paused'].includes(t.state)) return false
    if (['downloading', 'complete', 'error'].includes(filter) && t.state !== filter) return false
    if (query) {
      const q = query.toLowerCase()
      if (!t.filename.toLowerCase().includes(q) && !t.url.toLowerCase().includes(q)) return false
    }
    return true
  })

  $: counts = {
    all: all.length,
    active: all.filter((t) => ['queued', 'downloading', 'paused'].includes(t.state)).length,
    downloading: all.filter((t) => t.state === 'downloading').length,
    complete: all.filter((t) => t.state === 'complete').length,
    error: all.filter((t) => t.state === 'error').length,
  }
</script>

<div class="mb-3 flex flex-wrap items-center gap-2">
  <div class="tabs tabs-boxed max-w-full overflow-x-auto bg-base-200">
    {#each FILTERS as f (f.id)}
      <button class="tab {filter === f.id ? 'tab-active' : ''}" on:click={() => (filter = f.id)}>
        {f.label}
        <span class="ml-1 opacity-50">{counts[f.id]}</span>
      </button>
    {/each}
  </div>
  <input
    type="search"
    class="input input-bordered input-sm w-40 sm:w-44"
    placeholder="Search name or URL…"
    bind:value={query}
  />
  <span class="grow" />
  <button class="btn btn-primary btn-sm" on:click={() => showAdd.set(true)}>+ Add download</button>
</div>

{#if filtered.length === 0}
  <p class="rounded-box bg-base-200/40 p-6 text-center text-sm opacity-60">
    No tasks {filter === 'all' ? 'yet' : `in “${FILTERS.find((f) => f.id === filter)?.label}”`}.
  </p>
{:else}
  <div class="divide-y divide-base-300 rounded-box border border-base-300 bg-base-200 shadow-sm">
    {#each filtered as t (t.id)}
      <div class="group p-3 transition-colors hover:bg-base-300/40">
        <!-- line 1: file + hover file-actions + lifecycle actions -->
        <div class="flex flex-wrap items-center gap-2">
          <span class="text-base" title={t.category}>{categoryIcon(t.category)}</span>
          <span class="max-w-[16rem] truncate text-sm font-semibold sm:max-w-[22rem]" title={t.url}>{t.filename}</span>

          <!-- GO / Open — next to the file, appear on hover -->
          <FileActions task={t} show={t.state === 'complete'} />

          <span class="badge badge-sm badge-ghost max-sm:hidden">{categoryIcon(t.category)} {t.category}</span>
          <span class="badge badge-sm {badgeClass(t.state)} uppercase">{stateLabel(t.state)}</span>
          <span class="grow" />

          <div class="flex items-center gap-1">
            {#if t.state === 'downloading' || t.state === 'queued'}
              <button class="btn btn-xs" on:click={() => doTaskAction(t, 'pause')}>Pause</button>
            {:else if t.state === 'paused'}
              <button class="btn btn-xs btn-primary" on:click={() => doTaskAction(t, 'resume')}>Resume</button>
            {:else if t.state === 'error'}
              <button class="btn btn-xs btn-primary" on:click={() => doTaskAction(t, 'retry')}>Retry</button>
            {:else if t.state === 'complete'}
              <!-- space holder: GO/Open already sit next to the filename -->
            {/if}
            {#if t.state !== 'complete'}
              <button class="btn btn-ghost btn-xs" title="cancel" on:click={() => doTaskAction(t, 'cancel')}>✗</button>
            {/if}
            <button
              class="btn btn-ghost btn-xs text-error"
              title="remove task + file"
              on:click={() => doTaskAction(t, 'delete-file')}>🗑</button>
          </div>
        </div>

        <!-- line 2: where + when (full values on hover / tap) -->
        <div class="mt-1 flex flex-wrap items-center gap-x-3 gap-y-0.5 font-mono text-xs opacity-50">
          <span class="min-w-0 max-w-full truncate" title={t.save_path}>→ {t.save_path}</span>
          <span title="added {fmtDate(t.created_at)}">added {fmtDate(t.created_at)}</span>
        </div>

        <!-- line 3: progress -->
        {#if t.state === 'downloading' || t.state === 'queued' || t.state === 'paused'}
          <div class="mt-1.5 flex items-center gap-2">
            <progress
              class="progress {t.state === 'paused' ? 'progress-warning' : 'progress-primary'} h-1.5 grow"
              value={fmtPct(t)} max="100" />
            <span class="font-mono text-xs opacity-60">{fmtPct(t)}%</span>
          </div>
          <div class="mt-0.5 flex items-center gap-3 font-mono text-xs opacity-50">
            <span>{fmtBytes(t.downloaded_bytes)} / {fmtBytes(t.total_bytes)}</span>
            <span class="text-secondary">{t.state === 'downloading' && t._speed ? fmtSpeed(t._speed) : ''}</span>
          </div>
        {:else if t.state === 'complete'}
          <div class="mt-1 font-mono text-xs opacity-50">{fmtBytes(t.downloaded_bytes)} on disk</div>
        {/if}

        {#if t.error}
          <div class="mt-1 max-w-full truncate text-xs text-error" title={t.error}>{t.error}</div>
        {/if}
      </div>
    {/each}
  </div>
{/if}
