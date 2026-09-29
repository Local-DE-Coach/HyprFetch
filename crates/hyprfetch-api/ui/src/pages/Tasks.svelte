<script>
  // Tasks — every download task with status filters, search and per-task
  // controls (pause / resume / retry / cancel / remove).
  import { fmtBytes, fmtSpeed, fmtPct, fmtDate, stateLabel, badgeClass, categoryIcon } from '../lib/format.js'
  import { active, finished, doTaskAction, showAdd } from '../lib/store.js'

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
  <div class="tabs tabs-boxed bg-base-200">
    {#each FILTERS as f (f.id)}
      <button class="tab {filter === f.id ? 'tab-active' : ''}" on:click={() => (filter = f.id)}>
        {f.label}
        <span class="ml-1 opacity-50">{counts[f.id]}</span>
      </button>
    {/each}
  </div>
  <input
    type="search"
    class="input input-bordered input-sm w-44"
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
  <div class="overflow-x-auto rounded-box border border-base-300 bg-base-200 shadow-sm">
    <table class="table table-zebra table-sm">
      <thead>
        <tr>
          <th>File</th>
          <th class="hidden md:table-cell">Category</th>
          <th>Status</th>
          <th class="hidden sm:table-cell">Progress</th>
          <th class="text-right">Speed</th>
          <th class="hidden lg:table-cell">Saved to</th>
          <th class="hidden xl:table-cell text-right">Added</th>
          <th class="text-right">Actions</th>
        </tr>
      </thead>
      <tbody>
        {#each filtered as t (t.id)}
          <tr>
            <td>
              <div class="max-w-[180px] truncate font-medium" title={t.url}>{t.filename}</div>
              {#if t.error}<div class="max-w-[180px] truncate text-xs text-error" title={t.error}>{t.error}</div>{/if}
            </td>
            <td class="hidden md:table-cell">
              <span class="badge badge-sm badge-ghost">{categoryIcon(t.category)} {t.category}</span>
            </td>
            <td><span class="badge badge-sm {badgeClass(t.state)} uppercase">{stateLabel(t.state)}</span></td>
            <td class="hidden sm:table-cell">
              <div class="flex items-center gap-2">
                <progress
                  class="progress {t.state === 'error' ? 'progress-error' : t.state === 'paused' ? 'progress-warning' : t.state === 'complete' ? 'progress-success' : 'progress-primary'} h-1.5 w-24"
                  value={fmtPct(t)} max="100" />
                <span class="font-mono text-xs opacity-60">{fmtPct(t)}%</span>
              </div>
              <div class="font-mono text-xs opacity-50">{fmtBytes(t.downloaded_bytes)} / {fmtBytes(t.total_bytes)}</div>
            </td>
            <td class="text-right font-mono text-xs text-secondary">{t.state === 'downloading' ? fmtSpeed(t._speed) : '—'}</td>
            <td class="hidden max-w-[220px] lg:table-cell">
              <span class="truncate font-mono text-xs opacity-50" title={t.save_path}>{t.save_path}</span>
            </td>
            <td class="hidden xl:table-cell text-right font-mono text-xs opacity-50">{fmtDate(t.created_at)}</td>
            <td class="text-right">
              <div class="flex justify-end gap-1">
                {#if t.state === 'downloading' || t.state === 'queued'}
                  <button class="btn btn-xs" on:click={() => doTaskAction(t, 'pause')}>Pause</button>
                {:else if t.state === 'paused'}
                  <button class="btn btn-xs btn-primary" on:click={() => doTaskAction(t, 'resume')}>Resume</button>
                {:else if t.state === 'error'}
                  <button class="btn btn-xs" on:click={() => doTaskAction(t, 'retry')}>Retry</button>
                {/if}
                {#if t.state !== 'complete'}
                  <button class="btn btn-xs btn-ghost" title="cancel" on:click={() => doTaskAction(t, 'cancel')}>✗</button>
                {/if}
                <button
                  class="btn btn-xs btn-ghost text-error"
                  title="remove task + file"
                  on:click={() => doTaskAction(t, 'delete-file')}>🗑</button>
              </div>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
