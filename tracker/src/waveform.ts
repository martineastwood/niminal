import { readLoop, writeLoop, type SampleLoop } from './loops';
import type { Definition } from './project';
import { sampleFile } from './instruments';
import { withFit, withStretch, readFit, sourceTempo, estimateBeats } from './stretch';
import { addCut, cutNear, moveCut, pieceAt, pieceKey, readCuts, removeCut, writeCuts } from './slices';

/** A sample's waveform: its length, and the lowest and highest value of each of equal stretches of it (two numbers a stretch). */
export interface Peaks { frames: number; rate: number; peaks: number[] }

export interface SampleActions {
  /** The instrument whose sample to show. */
  current(): Definition | null;
  loadPeaks(file: string): Promise<Peaks>;
  /** Puts new code in an instrument and applies it. */
  edit(definition: Definition, source: string): void;
  /** Plays a piece of the instrument's sample (piece 0 when it is not cut). */
  hear(definition: Definition, piece: number): void;
  /** The shares of the sample at which its hits begin. */
  detect(file: string, max?: number): Promise<number[]>;
}

const height = 150, reach = 6;

/**
 * The waveform of the current sampler, with a marker where each piece of a sliced one starts. Drag a marker to move a cut, double-click
 * the waveform to add one, double-click a marker (or select it and press Delete) to remove it, and click to hear the piece under the pointer.
 */
export class SamplePanel {
  private readonly canvas = document.createElement('canvas');
  private readonly title = document.createElement('span');
  private readonly stretch = document.createElement('input');
  private readonly stretchMode = document.createElement('select');
  private readonly loopMode = document.createElement('select');
  private readonly loopStart = document.createElement('input');
  private readonly loopEnd = document.createElement('input');
  private readonly loopCrossfade = document.createElement('input');
  private loop: SampleLoop = { mode: 'none', start: 0, end: 1 };
  private loopDrag: 'start' | 'end' | null = null;
  private readonly beats = document.createElement('input');
  private readonly estimate = document.createElement('button');
  private estimating = false;
  private beatDraft: { definition: Definition; value: number } | null = null;
  private readonly fitted = document.createElement('span');
  private readonly fitButtons: HTMLButtonElement[] = [];
  private readonly hint = document.createElement('p');
  private readonly cache = new Map<string, Peaks>();
  private definition: Definition | null = null;
  private peaks: Peaks | null = null;
  private points: number[] = [];
  private selected = -1;
  private drag: { index: number; moved: boolean } | null = null;
  private hovered = -1;
  private revision = 0;

