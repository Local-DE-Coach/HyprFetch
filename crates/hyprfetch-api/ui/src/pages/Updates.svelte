<script>
  // Updates — current version, latest release, one-click install (download →
  // sha256 verify → atomic swap → restart) plus plain restart. Mirrors the
  // `hyprfetch update [--check]` CLI. Also surfaces stale shadowing copies
  // (old install.sh builds on PATH that keep launching the previous
  // version after an update) with a one-click fix.
  import { serverInfo, settings, refreshServer, notify } from '../lib/store.js'
  import { fmtUptime } from '../lib/format.js'
  import {
    checkUpdate,
    applyUpdate,
    restartServer,
    fixStaleCopies,
    authorizeUpdate,
    authorizeStatus,
  } from '../api.js'

  let updateInfo = null
  let staleCopies = []
  let busy = false
  let msg = ''

  // needs_password: the update downloaded fine but the system location
  // needs the user's password once (no passwordless route answered).
  let needsPassword = false
  let oneClickReady = false
  let authorizing = false
  let authorizeMsg = ''
  let authorizePoll = null

  async function doCheck() {
    busy = true
    msg = ''
    try {
      updateInfo = await checkUpdate()
      staleCopies = updateInfo.stale_copies ?? []
      oneClickReady = !!updateInfo.one_click_ready
      msg = updateInfo.available
        ? `version ${updateInfo.latest} is available`
        : updateInfo.latest
          ? 'you are on the latest release'
          : updateInfo.error ?? 'update channel unreachable — see https://istias.tech/hyprfetch/updates'
    } catch (e) {
      msg = `check failed: ${e.message}`
    } finally {
      busy = false
    }
  }

  async function doApply() {
    busy = true
    msg = 'installing… the server will restart and auto-resume downloads'
    try {
      const res = await applyUpdate(true)
      if (res.needs_password) {
        needsPassword = true
        oneClickReady = false
        msg = `the update downloaded, but installing it in a system location needs your password once`
        return
      }
      staleCopies = res.stale_copies ?? []
      msg = `installed ${res.installed} — restarting… page reloads in a few seconds`
      setTimeout(() => location.reload(), 4000)
      await refreshServer()
    } catch (e) {
      msg = `install failed: ${e.message}`
    } finally {
      busy = false
    }
  }

  // The one-time setup: opens a terminal window; the user types their
  // password ONCE; the helper + sudoers rule land, the pending update
  // finishes and the server restarts itself. Every later update is silent.
  async function doAuthorize() {
    authorizing = true
    authorizeMsg = 'opening a terminal window…'
    try {
      const res = await authorizeUpdate()
      if (!res.spawned) {
        authorizeMsg = `could not open a terminal: ${res.error}. Manual way: ${res.manual_command}`
        return
      }
      authorizeMsg = res.message
      // Poll while the terminal setup runs; the daemon restarts itself when
      // the swap lands — reload shortly after.
      authorizePoll = setInterval(async () => {
        try {
          const st = await authorizeStatus()
          if (st.done) {
            clearInterval(authorizePoll)
            authorizePoll = null
            needsPassword = false
            oneClickReady = true
            authorizeMsg = 'one-click updates enabled ✔'
            msg = 'update installed — reloading…'
            setTimeout(() => location.reload(), 4000)
          } else if (st.error) {
            clearInterval(authorizePoll)
            authorizePoll = null
            authorizing = false
            authorizeMsg = st.error
          }
        } catch (_) {
          /* daemon restarting — keep polling */
        }
      }, 2000)
    } catch (e) {
      authorizeMsg = `setup failed: ${e.message}`
    } finally {
      authorizing = false
    }
  }

  async function doFixStale() {
    busy = true
    msg = 'removing stale copies…'
    try {
      const res = await fixStaleCopies()
      msg = res.message
      let note = ''
      if (res.owned?.length) {
        note = ` package-owned copies must go via the package manager: ${res.owned.map((o) => o.path).join(', ')}`
      }
      if (res.failed?.length) {
        note = ` could not remove: ${res.failed.map((f) => f.path).join(', ')}${note}`
      }
      msg = (res.removed?.length ? `removed: ${res.removed.join(', ')}. ` : '') + res.message + note
      await refreshServer()
    } catch (e) {
      msg = `fix failed: ${e.message}`
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
        <li>• updates come ONLY from the istias.tech update channel — GitHub is never contacted</li>
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
        {#if oneClickReady}
          <span class="badge badge-lg badge-success" title="in-app updates run without any password prompt">one-click updates ✓</span>
        {/if}
        {#if updateInfo}
          <span class="badge badge-lg {updateInfo.available ? 'badge-success' : 'badge-ghost'}">
            latest: {versionBadge(updateInfo.latest)}
          </span>
          {#if updateInfo.channel}
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

      {#if needsPassword}
        <div class="alert alert-info py-2 px-3 text-xs" role="alert">
          <div class="min-w-0">
            <div class="font-semibold">
              🔐 One password, then updates are automatic forever
            </div>
            <p class="opacity-80 mt-1">
              HyprFetch lives in a system location, so installing the update
              needs root ONCE. Other apps do this through a system rule —
              click below and a terminal window opens: type your password
              there one time. Every future in-app update then installs
              silently (this is exactly how GUI package managers work).
            </p>
            <div class="mt-1 flex flex-wrap items-center gap-2">
              <button class="btn btn-xs btn-primary" disabled={authorizing} on:click={doAuthorize}>
                {authorizing ? 'waiting for the terminal…' : 'Enable one-click updates (password once)'}
              </button>
              <span class="opacity-60 font-mono">or run: sudo hyprfetch update</span>
            </div>
            {#if authorizeMsg}<p class="mt-1 opacity-80">{authorizeMsg}</p>{/if}
          </div>
        </div>
      {/if}

      {#if staleCopies.length}
        <div class="alert alert-warning py-2 px-3 text-xs" role="alert">
          <div class="min-w-0">
            <div class="font-semibold">
              ⚠ {staleCopies.length} other HyprFetch {staleCopies.length === 1 ? 'copy' : 'copies'} on PATH
              {#if staleCopies.some((c) => c.shadows)}— one SHADOWS this install, so the old version keeps launching{/if}
            </div>
            {#each staleCopies as c (c.path)}
              <div class="opacity-80 truncate font-mono">
                {c.shadows ? 'shadows: ' : 'duplicate: '}{c.path}{c.version ? ` (${c.version})` : ''}{c.owned_by ? ` — package ${c.owned_by}` : ''}
              </div>
            {/each}
            <div class="mt-1 flex flex-wrap items-center gap-2">
              <button class="btn btn-xs btn-warning" disabled={busy} on:click={doFixStale}>
                Remove stale copies
              </button>
              <span class="opacity-60">package-owned copies must be removed via the package manager</span>
            </div>
          </div>
        </div>
      {/if}

      <p class="text-xs opacity-60">
        Updates are served by the project's own server (istias.tech) as
        sha256-verified archives — no GitHub account or token needed, ever.
        Slow or flaky networks are fine: downloads stream with no time limit
        and resume automatically. System installs are handled by the
        one-click update rule (enable it above once) or the desktop pkexec
        prompt; the banner above appears when an old copy would otherwise
        keep launching the previous version.
      </p>
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
