<script lang="ts">
  import { onMount } from 'svelte';
  import { project, sel, selected, checkpoint, align, removeSelected, duplicateSelected, reorder, setPaper } from '../lib/doc.svelte';
  import { localGeom, requestOutlineFont, type ImageObj, type TextObj } from '../lib/render.svelte';
  import { listStrokeFonts } from '../lib/text/strokefont';
  import { PAPER_PRESETS } from '../lib/paper';
  import { DEFAULT_CONVERT_PARAMS, type ConvertMode } from '../lib/image/convert';
  import { api, type FontFile } from '../lib/api';

  let { textarea = $bindable<HTMLTextAreaElement | undefined>() }: { textarea?: HTMLTextAreaElement | undefined } = $props();
  const strokeFonts = listStrokeFonts();
  let sysFonts = $state<FontFile[]>([]);
  onMount(() => { api.systemFonts().then((f) => (sysFonts = f)).catch(() => {}); });

  const objs = $derived(selected());
  const one = $derived(objs.length === 1 ? objs[0] : null);
  const geo = $derived(one ? localGeom(one, project.assets) : null);
  const k = (key: string) => () => checkpoint(key);

  function paperPreset(id: string) {
    const p = PAPER_PRESETS.find((x) => x.id === id)!;
    setPaper(p.w, p.h, p.name, project.paper.orientation);
  }
  function orient(o: 'portrait' | 'landscape') { setPaper(project.paper.w, project.paper.h, project.paper.name, o); }
  function customSize(w: number, h: number) { setPaper(w, h, 'Custom', project.paper.w <= project.paper.h ? 'portrait' : 'landscape'); }
  function pickFont(path: string) {
    if (!one || one.type !== 'text') return;
    checkpoint('font'); one.fontPath = path; requestOutlineFont(path);
  }
  const modeList: [ConvertMode, string][] = [['outline', 'Outline'], ['lineart', 'Line art'], ['hatch', 'Hatching'], ['crosshatch', 'Crosshatching'], ['threshold', 'Threshold']];
  function show(img: ImageObj, ...keys: string[]) { return keys.includes(img.mode); }
  const sliders = [
    ['brightness', 'Brightness', -1, 1, 0.02, ['outline', 'lineart', 'hatch', 'crosshatch', 'threshold']],
    ['contrast', 'Contrast', -1, 1, 0.02, ['outline', 'lineart', 'hatch', 'crosshatch', 'threshold']],
    ['threshold', 'Threshold', 0.05, 0.95, 0.01, ['lineart', 'threshold']],
    ['edgeSensitivity', 'Edge sensitivity', 0, 1, 0.01, ['outline']],
    ['hatchAngleDeg', 'Hatch angle °', 0, 180, 1, ['hatch', 'crosshatch']],
    ['hatchSpacingMm', 'Hatch spacing mm', 0.4, 8, 0.1, ['hatch', 'crosshatch']],
    ['detail', 'Detail level', 0.05, 1, 0.01, ['outline', 'lineart', 'hatch', 'crosshatch', 'threshold']],
    ['minFeatureMm', 'Min feature mm', 0, 10, 0.1, ['outline', 'lineart', 'hatch', 'crosshatch', 'threshold']],
    ['simplifyMm', 'Simplification mm', 0, 2, 0.02, ['outline', 'lineart', 'threshold', 'hatch', 'crosshatch']],
  ] as const;
</script>