  constructor(private readonly root: HTMLElement, private readonly actions: SampleActions) {
    const bar = document.createElement('div'); bar.className = 'sample-bar';
    this.title.className = 'sample-title';
    const find = document.createElement('button'); find.textContent = 'Find hits'; find.title = 'Put a cut at each hit in the sample';
    find.onclick = () => void this.findHits();
    const equal = document.createElement('input'); equal.type = 'number'; equal.min = '2'; equal.max = '48'; equal.value = '16'; equal.setAttribute('aria-label', 'Equal pieces'); equal.style.width = '3.5em';
    const cut = document.createElement('button'); cut.textContent = 'Cut equally'; cut.title = 'Cut the sample into this many equal pieces';
    cut.onclick = () => { const count = Math.trunc(equal.valueAsNumber); if (count >= 2 && count <= 48) this.setPoints(Array.from({ length: count }, (_, i) => i / count)); };
    this.stretch.type = 'number'; this.stretch.min = '0.25'; this.stretch.max = '4'; this.stretch.step = '0.05'; this.stretch.value = '1';
    this.stretch.setAttribute('aria-label', 'Time stretch'); this.stretch.style.width = '4em';
    this.stretch.title = 'Duration multiplier, preserving pitch: 2 is twice as long';
    this.stretch.onchange = () => this.commitStretch();
    this.stretchMode.setAttribute('aria-label', 'Stretch algorithm');
    this.stretchMode.title = 'Granular: fixed windows; WSOLA: matches waveforms for smoother joins, using more CPU; Transient WSOLA: protects forward attacks';
    for (const [value, text] of [['0', 'Granular'], ['1', 'WSOLA'], ['2', 'Transient WSOLA']]) {
      const option = document.createElement('option'); option.value = value; option.textContent = text; this.stretchMode.append(option);
    }
    this.stretchMode.onchange = () => this.commitStretch();
    const stretchLabel = document.createElement('label'); stretchLabel.append('Stretch × ', this.stretch);
    this.beats.type = 'number'; this.beats.min = '0.25'; this.beats.max = '256'; this.beats.step = 'any'; this.beats.value = '4'; this.beats.style.width = '4em';
    this.beats.setAttribute('aria-label', 'Beats in the sample'); this.beats.title = 'How many beats the whole sample lasts, as recorded';
    const fit = document.createElement('button'); fit.textContent = 'Fit to tempo'; fit.title = 'Play the sample in time with the song, at any tempo, without changing its pitch';
    fit.onclick = () => this.fit(this.beats.valueAsNumber);
    const unfit = document.createElement('button'); unfit.textContent = 'Off'; unfit.title = 'Stop fitting the sample to the song\'s tempo';
    unfit.onclick = () => this.fit(0);
    this.estimate.textContent = 'Estimate beats'; this.estimate.title = 'Suggest the beat count and recorded tempo from detected hits; review it before fitting';
    this.estimate.onclick = () => void this.estimateTempo();
    const half = document.createElement('button'); half.textContent = '÷2'; half.setAttribute('aria-label', 'Halve beat count'); half.title = 'Halve the suggested or entered beat count';
    half.onclick = () => this.adjustBeats(0.5);
    const double = document.createElement('button'); double.textContent = '×2'; double.setAttribute('aria-label', 'Double beat count'); double.title = 'Double the suggested or entered beat count';
    double.onclick = () => this.adjustBeats(2);
    this.beats.oninput = () => { if (this.definition) this.beatDraft = { definition: this.definition, value: this.beats.valueAsNumber }; };
    this.beats.onchange = () => this.adjustBeats(1);
    this.fitButtons.push(fit, unfit, half, double);
    this.fitted.className = 'muted';
    const beatsLabel = document.createElement('label'); beatsLabel.append('lasts ', this.beats, ' beats');
    bar.append(this.title, find, equal, cut, stretchLabel, this.stretchMode, beatsLabel, this.estimate, half, double, fit, unfit, this.fitted);
    this.loopMode.setAttribute('aria-label', 'Sample loop mode');
    for (const [value, text] of [['none', 'Loop off'], ['forward', 'Forward loop'], ['bidirectional', 'Ping-pong loop']]) {
      const option = document.createElement('option'); option.value = value; option.textContent = text; this.loopMode.append(option);
    }
    this.loopMode.onchange = () => this.commitLoop();
    bar.append(this.loopMode);
    for (const [input, name] of [[this.loopStart, 'Start'], [this.loopEnd, 'End']] as const) {
      input.type = 'number'; input.min = '0'; input.max = '100'; input.step = '0.1'; input.style.width = '5em';
      input.setAttribute('aria-label', `Loop ${name.toLowerCase()} percent`);
      input.onchange = () => this.commitLoop();
      const label = document.createElement('label'); label.append(`${name} `, input, '%'); bar.append(label);
    }
    this.loopCrossfade.type = 'number'; this.loopCrossfade.min = '0'; this.loopCrossfade.step = '1'; this.loopCrossfade.style.width = '5em';
    this.loopCrossfade.setAttribute('aria-label', 'Loop crossfade milliseconds');
    this.loopCrossfade.onchange = () => this.commitLoop();
    const fadeLabel = document.createElement('label'); fadeLabel.append('Crossfade ', this.loopCrossfade, 'ms'); bar.append(fadeLabel);
    this.canvas.tabIndex = 0; this.canvas.className = 'sample-canvas'; this.canvas.setAttribute('aria-label', 'Sample waveform');
    this.hint.className = 'device-note';
    root.replaceChildren(bar, this.canvas, this.hint);
    this.canvas.onpointerdown = event => this.down(event);
    this.canvas.onpointermove = event => this.move(event);
    this.canvas.onpointerup = () => this.up();
    this.canvas.ondblclick = event => this.doubleClick(event);
    this.canvas.onkeydown = event => { if ((event.key === 'Delete' || event.key === 'Backspace') && this.selected > 0) { event.preventDefault(); this.setPoints(removeCut(this.points, this.selected)); this.selected = -1; } };
    if (typeof ResizeObserver !== 'undefined') new ResizeObserver(() => this.draw()).observe(root);
  }

