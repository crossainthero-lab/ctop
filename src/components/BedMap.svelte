<script lang="ts">
  // Top-down map of the machine bed: nozzle, pencil tip, calibrated paper, safe envelope, simulator trail.
  import { api, type SimSnapshot } from '../lib/api';
  import { cal, machine } from '../lib/state.svelte';

  let { sim = null, trail = [] }: { sim?: SimSnapshot | null; trail?: [number, number, number, number][] } = $props();
  const bed = $derived<[number, number]>(machine.s.fwLimits ? [machine.s.fwLimits.xMax, machine.s.fwLimits.yMax] : [310, 310]);
  const r = $derived(cal.status?.resolved ?? null);
  const pos = $derived(sim ? sim.phys : machine.s.position);
  const off = $derived<[number, number]>(r ? r.offset : sim ? sim.truth.pencilOffset : [0, 0]);
  const M = 14;
  // machine Y grows away from the viewer → flip for display
  const X = (x: number) => M + x;
  const Y = (y: number) => M + bed[1] - y;
  const paperPoly = $derived.by(() => {
    if (!r) return '';
    const [w, h] = r.paper;
    const pts: [number, number][] = [[0, 0], [w, 0], [w, h], [0, h]].map(([px, py]) => {
      const u = r.swapXy ? py : px, v = r.swapXy ? px : py;
      return [r.origin[0] + (r.flipX ? -u : u), r.origin[1] + (r.flipY ? -v : v)];
    });
    return pts.map((p) => `${X(p[0])},${Y(p[1])}`).join(' ');
  });
  const simPaper = $derived(sim ? sim.truth.paperRect : null);
  const tipPt = $derived(pos ? { x: pos[0] + off[0], y: pos[1] + off[1] } : null);
</script>

<svg viewBox="0 0 {bed[0] + 2 * M} {bed[1] + 2 * M}" class="bed" role="img" aria-label="Machine bed map">
  <rect x={M} y={M} width={bed[0]} height={bed[1]} class="bedrect" />
  {#each Array.from({ length: Math.floor(bed[0] / 50) + 1 }, (_, i) => i * 50) as gx}
    <line x1={X(gx)} y1={M} x2={X(gx)} y2={M + bed[1]} class="g" /><text x={X(gx)} y={M + bed[1] + 10} class="t">{gx}</text>
  {/each}
  {#each Array.from({ length: Math.floor(bed[1] / 50) + 1 }, (_, i) => i * 50) as gy}
    <line x1={M} y1={Y(gy)} x2={M + bed[0]} y2={Y(gy)} class="g" /><text x="1" y={Y(gy) + 3} class="t">{gy}</text>
  {/each}
  {#if r}
    <rect x={X(r.envelope.xMin)} y={Y(r.envelope.yMax)} width={r.envelope.xMax - r.envelope.xMin} height={r.envelope.yMax - r.envelope.yMin} class="env" />
  {/if}
  {#if simPaper}
    <rect x={X(simPaper[0])} y={Y(simPaper[3])} width={simPaper[2] - simPaper[0]} height={simPaper[3] - simPaper[1]} class="truth" />
  {/if}
  {#if paperPoly}<polygon points={paperPoly} class="paper" />{/if}
  {#each trail as t}<line x1={X(t[0])} y1={Y(t[1])} x2={X(t[2])} y2={Y(t[3])} class="ink" />{/each}
  {#if pos}
    <circle cx={X(pos[0])} cy={Y(pos[1])} r="4" class="nozzle" />
    {#if tipPt}<line x1={X(pos[0])} y1={Y(pos[1])} x2={X(tipPt.x)} y2={Y(tipPt.y)} class="arm" /><circle cx={X(tipPt.x)} cy={Y(tipPt.y)} r="2.4" class="tip" />{/if}
  {/if}
</svg>

<style>
  .bed { width: 100%; height: 100%; background: var(--paper-2); display: block; }
  .bedrect { fill: #f7f5ef; stroke: var(--ink); stroke-width: .8; }
  .g { stroke: var(--line); stroke-width: .3; } .t { font: 6px var(--mono); fill: var(--mute-c); text-anchor: middle; }
  .t { text-anchor: start; } 
  .env { fill: none; stroke: var(--ok); stroke-width: .6; stroke-dasharray: 3 2; }
  .paper { fill: #fff; stroke: var(--ink); stroke-width: .6; fill-opacity: .9; }
  .truth { fill: none; stroke: var(--sim); stroke-width: .5; stroke-dasharray: 1 2; }
  .ink { stroke: var(--ink); stroke-width: .6; stroke-linecap: round; }
  .nozzle { fill: none; stroke: var(--graphite); stroke-width: 1; } .tip { fill: var(--danger); } .arm { stroke: var(--danger); stroke-width: .6; }
</style>
