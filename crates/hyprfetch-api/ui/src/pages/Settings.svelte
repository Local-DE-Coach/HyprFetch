<script>
  // Settings — save folders (base + per-category overrides), auto-sort,
  // concurrency, QoS bandwidth cap, security and the GitHub token that
  // powers the in-app updater.
  import { onMount } from 'svelte'
  import { fmtBytes } from '../lib/format.js'
  import { settings, categories, saveSettings, getQos, setQos, notify } from '../lib/store.js'

  // ---- save folders ----
  let baseDir = ''
  let categorize = true
  let catDirs = {}          // name -> dir string
  let loadedBase = ''
  let savingFolders = false

  $: if ($categories) {
    if ($categories.base && !loadedBase) {
      baseDir = $categories.base
      loadedBase = $categories.base
    }
    if ($categories.categorize !== undefined && categorize !== $categories.categorize && firstCatSync) {
      categorize = $categories.categorize
    }
  }
  let firstCatSync = true

  $: if ($categories?.categories) {
    const next = {}
    for (const c of $categories.categories) next[c.name] = c.dir
    if (!Object.keys(catDirs).length) catDirs = next
  }

  async function saveFolders() {
    savingFolders = true
    try {
      const patch = {}
      if (baseDir.trim() && baseDir.trim() !== loadedBase) patch.download_dir = baseDir.trim()
      if (patch.download_dir) patch.categorize = categorize ? 'true' : 'false'
      else if (categorize !== $categories.categorize) patch.categorize = categorize ? 'true' : 'false'
      for (const c of $categories.categories) {
        if (catDirs[c.name] !== c.dir) patch[`category_dir_${c.name}`] = catDirs[c.name]
      }
      if (!Object.keys(patch).length) { notify('no folder changes to save'); return }
      await saveSettings(patch)
      loadedBase = ''
      catDirs = {}
      firstCatSync = false
      notify('folders saved ✓ — directory layout rebuilt')
    } catch (e) {
      notify(`save failed: ${e.message}`)
    } finally {
      savingFolders = false
    }
  }

  // ---- concurrency ----
  let segmentsDefault = 8
  let maxConcurrent = 3
  let savingQueue = false
  $: if ($settings?.segments_default && segmentsDefault === 8) {
    segmentsDefault = parseInt($settings.segments_default) || 8
  }
  $: if ($settings?.max_concurrent_tasks && maxConcurrent === 3) {
    maxConcurrent = parseInt($settings.max_concurrent_tasks) || 3
  }
  async function saveQueue() {
    savingQueue = true
    try {
      await saveSettings({
        segments_default: String(segmentsDefault),
        max_concurrent_tasks: String(maxConcurrent),
      })
      notify('queue settings saved ✓')
    } catch (e) {
      notify(`save failed: ${e.message}`)
    } finally {
      savingQueue = false
    }
  }

  // ---- QoS ----
  let qosEnabled = false
  let qosMbps = 5
  let qosSaving = false
  onMount(async () => {
    try {
      const q = await getQos()
      qosEnabled = q.enabled
      if (q.target_bps) qosMbps = Math.round((q.target_bps / 1_000_000) * 100) / 100
    } catch (_) { /* defaults */ }
  })
  async function saveQos() {
    qosSaving = true
    try {
      const bps = qosEnabled ? Math.max(1, Math.round(qosMbps * 1_000_000)) : 0
      await setQos(qosEnabled, bps)
      notify(qosEnabled ? `QoS on — capped at ${fmtBytes(bps)}/s` : 'QoS off — full speed')
    } catch (e) {
      notify(`error: ${e.message}`)
    } finally {
      qosSaving = false
    }
  }

  // ---- security ----
  let ssrfBlock = true
  let savingSec = false
  $: if ($settings?.ssrf_block_private !== undefined && ssrfBlock) {
    ssrfBlock = $settings.ssrf_block_private !== 'false'
  }
  async function saveSecurity() {
    savingSec = true
    try {
      await saveSettings({ ssrf_block_private: ssrfBlock ? 'true' : 'false' })
      notify('security setting saved ✓')
    } catch (e) {
      notify(`save failed: ${e.message}`)
    } finally {
      savingSec = false
    }
  }

  // ---- GitHub token (updater) ----
  let ghToken = ''
  let savingToken = false
  $: tokenSet = $settings?.github_token_set === 'true'
  async function saveToken() {
    if (!ghToken.trim()) { notify('paste a token first (or leave to keep current)'); return }
    savingToken = true
    try {
      await saveSettings({ github_token: ghToken.trim() })
      ghToken = ''
      notify('GitHub token saved ✓ — updates will use it')
    } catch (e) {
      notify(`save failed: ${e.message}`)
    } finally {
      savingToken = false
    }
  }
</script>