  /** Reads the current instrument again: its sample, and where its code cuts it. Does nothing while a marker is being dragged. */
  async refresh() {
    if (this.drag || this.loopDrag) return;
    const revision = ++this.revision, definition = this.actions.current(), file = definition && sampleFile(definition);
    if (definition !== this.definition) { this.beatDraft = null; this.beats.value = '4'; }
    this.definition = definition;
    this.loop = readLoop(definition?.source ?? '');
    const wholeSample = !!file && !readCuts(definition?.source ?? '') && /begin: 0\.\.1 = 0/.test(definition?.source ?? '') && !!definition?.source.includes('begin: begin');
    this.loopMode.disabled = !wholeSample;
    this.showLoop();
    this.stretch.value = /stretch: 0\.25\.\.4 = ([\d.]+)/.exec(definition?.source ?? '')?.[1] ?? '1';
    this.stretch.disabled = !file || !definition?.source.includes('rate: rate');
    this.stretchMode.value = /stretch_mode: 0\.\.2 = ([012])/.exec(definition?.source ?? '')?.[1] ?? '0';
    this.stretchMode.disabled = this.stretch.disabled;
    for (const control of [this.beats, ...this.fitButtons]) control.disabled = this.stretch.disabled;
    this.estimate.disabled = this.stretch.disabled || this.estimating;
    if (!definition || !file) { this.showFit(''); this.peaks = null; this.points = []; this.title.textContent = definition?.name ?? ''; this.hint.textContent = 'Choose an instrument that plays a sample (♪ imports one) to see its waveform.'; this.draw(); return; }
    let peaks = this.cache.get(file);
    try { if (!peaks) { peaks = await this.actions.loadPeaks(file); this.cache.set(file, peaks); } }
    catch (error) { if (revision === this.revision) { this.peaks = null; this.hint.textContent = `Could not read ${file}: ${error}`; this.draw(); } return; }
    if (revision !== this.revision) return;
    this.peaks = peaks; this.points = readCuts(definition.source)?.points ?? []; this.showFit(definition.source);
    if (this.selected >= this.points.length) this.selected = -1;
    this.title.textContent = `${definition.name} · ${(peaks.frames / peaks.rate).toFixed(2)} s${this.points.length ? ` · ${this.points.length} pieces` : ''}`;
    this.hint.textContent = this.points.length
      ? 'Click a piece to hear it. Drag a marker to move a cut, double-click the waveform to add one, double-click a marker to remove it.'
      : this.loop.mode !== 'none' ? 'Drag the Start and End markers to set the sustain loop. Click to hear it. The intro plays once; the marked region repeats while the note is held.'
      : 'Click to hear the sample. The scissors (✂) in the instrument list cut it into pieces you can then move here.';
    this.draw();
  }