<div class="insp col">
  <div class="panel">
    <header><h3>Sheet</h3><span class="mono mute">{project.paper.w}×{project.paper.h} mm</span></header>
    <div class="body col">
      <div class="field"><span class="label">Paper</span>
        <select value={PAPER_PRESETS.find((p) => p.name === project.paper.name)?.id ?? 'custom'} onchange={(e) => paperPreset((e.target as HTMLSelectElement).value)}>
          {#each PAPER_PRESETS as p}<option value={p.id}>{p.name}{p.id !== 'custom' ? ` · ${p.w}×${p.h}` : ''}</option>{/each}
        </select></div>
      <div class="field"><span class="label">Orientation</span>
        <div class="seg"><button class:on={project.paper.orientation === 'portrait'} onclick={() => orient('portrait')}>Portrait</button><button class:on={project.paper.orientation === 'landscape'} onclick={() => orient('landscape')}>Landscape</button></div></div>
      <div class="field"><span class="label">Size mm</span>
        <span class="row"><input type="number" style="width:70px" value={project.paper.w} min="20" max="1000" onchange={(e) => customSize(+(e.target as HTMLInputElement).value, project.paper.h)} /> × <input type="number" style="width:70px" value={project.paper.h} min="20" max="1000" onchange={(e) => customSize(project.paper.w, +(e.target as HTMLInputElement).value)} /></span></div>
      <div class="field"><span class="label">Margin mm</span><input type="number" min="0" max="60" step="1" style="width:70px" value={project.paper.margin} oninput={(e) => { checkpoint('margin'); project.paper.margin = Math.max(0, +(e.target as HTMLInputElement).value); }} /></div>
    </div>
  </div>

  {#if !objs.length}
    <div class="panel"><div class="body mute small">Nothing selected. Add text (T), import an image or SVG, paste from the clipboard, drop a file, or sketch with the pen (P).</div></div>
  {:else}
    <div class="panel">
      <header><h3>{objs.length > 1 ? `${objs.length} objects` : one!.type}</h3>
        <span class="row"><button class="btn sm" title="Duplicate (Ctrl+D)" onclick={duplicateSelected}>Duplicate</button><button class="btn sm" onclick={removeSelected}>Delete</button></span></header>
      <div class="body col">
        {#if one}
          <div class="field"><span class="label">Name</span><input type="text" bind:value={one.name} oninput={k('name')} /></div>
          <div class="field"><span class="label">Position mm</span>
            <span class="row"><input type="number" style="width:72px" step="0.5" bind:value={one.x} oninput={k('pos')} /><input type="number" style="width:72px" step="0.5" bind:value={one.y} oninput={k('pos')} /></span></div>
          <div class="field"><span class="label">Rotation °</span><span class="row"><input type="number" style="width:72px" step="1" bind:value={one.rotation} oninput={k('rot')} /><input type="range" min="0" max="360" bind:value={one.rotation} oninput={k('rot')} /></span></div>
          {#if geo}<div class="mute small mono">{geo.w.toFixed(1)} × {geo.h.toFixed(1)} mm · {geo.strokes.length} strokes</div>{/if}
          {#each geo?.notes ?? [] as n}<div class="notice small">{n}</div>{/each}
        {:else}
          <div class="row wrap">
            <button class="btn sm" onclick={() => align('left')}>⇤ left</button><button class="btn sm" onclick={() => align('hcenter')}>↔ centre</button><button class="btn sm" onclick={() => align('right')}>right ⇥</button>
            <button class="btn sm" onclick={() => align('top')}>⤒ top</button><button class="btn sm" onclick={() => align('vcenter')}>↕ middle</button><button class="btn sm" onclick={() => align('bottom')}>bottom ⤓</button>
          </div>
          <p class="mute small m0">Drag to move the group; use the corner and rotate handles on the canvas to scale and rotate it as one.</p>
        {/if}
        <div class="row wrap">
          <button class="btn sm" onclick={() => align('page-h')}>Centre on page ↔</button>
          <button class="btn sm" onclick={() => align('page-v')}>↕</button>
          <button class="btn sm" onclick={() => reorder('front')}>Bring forward</button>
          <button class="btn sm" onclick={() => reorder('back')}>Send back</button>
        </div>
      </div>
    </div>

    {#if one?.type === 'text'}
      {@const t = one as TextObj}
      <div class="panel">
        <header><h3>Text</h3></header>
        <div class="body col">
          <textarea bind:this={textarea} rows="4" bind:value={t.text} oninput={k('text')} spellcheck="false"></textarea>
          <div class="field"><span class="label">Rendering</span>
            <div class="seg"><button class:on={t.mode === 'stroke'} onclick={() => { checkpoint(); t.mode = 'stroke'; }}>Single-stroke</button><button class:on={t.mode === 'outline'} onclick={() => { checkpoint(); t.mode = 'outline'; if (!t.fontPath && sysFonts[0]) pickFont((sysFonts.find((f) => /DejaVu Sans$/.test(f.family) && /Book|Regular/.test(f.style)) ?? sysFonts[0]).path); else requestOutlineFont(t.fontPath); }}>Outline font</button></div></div>
          {#if t.mode === 'stroke'}
            <div class="field"><span class="label">Font</span><select bind:value={t.fontId} onchange={k('font')}>{#each strokeFonts as f}<option value={f.id}>{f.name}</option>{/each}</select></div>
            <p class="mute small m0">Real single-line Hershey fonts: the pencil traces each letter once, like handwriting.</p>
          {:else}
            <div class="field"><span class="label">Font file</span>
              <select value={t.fontPath} onchange={(e) => pickFont((e.target as HTMLSelectElement).value)}>
                {#each sysFonts as f}<option value={f.path}>{f.family} — {f.style}</option>{/each}
                {#if !sysFonts.length}<option value="">No system fonts found</option>{/if}</select></div>
            <p class="mute small m0">Outline mode traces the <em>edges</em> of each glyph (hollow letters, double lines on thick strokes).</p>
          {/if}
          <div class="field"><span class="label">Size (cap height)</span><span class="row"><input type="number" style="width:72px" min="1" max="200" step="0.5" bind:value={t.sizeMm} oninput={k('size')} /> mm</span></div>
          <div class="field"><span class="label">Letter spacing</span><span class="row"><input type="number" style="width:72px" step="0.1" bind:value={t.letterSpacingMm} oninput={k('ls')} /> mm</span></div>
          <div class="field"><span class="label">Line spacing</span><span class="row"><input type="number" style="width:72px" min="0.5" max="4" step="0.05" bind:value={t.lineSpacing} oninput={k('lh')} /> ×</span></div>
          <div class="field"><span class="label">Align</span>
            <div class="seg">{#each ['left', 'center', 'right'] as a}<button class:on={t.align === a} onclick={() => { checkpoint(); t.align = a as 'left'; }}>{a}</button>{/each}</div></div>
          <div class="field"><span class="label">Wrap width</span><span class="row"><input type="number" style="width:72px" min="0" step="5" bind:value={t.maxWidthMm} oninput={k('wrap')} /> mm <span class="mute small">0 = no wrap</span></span></div>
          <button class="btn sm" onclick={() => { checkpoint(); t.maxWidthMm = Math.max(10, project.paper.w - 2 * project.paper.margin - t.x); }}>Wrap to page width</button>
        </div>
      </div>
    {/if}

    {#if one?.type === 'image'}
      {@const im = one as ImageObj}
      <div class="panel">
        <header><h3>Image → pencil</h3></header>
        <div class="body col">
          <div class="field"><span class="label">Mode</span><select bind:value={im.mode} onchange={k('mode')}>{#each modeList as [id, n]}<option value={id}>{n}</option>{/each}</select></div>
          <div class="field"><span class="label">Output width</span><span class="row"><input type="number" style="width:72px" min="5" max="500" step="1" bind:value={im.widthMm} oninput={k('iw')} /> mm</span></div>
          {#each sliders as [key, label, min, max, step, modes]}
            {#if show(im, ...modes)}
              <div class="field"><span class="label">{label}</span>
                <span class="row"><input type="range" {min} {max} {step} bind:value={im.params[key]} oninput={k('p' + key)} /><span class="mono tiny">{(+im.params[key]).toFixed(step < 0.1 ? 2 : 1)}</span></span></div>
            {/if}
          {/each}
          {#if im.mode === 'threshold' || im.mode === 'lineart'}
            <label class="row"><input type="checkbox" bind:checked={im.params.invert} onchange={k('inv')} /> Invert (trace light areas)</label>
          {/if}
          <button class="btn sm" onclick={() => { checkpoint(); im.params = { ...DEFAULT_CONVERT_PARAMS }; }}>Reset conversion</button>
          <p class="mute small m0">A single pencil line cannot reproduce photographic tone. Hatching approximates shading; outline and line-art work best on logos, drawings and high-contrast pictures.</p>
        </div>
      </div>
    {/if}

    {#if one && (one.type === 'svg' || one.type === 'path')}
      <div class="panel"><header><h3>{one.type === 'svg' ? 'SVG artwork' : 'Sketch'}</h3></header>
        <div class="body col"><div class="field"><span class="label">Scale</span><span class="row"><input type="number" style="width:72px" min="0.01" step="0.05" bind:value={one.scale} oninput={k('scale')} /> ×</span></div>
          {#if one.type === 'svg'}<p class="mute small m0">Shapes are drawn as outlines; fills are not hatched.</p>{/if}</div></div>
    {/if}
  {/if}
  <span hidden>{sel.ids.length}</span>
</div>

<style>
  .insp { padding: 0; } .small { font-size: 11.5px; } .m0 { margin: 0; } .wrap { flex-wrap: wrap; } .tiny { font-size: 11px; min-width: 38px; text-align: right; }
  .field { grid-template-columns: 104px 1fr; }
</style>
