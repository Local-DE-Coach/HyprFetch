<script>
  // Updates — current version, latest release, one-click install (download →
  // sha256 verify → atomic swap → restart) plus plain restart. Mirrors the
  // `hyprfetch update [--check]` CLI.
  import { serverInfo, settings, refreshServer, notify } from '../lib/store.js'
  import { fmtUptime } from '../lib/format.js'
  import { checkUpdate, applyUpdate, restartServer } from '../api.js'

  let updateInfo = null
  let busy = false
  let msg = ''

  $: tokenSet = $settings?.github_token_set === 'true'

  async function doCheck() {
    busy = true
    msg = ''
    try {
      updateInfo = await checkUpdate()
      msg = updateInfo.available
        ? `version ${updateInfo.latest} is available`
        : updateInfo.latest
          ? 'you are on the latest release'
          : updateInfo.error ?? 'no release information'
    } catch (e) {
      msg = `check failed: ${e.message} — set a GitHub token in Settings if this is a private-repo auth error`
    } finally {
      busy = false
    }
  }

  async function doApply() {
    busy = true
    msg = 'installing… the server will restart and auto-resume downloads'
    try {
      const res = await applyUpdate(true)
      msg = `installed ${res.installed} — restarting… page reloads in a few seconds`
      setTimeout(() => location.reload(), 4000)
      await refreshServer()
    } catch (e) {
      msg = `install failed: ${e.message}`
    } finally {
      busy = false
    }
  }

  async function doRestart() {
    busy = true
    msg = 'restarting…'
    try {
      await restartServer()
      msg = 'restarting — reconnecting in a few seconds'
      setTimeout(() => location.reload(), 4000)
    } catch (e) {
      msg = `restart failed: ${e.message}`
    } finally {
      busy = false
    }
  }

  $: versionBadge = (v) => (v ? `v${v}` : '—')
</script>

<div class="grid gap-5 lg:grid-cols-2">
  <!-- Current -->
  <section class="card border border-base-300 bg-base-200 shadow-sm">
    <div class="card-body gap-3 p-5">
      <h2 class="card-title text-base">This server</h2>
      <div class="flex items-baseline gap-3">
        <span class="font-mono text-3xl font-bold text-primary">v{$serverInfo.version || '…'}</span>
        <span class="text-xs opacity-50">up {fmtUptime($serverInfo.uptime_secs)}</span>
      </div>
      <ul class="text-sm opacity-70">
        <li>• {$serverInfo.active_tasks ?? 0} active task(s), {$serverInfo.ws_clients ?? 0} UI client(s) connected</li>
        <li>• updates are checked via the istias.tech update channel first (fast, no GitHub), then GitHub releases</li>
        <li>• downloaded archives are sha256-verified and swapped atomically</li>
        <li>• active downloads are paused and auto-resumed after the restart</li>
      </ul>
    </div>
  </section>

  <!-- Check + install -->
  <section class="card border border-base-300 bg-base-200 shadow-sm">
    <div class="card-body gap-3 p-5">
      <h2 class="card-title text-base">Check for updates</h2>
      <div class="flex flex-wrap items-center gap-3">
        <span class="badge badge-lg badge-outline">installed: {versionBadge($serverInfo.version)}</span>
        {#if updateInfo}
          <span class="badge badge-lg {updateInfo.available ? 'badge-success' : 'badge-ghost'}">
            latest: {versionBadge(updateInfo.latest)}
          </span>
          {#if updateInfo.via_channel}
            <span class="badge badge-lg badge-ghost">via update channel ↯</span>
          {/if}
          {#if updateInfo.available}
            <span class="badge badge-lg badge-primary animate-pulse">update available</span>
          {/if}
        {/if}
      </div>
      <div class="flex flex-wrap gap-2">
        <button class="btn btn-sm" disabled={busy} on:click={doCheck}>Check now</button>
        {#if updateInfo?.available}
          <button class="btn btn-primary btn-sm" disabled={busy} on:click={doApply}>
            Install {updateInfo.latest} &amp; restart
          </button>
        {/if}
        <button class="btn btn-ghost btn-sm" disabled={busy} on:click={doRestart}>Restart server</button>
        {#if updateInfo?.release_url}
          <a class="btn btn-ghost btn-sm" href={updateInfo.release_url} target="_blank" rel="noreferrer">Release notes ↗</a>
        {/if}
      </div>
      {#if msg}<p class="text-xs opacity-70">{msg}</p>{/if}
      {#if !tokenSet}
        <p class="text-xs text-warning">
          No token stored in settings — the updater will still try env vars and your
          local clone's origin URL. If checks fail, paste a PAT in Settings → GitHub token.
        </p>
      {/if}
    </div>
  </section>

  <!-- CLI equivalent -->
  <section class="card border border-base-300 bg-base-200 shadow-sm lg:col-span-2">
    <div class="card-body gap-2 p-5">
      <h2 class="card-title text-base">Prefer the terminal?</h2>
      <p class="text-xs opacity-60">The same flow works from the command line:</p>
      <div class="mockup-code text-xs">
        <pre data-prefix="$"><code>hyprfetch update --check   # report only</code></pre>
        <pre data-prefix="$"><code>hyprfetch update           # download → sha256 verify → atomic swap → restart</code></pre>
      </div>
      <p class="text-xs opacity-50">
        Installed from a source clone? <code class="font-mono">hyprfetch update --from-git --source-dir ~/HyprFetch</code>
        pulls and rebuilds instead of downloading a release tarball.
      </p>
    </div>
  </section>
</div>