  /** Fits the sample to the song's tempo, given how many beats it lasts as recorded; 0 turns the fit off. */
  private fit(beats: number) {
    if (!this.definition || !this.peaks) return;
    const bpm = beats === 0 ? 0 : sourceTempo(beats, this.peaks.frames / this.peaks.rate);
    if (bpm === null || beats < 0 || !Number.isFinite(beats)) { this.hint.textContent = 'That many beats is not a tempo between 0 and 400 for this sample: check the number of beats.'; return; }
    this.beatDraft = null;
    const source = withFit(this.definition.source, bpm);
    if (source !== this.definition.source) this.actions.edit(this.definition, source);
    this.showFit(source);
  }
  private showFit(source: string) {
    const bpm = readFit(source), seconds = this.peaks ? this.peaks.frames / this.peaks.rate : 0;
    this.fitted.textContent = bpm ? `fitted: recorded at ${bpm} bpm` : 'not fitted to the tempo';
    if (this.beatDraft?.definition === this.definition) this.beats.value = String(this.beatDraft.value);
    else if (bpm && seconds) this.beats.value = String(Math.round(bpm * seconds / 60 * 100) / 100);
  }

  private async estimateTempo() {
    const definition = this.definition, peaks = this.peaks, file = definition && sampleFile(definition), revision = this.revision;
    if (!definition || !peaks || !file || this.estimating) return;
    this.estimating = true; this.estimate.disabled = true;
    this.hint.textContent = 'Estimating the beat count from detected hits…';
    const current = () => this.definition === definition && this.revision === revision && sampleFile(definition) === file;
    try {
      const hits = await this.actions.detect(file, 256);
      if (!current()) return;
      const result = estimateBeats(hits, peaks.frames / peaks.rate);
      if (!result) { this.hint.textContent = 'Not enough regular hits to estimate a tempo. Enter the beat count manually.'; return; }
      this.beats.value = String(result.beats);
      this.beatDraft = { definition, value: result.beats };
      this.hint.textContent = `Estimated ${result.beats} beats · ${result.bpm} bpm. Check by ear; use ÷2 or ×2 for half/double time, then Fit to tempo.`;
    } catch (error) {
      if (current()) this.hint.textContent = `Could not estimate the tempo: ${error}`;
    } finally {
      this.estimating = false; this.estimate.disabled = this.stretch.disabled;
    }
  }
  private adjustBeats(factor: number) {
    const value = this.beats.valueAsNumber * factor;
    if (!this.definition || !Number.isFinite(value) || value < 0.25 || value > 256) return;
    this.beats.value = String(value); this.beatDraft = { definition: this.definition, value };
    const bpm = this.peaks && sourceTempo(value, this.peaks.frames / this.peaks.rate);
    this.hint.textContent = bpm ? `${value} beats · ${bpm} bpm. Check by ear, then Fit to tempo.` : 'Check the beat count before fitting to tempo.';
  }

  private commitStretch() {
    const value = this.stretch.valueAsNumber, mode = this.stretchMode.value;
    if (!this.definition || !Number.isFinite(value) || value < 0.25 || value > 4 || !['0', '1', '2'].includes(mode)) return;
    let source = withStretch(this.definition.source);
    source = source.replace(/stretch: 0\.25\.\.4 = [\d.]+/, `stretch: 0.25..4 = ${value}`).replace(/stretch_mode: 0\.\.2 = [012]/, `stretch_mode: 0..2 = ${mode}`);
    if (source !== this.definition.source) this.actions.edit(this.definition, source);
  }