<div class="grid gap-5 lg:grid-cols-2">
  <!-- Save folders -->
  <section class="card border border-base-300 bg-base-200 shadow-sm lg:col-span-2">
    <div class="card-body gap-3 p-5">
      <h2 class="card-title text-base">Save folders</h2>
      <p class="text-xs opacity-60">
        Downloads are sorted by file type into these folders. Folders are created
        automatically — here or on disk — the moment you save. Leave a category
        folder unchanged to use the default <code class="font-mono">&lt;base&gt;/&lt;category&gt;</code>.
      </p>

      <label class="grid max-w-xl gap-1.5 text-sm">
        <span>Base download folder <span class="opacity-50">(main location)</span></span>
        <input type="text" class="input input-bordered font-mono" bind:value={baseDir}
          placeholder="~/Downloads" />
      </label>

      <label class="flex w-fit cursor-pointer items-center gap-2 text-sm">
        <input type="checkbox" class="toggle toggle-primary" bind:checked={categorize} />
        Auto-sort downloads by type {categorize ? '(on)' : '(off)'}
      </label>

      <div class="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
        {#each $categories.categories as c (c.name)}
          <label class="grid gap-1 text-xs">
            <span class="capitalize opacity-70">{c.name}</span>
            <input type="text" class="input input-bordered input-sm font-mono"
              bind:value={catDirs[c.name]} />
          </label>
        {/each}
      </div>

      <div class="card-actions justify-end">
        <button class="btn btn-primary btn-sm" disabled={savingFolders} on:click={saveFolders}>
          {savingFolders ? 'Saving…' : 'Save folders'}
        </button>
      </div>
    </div>
  </section>

  <!-- Queue -->
  <section class="card border border-base-300 bg-base-200 shadow-sm">
    <div class="card-body gap-3 p-5">
      <h2 class="card-title text-base">Queue</h2>
      <label class="grid gap-1.5 text-sm">
        <span>Default segments per download <span class="opacity-50">(1–32)</span></span>
        <input type="number" min="1" max="32" class="input input-bordered w-28" bind:value={segmentsDefault} />
      </label>
      <label class="grid gap-1.5 text-sm">
        <span>Max simultaneous downloads</span>
        <input type="number" min="1" max="16" class="input input-bordered w-28" bind:value={maxConcurrent} />
      </label>
      <div class="card-actions justify-end">
        <button class="btn btn-primary btn-sm" disabled={savingQueue} on:click={saveQueue}>
          {savingQueue ? 'Saving…' : 'Save queue'}
        </button>
      </div>
    </div>
  </section>

  <!-- QoS + security -->
  <section class="card border border-base-300 bg-base-200 shadow-sm">
    <div class="card-body gap-3 p-5">
      <h2 class="card-title text-base">Bandwidth &amp; security</h2>
      <div class="flex flex-wrap items-center gap-4">
        <label class="flex cursor-pointer items-center gap-2 text-sm">
          <input type="checkbox" class="toggle toggle-primary" bind:checked={qosEnabled} on:change={saveQos} disabled={qosSaving} />
          QoS cap {qosEnabled ? 'on' : 'off'}
        </label>
        <label class="flex items-center gap-2 text-sm">
          <input type="number" min="0.1" step="0.5" class="input input-bordered input-sm w-24"
            bind:value={qosMbps} on:change={saveQos} disabled={!qosEnabled || qosSaving} />
          MiB/s <span class="opacity-50">(total, all tasks)</span>
        </label>
      </div>
      <label class="flex w-fit cursor-pointer items-center gap-2 text-sm">
        <input type="checkbox" class="toggle toggle-warning" bind:checked={ssrfBlock} />
        Block private/loopback download URLs (SSRF protection)
      </label>
      <div class="card-actions justify-end">
        <button class="btn btn-primary btn-sm" disabled={savingSec} on:click={saveSecurity}>
          {savingSec ? 'Saving…' : 'Save security'}
        </button>
      </div>
    </div>
  </section>

  <!-- GitHub token -->
  <section class="card border border-base-300 bg-base-200 shadow-sm lg:col-span-2">
    <div class="card-body gap-3 p-5">
      <h2 class="card-title text-base">GitHub token <span class="badge badge-sm badge-ghost">updates</span></h2>
      <p class="text-xs opacity-60">
        HyprFetch is installed from a private repo. Updates already work if you
        cloned with your PAT in the URL — the token is picked up from the clone.
        You can also paste a fine-grained PAT here (stored locally in the app
        database, never displayed again). Tokens set via
        <code class="font-mono">HYPRFETCH_GITHUB_TOKEN</code> or the config file take precedence.
      </p>
      <div class="flex flex-wrap items-end gap-3">
        <label class="grid gap-1.5 text-sm">
          <span>
            Status:
            {#if tokenSet}<span class="text-success">token configured</span>
            {:else}<span class="opacity-60">no token in settings (env/clone may cover it)</span>{/if}
          </span>
          <input type="password" class="input input-bordered w-80 font-mono" bind:value={ghToken}
            placeholder="github_pat_…" autocomplete="off" />
        </label>
        <button class="btn btn-primary btn-sm" disabled={savingToken} on:click={saveToken}>
          {savingToken ? 'Saving…' : 'Save token'}
        </button>
      </div>
    </div>
  </section>
</div>
