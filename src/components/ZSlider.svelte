<script lang="ts">
  // Vertical height slider: top = pen up (bigger Z), bottom = pen down (smaller Z). The thumb shows where the pen REALLY is
  // (the controller's reading) until you drag it; while you drag, the pen follows the thumb in short steps, and it stops the
  // moment you let go.
  let { z, min, max, disabled = false, onstep }: { z: number | null; min: number; max: number; disabled?: boolean; onstep: (dz: number) => void } = $props();
  let target = $state<number | null>(null);
  let timer: ReturnType<typeof setInterval> | undefined;
  const shown = $derived(target ?? z ?? min);

  function stop() { target = null; clearInterval(timer); timer = undefined; }
  function loop() {
    if (target === null || z === null || disabled) return;
    const dz = Math.max(-1, Math.min(1, target - z));
    if (Math.abs(dz) >= 0.02) onstep(dz);
  }
  function oninput(e: Event) {
    if (disabled) return;
    target = +(e.currentTarget as HTMLInputElement).value;
    if (!timer) { timer = setInterval(loop, 160); loop(); }
  }
  $effect(() => {
    const out = () => stop(), vis = () => { if (document.hidden) stop(); };
    window.addEventListener('blur', out); window.addEventListener('pagehide', out); window.addEventListener('pointerup', out); window.addEventListener('keyup', out); document.addEventListener('visibilitychange', vis);
    return () => { window.removeEventListener('blur', out); window.removeEventListener('pagehide', out); window.removeEventListener('pointerup', out); window.removeEventListener('keyup', out); document.removeEventListener('visibilitychange', vis); stop(); };
  });
  $effect(() => { if (disabled) stop(); });
</script>

<div class="z">
  <span class="end">UP</span>
  <input type="range" {min} {max} step="0.05" value={shown} {disabled} {oninput} onchange={stop} aria-label="Pen height: drag up to raise, down to lower" />
  <span class="end">DOWN</span>
</div>

<style>
  .z { display: grid; justify-items: center; gap: 6px; }
  .end { font-size: 11px; letter-spacing: .08em; color: var(--mute); }
  input[type='range'] { writing-mode: vertical-lr; direction: rtl; width: 44px; height: 260px; margin: 0; padding: 0; accent-color: var(--accent); cursor: grab; }
</style>
