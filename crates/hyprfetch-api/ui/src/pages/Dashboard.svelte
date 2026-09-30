<script>
  // Dashboard — the at-a-glance overview: counters, live speed, active
  // downloads, recent finishes and where files land on disk.
  import { page, nav } from '../lib/router.js'
  import { fmtBytes, fmtSpeed, fmtPct, stateLabel, badgeClass, categoryIcon, fmtDate } from '../lib/format.js'
  import {
    active, finished, globalSpeed, activeCount, categories, showAdd, openSaveFolder,
  } from '../lib/store.js'
  import TaskCard from '../lib/TaskCard.svelte'
  import FileActions from '../lib/FileActions.svelte'

  $: stats = {
    downloading: $active.filter((t) => t.state === 'downloading').length,
    queued: $active.filter((t) => t.state === 'queued').length,
    paused: $active.filter((t) => t.state === 'paused').length,
    done: $finished.filter((t) => t.state === 'complete').length,
    error: $finished.filter((t) => t.state === 'error').length,
  }

  $: downloadedTotal = [...$finished, ...$active].reduce(
    (sum, t) => sum + (t.state === 'complete' ? t.downloaded_bytes : 0), 0)

  $: recent = [...$finished]
    .sort((a, b) => (b.completed_at ?? b.updated_at) - (a.completed_at ?? a.updated_at))
    .slice(0, 6)

  function openAdd() { showAdd.set(true) }
</script>

<!-- stat row -->
<div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6">
  <div class="stat border border-base-300 bg-base-200 shadow-sm">
    <div class="stat-figure text-secondary">⇣</div>
    <div class="stat-title text-xs">Speed</div>
    <div class="stat-value text-xl text-secondary">{fmtSpeed($globalSpeed)}</div>
    <div class="stat-desc">{$activeCount} active task{$activeCount === 1 ? '' : 's'}</div>
  </div>
  <div class="stat border border-base-300 bg-base-200 shadow-sm">
    <div class="stat-title text-xs">Downloading</div>
    <div class="stat-value text-xl text-info">{stats.downloading}</div>
    <div class="stat-desc">right now</div>
  </div>
  <div class="stat border border-base-300 bg-base-200 shadow-sm">
    <div class="stat-title text-xs">Queued</div>
    <div class="stat-value text-xl">{stats.queued}</div>
    <div class="stat-desc">waiting for a slot</div>
  </div>
  <div class="stat border border-base-300 bg-base-200 shadow-sm">
    <div class="stat-title text-xs">Paused</div>
    <div class="stat-value text-xl text-warning">{stats.paused}</div>
    <div class="stat-desc">resumable</div>
  </div>
  <div class="stat border border-base-300 bg-base-200 shadow-sm">
    <div class="stat-title text-xs">Done</div>
    <div class="stat-value text-xl text-success">{stats.done}</div>
    <div class="stat-desc">{fmtBytes(downloadedTotal)} on disk</div>
  </div>
  <div class="stat border border-base-300 bg-base-200 shadow-sm">
    <div class="stat-title text-xs">Errors</div>
    <div class="stat-value text-xl text-error">{stats.error}</div>
    <div class="stat-desc">retry available</div>
  </div>
</div>

<!-- active downloads -->
<section class="mt-6">
  <div class="mb-2 flex items-center justify-between">
    <h2 class="text-xs font-semibold uppercase tracking-widest opacity-50">Downloading now</h2>
    <button class="btn btn-primary btn-xs" on:click={openAdd}>+ Add download</button>
  </div>
  {#if $active.length === 0}
    <p class="rounded-box bg-base-200/40 p-4 text-sm opacity-60">
      Nothing downloading. Hit <b>+ Add download</b> to fetch something — files are
      auto-sorted into your video / pictures / music folders.
    </p>
  {:else}
    {#each $active as t (t.id)}
      <TaskCard task={t} />
    {/each}
  {/if}
</section>

<!-- where files land -->
<section class="mt-6">
  <h2 class="mb-2 text-xs font-semibold uppercase tracking-widest opacity-50">Save folders</h2>
  <p class="mb-2 text-xs opacity-50">
    Files are sorted automatically by type. Base folder: <code class="font-mono">{$categories.base}</code>
    — click any folder to open it in your file manager.
    {#if !$categories.categorize}<span class="text-warning"> (auto-sort is OFF)</span>{/if}
  </p>
  <div class="grid grid-cols-2 gap-2 sm:grid-cols-4 lg:grid-cols-7">
    {#each $categories.categories as c (c.name)}
      <button
        class="card border border-base-300 bg-base-200/60 text-left shadow-sm transition-all hover:border-primary hover:bg-base-200 hover:shadow"
        title="Open {c.dir} in the file manager"
        on:click={() => openSaveFolder(c.dir)}
      >
        <div class="card-body p-3">
          <div class="text-lg">{categoryIcon(c.name)}</div>
          <div class="text-sm font-medium capitalize">{c.name}</div>
          <div class="truncate font-mono text-[10px] opacity-50" title={c.dir}>{c.dir}</div>
        </div>
      </button>
    {/each}
  </div>
  <div class="mt-1 flex items-center gap-1">
    <button class="btn btn-ghost btn-xs" on:click={() => openSaveFolder($categories.base)} title="Open {$categories.base} in the file manager">Open base folder →</button>
    <button class="btn btn-ghost btn-xs" on:click={() => nav('settings')}>Edit folders →</button>
  </div>
</section>

<!-- recent finished -->
<section class="mt-6">
  <div class="mb-2 flex items-center justify-between">
    <h2 class="text-xs font-semibold uppercase tracking-widest opacity-50">Recent finished</h2>
    <button class="btn btn-ghost btn-xs" on:click={() => nav('tasks')}>View all →</button>
  </div>
  {#if recent.length === 0}
    <p class="rounded-box bg-base-200/40 p-4 text-sm opacity-60">No finished downloads yet.</p>
  {:else}
    {#each recent as t (t.id)}
      <article class="card group mb-1.5 border border-base-300 bg-base-200/70 shadow-sm">
        <div class="card-body flex-row items-center gap-2 p-3">
          <span class="max-w-[30%] truncate text-sm font-medium sm:max-w-[35%]" title={t.url}>{t.filename}</span>
          <!-- GO / Open — hover reveals the desktop actions next to the file -->
          <FileActions task={t} show={t.state === 'complete'} />
          <span class="badge badge-sm badge-ghost max-sm:hidden">{categoryIcon(t.category)} {t.category}</span>
          <span class="badge badge-sm {badgeClass(t.state)} uppercase">{stateLabel(t.state)}</span>
          <span class="grow" />
          <span class="hidden font-mono text-xs opacity-50 sm:inline">{fmtDate(t.completed_at ?? t.updated_at)}</span>
          <span class="font-mono text-xs opacity-70">{fmtBytes(t.downloaded_bytes)}</span>
          <progress class="progress progress-success h-1 w-16 max-sm:hidden" value={fmtPct(t)} max="100" />
        </div>
      </article>
    {/each}
  {/if}
</section>
