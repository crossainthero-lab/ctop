<script lang="ts">
  import type { Stroke } from '../lib/types';
  import { pathData } from '../lib/render.svelte';

  let { strokes, paper, done = 0, travel = true, width = 0.5 }: { strokes: Stroke[]; paper: [number, number]; done?: number; travel?: boolean; width?: number } = $props();
  const doneD = $derived(pathData(strokes.slice(0, done)));
  const todoD = $derived(pathData(strokes.slice(done)));
  const travelD = $derived.by(() => {
    if (!travel) return '';
    let d = '';
    for (let i = 1; i < strokes.length; i++) {
      const a = strokes[i - 1], b = strokes[i];
      d += `M${a[a.length - 1][0].toFixed(1)} ${a[a.length - 1][1].toFixed(1)}L${b[0][0].toFixed(1)} ${b[0][1].toFixed(1)}`;
    }
    return d;
  });
</script>

<svg viewBox="-4 -4 {paper[0] + 8} {paper[1] + 8}" class="pv" role="img" aria-label="Job preview">
  <rect x="0" y="0" width={paper[0]} height={paper[1]} class="sheet" />
  {#if travelD}<path d={travelD} class="travel" />{/if}
  <path d={todoD} class="todo" style="stroke-width:{width}" />
  <path d={doneD} class="done" style="stroke-width:{width * 1.2}" />
</svg>

<style>
  .pv { width: 100%; height: 100%; display: block; background: var(--paper-2); }
  .sheet { fill: #fff; stroke: var(--line-strong); stroke-width: .4; filter: drop-shadow(0 1px 2px rgba(0,0,0,.2)); }
  .travel { fill: none; stroke: var(--danger); stroke-width: .18; stroke-dasharray: 1.2 1.2; opacity: .7; }
  .todo, .done { fill: none; stroke-linecap: round; stroke-linejoin: round; }
  .todo { stroke: #9a978c; } .done { stroke: var(--ink); }
</style>
