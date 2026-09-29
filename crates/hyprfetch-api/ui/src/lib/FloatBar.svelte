<script>
  // Floating download monitor (IDM-style transfer window):
  // one compact panel bottom-right streaming live progress for EVERY active
  // download. Can be collapsed to a bubble or hidden completely; the choice
  // persists in localStorage. Reuses the shared WS stores — zero extra
  // connections, zero extra RAM.
  import { fade, fly } from 'svelte/transition'
  import { fmtBytes, fmtSpeed, fmtPct } from './format.js'
  import { active, globalSpeed, floatPanel, setFloatPanel } from './store.js'

  $: visible = $floatPanel === 'show' && $active.length > 0
  $: doneBytes = $active.reduce((s, t) => s + (t.downloaded_bytes ?? 0), 0)
  $: totalBytes = $active.reduce((s, t) => s + (t.total_bytes ?? 0), 0)
</script>

{#if visible}
  <div
    class="fixed bottom-3 right-3 z-30 w-[300px] max-w-[calc(100vw-1.5rem)] overflow-hidden rounded-box border border-base-300 bg-base-100 shadow-xl"
    in:fly={{ y: 24, duration: 200 }}
    out:fade={{ duration: 120 }}
    role="region"
    aria-label="live download progress"
  >
    <!-- header -->
    <div class="flex items-center gap-2 border-b border-base-300 bg-base-200 px-3 py-2">
      <span class="text-primary">⇣</span>
      <span class="text-xs font-semibold uppercase tracking-wider">
        Downloads <span class="opacity-60">({$active.length})</span>
      </span>
      <span class="grow" />
      <span class="font-mono text-xs text-secondary">{fmtSpeed($globalSpeed)}</span>
      <button
        class="btn btn-ghost btn-xs px-1.5"
        title="Collapse"
        aria-label="Collapse download panel"
        on:click={() => setFloatPanel('min')}
      >–</button>
      <button
        class="btn btn-ghost btn-xs px-1.5"
        title="Hide (re-enable from the ⇣ button in the header)"
        aria-label="Hide download panel"
        on:click={() => setFloatPanel('hide')}
      >✕</button>
    </div>

    <!-- per-task rows -->
    <div class="max-h-[40vh] divide-y divide-base-300 overflow-y-auto">
      {#each $active as t (t.id)}
        <div class="px-3 py-2" title="{t.filename} — {t.save_path}">
          <div class="flex items-center gap-2">
            <span class="min-w-0 flex-1 truncate text-xs font-medium">{t.filename}</span>
            <span class="font-mono text-[10px] opacity-60">{fmtPct(t)}%</span>
          </div>
          <progress
            class="progress {t.state === 'error' ? 'progress-error' : t.state === 'paused' ? 'progress-warning' : 'progress-primary'} h-1"
            value={fmtPct(t)}
            max="100"
          />
          <div class="mt-0.5 flex items-center gap-2 font-mono text-[10px] opacity-50">
            <span>{fmtBytes(t.downloaded_bytes)} / {fmtBytes(t.total_bytes)}</span>
            <span class="grow" />
            <span class="text-secondary">{t.state === 'downloading' && t._speed ? fmtSpeed(t._speed) : t.state}</span>
          </div>
        </div>
      {/each}
    </div>

    <!-- footer summary -->
    <div class="flex items-center gap-2 border-t border-base-300 bg-base-200 px-3 py-1.5 font-mono text-[10px] opacity-60">
      <span>{fmtBytes(doneBytes)} / {fmtBytes(totalBytes)}</span>
      <span class="grow" />
      <span>total {fmtSpeed($globalSpeed)}</span>
    </div>
  </div>
{:else if $floatPanel === 'min' && $active.length > 0}
  <!-- collapsed bubble -->
  <button
    class="btn btn-circle btn-sm fixed bottom-3 right-3 z-30 border border-base-300 bg-base-100 font-mono text-xs shadow-xl"
    title="Show download progress"
    on:click={() => setFloatPanel('show')}
    in:fade={{ duration: 120 }}
  >
    ⇣{$active.length}
  </button>
{/if}
