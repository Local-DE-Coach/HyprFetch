<script>
  // GO + Open buttons for downloaded files (desktop integration).
  //   GO   — open the containing folder in the Linux file manager,
  //          WITHOUT launching the file itself.
  //   Open — open the file with the system default application.
  // Hidden until the row is hovered (always visible on touch screens).
  import { openFile, revealFile } from './store.js'

  export let task
  export let show = true   // parent decides when the file exists (state === complete)
</script>

{#if show}
  <span class="inline-flex items-center gap-1 opacity-0 transition-opacity duration-150 focus-within:opacity-100 group-hover:opacity-100 max-sm:opacity-100">
    <button
      class="btn btn-ghost btn-xs gap-0.5 px-2 font-semibold text-success"
      title="Open the folder this file is in (without opening the file)"
      on:click={() => revealFile(task)}
    >GO</button>
    <button
      class="btn btn-ghost btn-xs px-2 font-semibold text-info"
      title="Open the file with its default Linux application"
      on:click={() => openFile(task)}
    >Open</button>
  </span>
{/if}