  private showLoop() {
    this.loopMode.value = this.loop.mode;
    this.loopStart.value = String(Number((this.loop.start * 100).toFixed(4)));
    this.loopEnd.value = String(Number((this.loop.end * 100).toFixed(4)));
    this.loopCrossfade.value = String(this.loop.crossfade ?? 0);
    this.loopCrossfade.disabled = this.loopMode.disabled || this.loop.mode === 'none';
    this.loopStart.disabled = this.loopEnd.disabled = this.loopMode.disabled || this.loop.mode === 'none';
  }
  private commitLoop() {
    if (!this.definition || this.loopMode.disabled) return;
    const loop: SampleLoop = { mode: this.loopMode.value as SampleLoop['mode'], start: this.loopStart.valueAsNumber / 100, end: this.loopEnd.valueAsNumber / 100, crossfade: this.loopCrossfade.valueAsNumber };
    if (!Number.isFinite(loop.crossfade) || (loop.crossfade ?? 0) < 0 || !Number.isFinite(loop.start) || !Number.isFinite(loop.end) || loop.start < 0 || loop.end > 1 || loop.end - loop.start < 1 / (this.peaks?.frames || 1)) {
      this.hint.textContent = 'Loop Start must be before End, between 0 and 100%; Crossfade must be zero or greater.'; this.showLoop(); return;
    }
    const source = writeLoop(this.definition.source, loop);
    this.loop = loop; this.showLoop(); this.draw();
    if (source !== this.definition.source) this.actions.edit(this.definition, source);
  }
  private loopNear(share: number, near: number): 'start' | 'end' | null {
    if (this.loopMode.disabled || this.loop.mode === 'none') return null;
    const a = Math.abs(share - this.loop.start), b = Math.abs(share - this.loop.end);
    return Math.min(a, b) <= near ? a <= b ? 'start' : 'end' : null;
  }

  private share(event: MouseEvent) {
    const box = this.canvas.getBoundingClientRect();
    return { share: Math.min(1, Math.max(0, (event.clientX - box.left) / (box.width || 1))), near: reach / (box.width || 1) };
  }
  private down(event: PointerEvent) {
    if (!this.definition || !this.peaks) return;
    const { share, near } = this.share(event), index = cutNear(this.points, share, near);
    this.canvas.focus();
    const loop = this.loopNear(share, near);
    if (loop) { this.loopDrag = loop; this.canvas.setPointerCapture?.(event.pointerId); return; }
    if (index >= 0) { this.selected = index; this.drag = { index, moved: false }; this.canvas.setPointerCapture?.(event.pointerId); }
    else { this.selected = -1; this.actions.hear(this.definition, this.points.length ? pieceAt(this.points, share) : 0); }
    this.draw();
  }
  private move(event: PointerEvent) {
    const { share, near } = this.share(event);
    if (this.loopDrag) {
      const gap = 1 / (this.peaks?.frames || 1);
      this.loop[this.loopDrag] = this.loopDrag === 'start' ? Math.min(share, this.loop.end - gap) : Math.max(share, this.loop.start + gap);
      this.showLoop(); this.draw();
    } else if (this.drag) {
      this.points = moveCut(this.points, this.drag.index, share); this.drag.moved = true; this.draw();
    } else {
      const hovered = cutNear(this.points, share, near);
      this.canvas.style.cursor = hovered >= 0 || this.loopNear(share, near) ? 'col-resize' : '';
      if (hovered !== this.hovered) { this.hovered = hovered; this.draw(); }
    }
  }
  private up() {
    if (this.loopDrag) { this.loopDrag = null; this.commitLoop(); return; }
    const drag = this.drag; this.drag = null;
    if (drag?.moved) this.commit();
  }
  private doubleClick(event: MouseEvent) {
    if (!this.peaks || !this.points.length) return;
    const { share, near } = this.share(event), index = cutNear(this.points, share, near);
    this.selected = -1;
    this.setPoints(index >= 0 ? removeCut(this.points, index) : addCut(this.points, share));
  }
  private setPoints(points: number[]) {
    if (points === this.points || !this.definition || !this.peaks) return;
    this.points = points; this.commit();
  }
  private commit() {
    if (!this.definition) return;
    const source = writeCuts(this.definition.source, this.points);
    if (source !== this.definition.source) this.actions.edit(this.definition, source);
    this.title.textContent = `${this.definition.name} · ${((this.peaks?.frames ?? 0) / (this.peaks?.rate || 1)).toFixed(2)} s · ${this.points.length} pieces`;
    this.draw();
  }
  private async findHits() {
    const file = this.definition && sampleFile(this.definition);
    if (!this.definition || !file) { this.hint.textContent = 'Choose an instrument that plays a sample first.'; return; }
    if (!this.points.length) { this.hint.textContent = 'Cut the sample with the scissors (✂) first; Find hits then moves the cuts to the hits.'; return; }
    try {
      const places = await this.actions.detect(file);
      if (places.length < 2) this.hint.textContent = 'No separate hits found in that sample.'; else this.setPoints(places.slice(0, 48));
    } catch (error) { this.hint.textContent = `Could not read the sample: ${error}`; }
  }

