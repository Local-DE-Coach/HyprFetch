<script>
  // One download card with progress + lifecycle buttons. Used by the
  // Dashboard and the Tasks page.
  import { fmtBytes, fmtSpeed, fmtPct, stateLabel, badgeClass, categoryIcon } from './format.js'
  import { doTaskAction } from './store.js'

  export let task
  export let compact = false
</script>

<article class="card {compact ? 'mb-1.5' : 'mb-2'} border border-base-300 bg-base-200 shadow-sm">
  <div class="card-body gap-2 {compact ? 'flex-row items-center gap-2 p-3' : 'p-4'}">
    <div class="flex min-w-0 flex-wrap items-center gap-2">
      <span class="max-w-[38%] truncate font-semibold" title={task.url}>{task.filename}</span>
      <span class="badge badge-sm badge-ghost gap-1" title="auto-sorted by file type">
        {categoryIcon(task.category)} {task.category}
      </span>
      <span class="badge badge-sm {badgeClass(task.state)} uppercase">{stateLabel(task.state)}</span>
      <span class="grow" />
      <span class="font-mono text-xs text-secondary">{task._speed ? fmtSpeed(task._speed) : ''}</span>
      <span class="font-mono text-xs opacity-70">
        {fmtBytes(task.downloaded_bytes)} / {fmtBytes(task.total_bytes)}
      </span>
    </div>

    {#if !compact}
      <progress
        class="progress {task.state === 'error' ? 'progress-error' : task.state === 'paused' ? 'progress-warning' : 'progress-primary'} h-1.5"
        value={fmtPct(task)} max="100"
      />
    {/if}

    <div class="flex flex-wrap items-center gap-2">
      {#if task.state === 'downloading' || task.state === 'queued'}
        <button class="btn btn-xs" on:click={() => doTaskAction(task, 'pause')}>Pause</button>
      {:else if task.state === 'paused'}
        <button class="btn btn-xs btn-primary" on:click={() => doTaskAction(task, 'resume')}>Resume</button>
      {:else if task.state === 'error'}
        <button class="btn btn-xs" on:click={() => doTaskAction(task, 'retry')}>Retry</button>
      {/if}
      {#if task.state !== 'complete'}
        <button class="btn btn-xs btn-outline btn-error" on:click={() => doTaskAction(task, 'cancel')}>Cancel</button>
      {/if}
      {#if compact && task.state === 'error'}
        <button class="btn btn-xs" on:click={() => doTaskAction(task, 'retry')}>Retry</button>
      {/if}
      <button
        class="btn btn-xs btn-outline btn-error"
        title="remove task + downloaded file"
        on:click={() => doTaskAction(task, 'delete-file')}>✕</button>
      {#if task.error}
        <span class="truncate text-xs text-error" title={task.error}>{task.error}</span>
      {/if}
      {#if !compact}
        <span class="grow" />
        <span class="truncate font-mono text-xs opacity-50" title={task.save_path}>→ {task.save_path}</span>
      {/if}
    </div>
  </div>
</article>
