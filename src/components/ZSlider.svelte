<script lang="ts">
  // Vertical pen-height stick. Push up to raise, down to lower; it springs back to the middle on release. Like the joystick it asks
  // for one bounded step per tick, never queues, and stops at once on release, blur, hidden tab or leaving the page.
  import { zStepFromStick, DEFAULT_Z } from '../lib/joystick';

  let { z, contact, disabled = false, onstep, onrelease }: {
    z: number | null; contact: number; disabled?: boolean; onstep: (dz: number) => void; onrelease?: () => void;
  } = $props();
  const H = 260, TRACK = H - 56;
  let el: HTMLDivElement | undefined = $state();
  let knob = $state(0); // -1..1, up positive
  let held = false, v = 0;
  let timer: ReturnType<typeof setInterval> | undefined;

  function release() {
    if (!held && !timer) return;
    held = false; v = 0; knob = 0; clearInterval(timer); timer = undefined; onrelease?.();
  }
  function tick() {
    if (!held || disabled || z === null) return;
    const dz = zStepFromStick(v, z, contact, DEFAULT_Z);
    if (dz) onstep(dz);
  }
  function move(e: PointerEvent) {
    if (!held || !el) return;
    const b = el.getBoundingClientRect();
    v = Math.max(-1, Math.min(1, -((e.clientY - (b.top + b.height / 2)) / ((TRACK / 2) * (b.height / H)))));
    knob = v;
  }
  function down(e: PointerEvent) {
    if (disabled) return;
    el?.setPointerCapture(e.pointerId); held = true; move(e);
    clearInterval(timer); timer = setInterval(tick, DEFAULT_Z.tickMs); tick();
  }
  $effect(() => {
    const stop = () => release(), vis = () => { if (document.hidden) release(); };
    window.addEventListener('blur', stop); window.addEventListener('pagehide', stop); document.addEventListener('visibilitychange', vis);
    return () => { window.removeEventListener('blur', stop); window.removeEventListener('pagehide', stop); document.removeEventListener('visibilitychange', vis); release(); };
  });
  $effect(() => { if (disabled) release(); });
</script>

<div class="z" class:off={disabled}>
  <div class="lab top">UP<br /><span>raises the pen</span></div>
  <div class="track" bind:this={el} role="application" aria-label="Pen height: push up to raise, down to lower" tabindex="-1"
       style="height:{H}px" onpointerdown={down} onpointermove={move} onpointerup={release} onpointercancel={release} onlostpointercapture={release}>
    <div class="rail"></div>
    <div class="mid"></div>
    <div class="knob" class:on={knob !== 0} style="top:{H / 2 - 22 - knob * (TRACK / 2)}px"></div>
  </div>
  <div class="lab bot">DOWN<br /><span>toward the paper</span></div>
</div>

<style>
  .z { display: grid; justify-items: center; gap: 4px; width: 86px; }
  .off { opacity: .45; }
  .track { position: relative; width: 70px; touch-action: none; user-select: none; cursor: grab; }
  .rail { position: absolute; left: 31px; top: 6px; bottom: 6px; width: 8px; border-radius: 4px; background: var(--soft); border: 1px solid var(--line); }
  .mid { position: absolute; left: 12px; right: 12px; top: 50%; border-top: 1.5px dashed var(--line); }
  .knob { position: absolute; left: 13px; width: 44px; height: 44px; border-radius: 50%; background: var(--surface); border: 2.5px solid var(--ink); }
  .knob.on { background: var(--accent); border-color: var(--accent); }
  .lab { font-size: 11px; letter-spacing: .08em; color: var(--mute); text-align: center; line-height: 1.2; } .lab span { letter-spacing: 0; font-size: 10px; }
</style>
