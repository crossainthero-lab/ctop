<script lang="ts">
  import { dlg } from '../lib/dialogs.svelte';
  let ticks = $state<boolean[]>([]);
  $effect(() => { ticks = (dlg.spec?.checks ?? []).map(() => false); });
  const ok = $derived(ticks.every(Boolean));
  function key(e: KeyboardEvent) { if (e.key === 'Escape') dlg.spec?.resolve(false); }
</script>

<svelte:window onkeydown={key} />
{#if dlg.spec}
  <div class="scrim" role="presentation">
    <div class="modal" role="dialog" aria-modal="true" class:danger={dlg.spec.danger}>
      <h2>{dlg.spec.title}</h2>
      {#if dlg.spec.body}<p class="body">{dlg.spec.body}</p>{/if}
      {#if dlg.spec.checks?.length}
        <div class="checks">
          {#each dlg.spec.checks as c, i}
            <label><input type="checkbox" bind:checked={ticks[i]} /> <span>{c}</span></label>
          {/each}
        </div>
      {/if}
      <div class="row end">
        <button class="btn" onclick={() => dlg.spec?.resolve(false)}>{dlg.spec.cancelLabel ?? 'Cancel'}</button>
        <button class="btn" class:danger={dlg.spec.danger} class:primary={!dlg.spec.danger} disabled={!ok} onclick={() => dlg.spec?.resolve(true)}>{dlg.spec.confirmLabel ?? 'Confirm'}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .scrim { position: fixed; inset: 0; background: rgba(27,27,25,.55); display: grid; place-items: center; z-index: 100; }
  .modal { background: var(--paper); border: 1px solid var(--ink); width: min(520px, 92vw); padding: 18px 20px; display: flex; flex-direction: column; gap: 12px; box-shadow: 0 18px 50px rgba(0,0,0,.35); }
  .modal.danger { border-top: 5px solid var(--danger); }
  .body { margin: 0; white-space: pre-line; color: var(--graphite); }
  .checks { display: flex; flex-direction: column; gap: 8px; border: 1px solid var(--line); padding: 10px; background: var(--panel); }
  .checks label { display: flex; gap: 8px; align-items: flex-start; cursor: pointer; }
  .checks input { margin-top: 2px; }
  .end { justify-content: flex-end; }
</style>
