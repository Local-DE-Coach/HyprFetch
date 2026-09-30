<script>
  // Updates — one clean flow (v0.5.0 redesign): see the current state at a
  // glance, ONE primary action, and help banners only when something needs
  // attention. Mirrors the `hyprfetch update [--check]` CLI. The old page
  // duplicated the same info across three cards; everything now lives in a
  // single card with a collapsed terminal alternative.
  import { serverInfo, refreshServer } from '../lib/store.js'
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
  let systemFixHint = ''
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
          : updateInfo.error ?? 'update channel unreachable'
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
        msg = 'the update downloaded, but installing it needs your password once'
        return
      }
      staleCopies = res.stale_copies ?? []
      msg = res.migrated
        ? `installed ${res.installed} — moved to ${res.new_path ?? '~/.local/bin'}; every later update installs silently`
        : `installed ${res.installed} — restarting…`
      if (res.system_fix_hint) systemFixHint = res.system_fix_hint
      setTimeout(() => location.reload(), 6000)
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
      authorizePoll = setInterval(async () => {
        try {
          const st = await authorizeStatus()
          if (st.done) {
            clearInterval(authorizePoll)
            authorizePoll = null
            needsPassword = false
            oneClickReady = true
            authorizeMsg = ''
            msg = 'one-click updates enabled — update installed, reloading…'
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
      let note = ''
      if (res.owned?.length) {
        note = ` package-owned: ${res.owned.map((o) => o.path).join(', ')}`
      }
      if (res.failed?.length) {
        note = ` could not remove: ${res.failed.map((f) => f.path).join(', ')}${note}`
      }
      msg = (res.removed?.length ? `removed ${res.removed.length} — ` : '') + res.message + note
      staleCopies = []
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

  // The single headline state the whole page hangs on.
  $: state = updateInfo?.available ? 'available' : updateInfo ? 'latest' : 'idle'
  $: current = $serverInfo.version || '…'
  $: latest = updateInfo?.latest || null
</script>

<section class="card border border-base-300 bg-base-200 shadow-sm mx-auto max-w-2xl">
  <div class="card-body gap-4 p-6">
    <!-- hero: current → latest -->
    <div class="flex items-center justify-between gap-3">
      <h2 class="card-title text-base">Software updates</h2>
      {#if oneClickReady}
        <span class="badge badge-sm badge-success" title="in-app updates run without any password prompt">one-click ✓</span>
      {/if}
    </div>

    <div class="flex items-center justify-center gap-4 py-2">
      <div class="text-center">
        <div class="text-[11px] uppercase tracking-wider opacity-50">installed</div>
        <div class="font-mono text-3xl font-bold text-primary">v{current}</div>
      </div>
      <div class="text-2xl opacity-40">{state === 'available' ? '→' : '·'}</div>
      <div class="text-center">
        <div class="text-[11px] uppercase tracking-wider opacity-50">
          {state === 'available' ? 'available' : 'latest'}
        </div>
        <div class="font-mono text-3xl font-bold {state === 'available' ? 'text-success' : 'opacity-40'}">
          {latest ? `v${latest}` : '—'}
        </div>
      </div>
    </div>

    <!-- the ONE primary row -->
    <div class="flex flex-wrap items-center justify-center gap-2">
      {#if state === 'available'}
        <button class="btn btn-primary" disabled={busy} on:click={doApply}>
          Install v{latest} &amp; restart
        </button>
      {:else}
        <button class="btn btn-primary" disabled={busy} on:click={doCheck}>
          {busy ? 'Checking…' : updateInfo ? 'Check again' : 'Check for updates'}
        </button>
      {/if}
      <button class="btn btn-ghost btn-sm" disabled={busy} on:click={doRestart}>Restart</button>
      {#if updateInfo?.release_url}
        <a class="btn btn-ghost btn-sm" href={updateInfo.release_url} target="_blank" rel="noreferrer">Notes ↗</a>
      {/if}
    </div>

    {#if msg}
      <p class="text-center text-xs opacity-70">{msg}</p>
    {/if}

    <!-- help banners — only when something actually needs attention -->
    {#if needsPassword}
      <div class="alert py-3 px-4 text-xs" role="alert">
        <div class="min-w-0">
          <div class="font-semibold">One password, then updates are automatic forever</div>
          <p class="mt-1 opacity-80">
            HyprFetch lives in a system location, so this one install needs
            root. Click below — a terminal window opens, type your password
            there once. Every future update then installs silently.
          </p>
          <div class="mt-2 flex flex-wrap items-center gap-2">
            <button class="btn btn-primary btn-xs" disabled={authorizing} on:click={doAuthorize}>
              {authorizing ? 'waiting for the terminal…' : 'Enable one-click updates'}
            </button>
            <span class="opacity-60 font-mono">or run: sudo hyprfetch update</span>
          </div>
          {#if authorizeMsg}<p class="mt-1 opacity-80">{authorizeMsg}</p>{/if}
        </div>
      </div>
    {/if}

    {#if systemFixHint}
      <div class="alert py-3 px-4 text-xs" role="alert">
        <div class="min-w-0">
          <div class="font-semibold">One step left: an old system copy could not be relinked</div>
          <p class="mt-1 opacity-80">Run this once in a terminal — afterwards every update is silent:</p>
          <div class="mt-1 font-mono break-all select-all opacity-90">{systemFixHint}</div>
        </div>
      </div>
    {/if}

    {#if staleCopies.length}
      <div class="alert py-3 px-4 text-xs" role="alert">
        <div class="min-w-0">
          <div class="font-semibold">
            {staleCopies.length} old HyprFetch {staleCopies.length === 1 ? 'copy' : 'copies'} on PATH
            {#if staleCopies.some((c) => c.shadows)}— one shadows this install{/if}
          </div>
          {#each staleCopies as c (c.path)}
            <div class="opacity-80 truncate font-mono">
              {c.shadows ? 'shadows: ' : 'duplicate: '}{c.path}{c.version ? ` (${c.version})` : ''}
            </div>
          {/each}
          <button class="btn btn-warning btn-xs mt-2" disabled={busy} on:click={doFixStale}>
            Remove stale copies
          </button>
        </div>
      </div>
    {/if}

    <!-- terminal alternative, collapsed -->
    <details class="collapse collapse-arrow rounded-box border border-base-300 bg-base-100/50">
      <summary class="collapse-title py-2 text-xs font-medium opacity-70">Prefer the terminal?</summary>
      <div class="collapse-content text-xs">
        <div class="mockup-code bg-base-300 text-xs">
          <pre data-prefix="$"><code>hyprfetch update --check</code></pre>
          <pre data-prefix="$"><code>hyprfetch update</code></pre>
        </div>
        <p class="mt-2 opacity-50">
          Source checkout? <code class="font-mono">hyprfetch update --from-git --source-dir ~/HyprFetch</code>
        </p>
      </div>
    </details>

    <p class="text-center text-[11px] opacity-40">
      sha256-verified archives from the project's own server — no GitHub, no tokens
    </p>
  </div>
</section>