  private draw() {
    const context = this.canvas.getContext?.('2d');
    const box = this.canvas.getBoundingClientRect(), ratio = (typeof devicePixelRatio === 'number' ? devicePixelRatio : 1) || 1;
    const width = Math.max(1, Math.round((box.width || this.root.clientWidth || 600) * ratio));
    this.canvas.width = width; this.canvas.height = Math.round(height * ratio); this.canvas.style.height = `${height}px`;
    if (!context) return;
    const style = getComputedStyle(this.canvas), colour = (name: string, fallback: string) => style.getPropertyValue(name).trim() || fallback;
    context.fillStyle = colour('--bg-bar', '#1b1b1f'); context.fillRect(0, 0, width, this.canvas.height);
    const middle = this.canvas.height / 2;
    context.strokeStyle = colour('--dim', '#666'); context.beginPath(); context.moveTo(0, middle); context.lineTo(width, middle); context.stroke();
    if (!this.peaks) return;
    const { peaks } = this.peaks, buckets = peaks.length / 2, scale = middle * 0.95;
    context.strokeStyle = colour('--accent', '#6af'); context.beginPath();
    for (let x = 0; x < width; x++) {
      const from = Math.floor(x * buckets / width), to = Math.max(from + 1, Math.floor((x + 1) * buckets / width));
      let low = 0, high = 0;
      for (let b = from; b < Math.min(to, buckets); b++) { low = Math.min(low, peaks[b * 2]); high = Math.max(high, peaks[b * 2 + 1]); }
      context.moveTo(x + 0.5, middle - high * scale); context.lineTo(x + 0.5, middle - low * scale - 1);
    }
    context.stroke();
    context.font = `${10 * ratio}px sans-serif`;
    if (!this.loopMode.disabled && this.loop.mode !== 'none') {
      context.fillStyle = colour('--accent', '#6af'); context.globalAlpha = 0.12;
      context.fillRect(this.loop.start * width, 0, (this.loop.end - this.loop.start) * width, this.canvas.height); context.globalAlpha = 1;
      for (const [point, text] of [[this.loop.start, 'Start'], [this.loop.end, 'End']] as const) {
        const x = Math.min(width - 1, Math.max(1, point * width));
        context.strokeStyle = colour('--accent-text', '#fff'); context.lineWidth = 2;
        context.beginPath(); context.moveTo(x, 0); context.lineTo(x, this.canvas.height); context.stroke(); context.lineWidth = 1;
        context.fillStyle = colour('--text', '#ddd'); context.fillText(text, Math.min(x + 4, width - 35 * ratio), 12 * ratio);
      }
    }
    this.points.forEach((point, index) => {
      const x = Math.round(point * width) + 0.5, active = index === this.selected || index === this.hovered;
      context.strokeStyle = active ? colour('--accent-text', '#fff') : colour('--text', '#ddd'); context.lineWidth = active ? 2 : 1;
      context.beginPath(); context.moveTo(x, 0); context.lineTo(x, this.canvas.height); context.stroke(); context.lineWidth = 1;
      context.fillStyle = colour('--text', '#ddd'); context.fillText(pieceKey(index), x + 3, 11 * ratio);
    });
  }
}
