<script lang="ts">
  // A round stick. While it is held away from the centre it asks for one bounded step every tick; letting go, losing focus,
  // hiding the tab or leaving the page stops asking at once. It never queues anything: the owner decides whether a step is
  // already in flight and simply refuses the next one.
  import { stepFromStick, deflection, type JoyOpts } from '../lib/joystick';

  let { opts, disabled = false, onstep, onrelease }: {
    opts: JoyOpts; disabled?: boolean;
    onstep: (dx: number, dy: number) => void;
    onrelease?: () => void;
  } = $props();

  const SIZE = 260, R = SIZE / 2 - 28;
  let el: HTMLDivElement | undefined = $state();
  let knob = $state({ x: 0, y: 0 });
  let held = false;
  let defl = { vx: 0, vy: 0 };
  let timer: ReturnType<typeof setInterval> | undefined;

  function release() {
    if (!held && !timer) return;
    held = false; defl = { vx: 0, vy: 0 }; knob = { x: 0, y: 0 };
    clearInterval(timer); timer = undefined;
    onrelease?.();
  }
  function tick() {
    if (!held || disabled) return;
    const s = stepFromStick(defl.vx, defl.vy, opts);
    if (s) onstep(s.dx, s.dy);
  }
  function move(e: PointerEvent) {
    if (!held || !el) return;
    const b = el.getBoundingClientRect();
    const d = deflection(e.clientX, e.clientY, b.left + b.width / 2, b.top + b.height / 2, R * (b.width / SIZE));
    defl = d; knob = { x: d.vx * R, y: d.vy * R };
  }
  function down(e: PointerEvent) {
    if (disabled) return;
    el?.setPointerCapture(e.pointerId);
    held = true; move(e);
    clearInterval(timer); timer = setInterval(tick, opts.tickMs);
    tick();
  }
  $effect(() => {
    const stop = () => release();
    const vis = () => { if (document.hidden) release(); };
    window.addEventListener('blur', stop); window.addEventListener('pagehide', stop); document.addEventListener('visibilitychange', vis);
    return () => { window.removeEventListener('blur', stop); window.removeEventListener('pagehide', stop); document.removeEventListener('visibilitychange', vis); release(); };
  });
  $effect(() => { if (disabled) release(); });
</script>

<div class="wrap" class:off={disabled}>
  <div class="stick" bind:this={el} role="application" aria-label="Joystick: drag to move the pen" tabindex="-1"
       style="width:{SIZE}px;height:{SIZE}px" onpointerdown={down} onpointermove={move} onpointerup={release} onpointercancel={release} onlostpointercapture={release}>
    <svg viewBox="0 0 {SIZE} {SIZE}" width={SIZE} height={SIZE} aria-hidden="true">
      <circle cx={SIZE / 2} cy={SIZE / 2} r={SIZE / 2 - 3} class="base" />
      <circle cx={SIZE / 2} cy={SIZE / 2} r={R} class="ring" />
      <line x1={SIZE / 2} y1="18" x2={SIZE / 2} y2={SIZE - 18} class="axis" /><line x1="18" y1={SIZE / 2} x2={SIZE - 18} y2={SIZE / 2} class="axis" />
      <text x={SIZE / 2} y="15" text-anchor="middle" class="lab">BACK</text>
      <text x={SIZE / 2} y={SIZE - 6} text-anchor="middle" class="lab">FRONT</text>
      <text x="8" y={SIZE / 2 + 4} class="lab">LEFT</text>
      <text x={SIZE - 8} y={SIZE / 2 + 4} text-anchor="end" class="lab">RIGHT</text>
      <circle cx={SIZE / 2 + knob.x} cy={SIZE / 2 + knob.y} r="26" class="knob" class:on={knob.x !== 0 || knob.y !== 0} />
    </svg>
  </div>
</div>

<style>
  .wrap { display: grid; place-items: center; }
  .stick { touch-action: none; user-select: none; cursor: grab; border-radius: 50%; }
  .stick:active { cursor: grabbing; }
  .off { opacity: .45; } .off .stick { cursor: not-allowed; }
  .base { fill: var(--soft); stroke: var(--line); stroke-width: 2; }
  .ring { fill: none; stroke: var(--line); stroke-width: 1.5; stroke-dasharray: 3 5; }
  .axis { stroke: var(--line); stroke-width: 1; }
  .lab { fill: var(--mute); font-size: 10px; letter-spacing: .08em; }
  .knob { fill: var(--surface); stroke: var(--ink); stroke-width: 2.5; transition: fill .1s; }
  .knob.on { fill: var(--accent); stroke: var(--accent); }
</style>
