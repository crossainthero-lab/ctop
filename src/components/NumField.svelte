<script lang="ts">
  let { value = $bindable<number | null>(null), step = 0.1, unit = '', width = 84, placeholder = 'not set', onchange }: {
    value?: number | null; step?: number; unit?: string; width?: number; placeholder?: string; onchange?: () => void;
  } = $props();
  function input(e: Event) {
    const t = (e.target as HTMLInputElement).value.trim();
    const n = t === '' ? null : Number(t);
    value = n !== null && Number.isFinite(n) ? n : null;
    onchange?.();
  }
</script>

<span class="nf"><input type="number" {step} {placeholder} value={value ?? ''} oninput={input} style="width:{width}px" class:unset={value === null} />{#if unit}<span class="unit">{unit}</span>{/if}</span>

<style>
  .nf { display: inline-flex; align-items: center; }
  input.unset { background: var(--warn-bg); }
  .unit { color: var(--mute-c); font-size: 11px; margin-left: 4px; }
</style>
