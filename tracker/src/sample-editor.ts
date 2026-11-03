import type { Definition } from './project';
import { sampleFile, withSample } from './instruments';
import { readLoop, writeLoop } from './loops';

export interface SampleAnalysis {
  frames: number; rate: number; channels: number; peak: number;
  silence_start: number; silence_end: number; crossings: number[]; loop_start: number; loop_end: number;
}
export interface SampleEdit {
  start: number; end: number; gain_db: number; reverse: boolean; normalize: boolean; fade_in_ms: number; fade_out_ms: number;
}
export interface PreparationActions {
  analyze?(file: string): Promise<SampleAnalysis>;
  prepare?(file: string, edit: SampleEdit): Promise<string>;
  preview?(file: string, start: number, end: number, repeat: boolean): void;
  stop?(): void;
}

/** View and selection use source-file coordinates, independent of zoom. */
export class SampleEditor {
  readonly root = document.createElement('div');
  start = 0;
  end = 1;
  viewStart = 0;
  viewEnd = 1;
  analysis: SampleAnalysis | null = null;
  private definition: Definition | null = null;
  private file: string | undefined;
  private readonly readout = document.createElement('span');
  private readonly message = document.createElement('span');
  private readonly pan = document.createElement('input');
  private readonly from = document.createElement('input');
  private readonly to = document.createElement('input');
  private readonly snapMode = document.createElement('select');
  private hits: number[] = [];
  private busy = false;
  private revision = 0;

  constructor(private readonly actions: PreparationActions & { edit(d: Definition, source: string): void; detect(file: string, max?: number, sensitivity?: number): Promise<number[]> }, private readonly draw: () => void) {
    this.root.className = 'sample-edit-tools';
    const row = () => { const r = document.createElement('div'); r.className = 'sample-toolbar'; this.root.append(r); return r; };
    const view = row(), selection = row(), edits = row();
    const button = (parent: HTMLElement, text: string, run: () => void) => { const b = document.createElement('button'); b.textContent = text; b.onclick = run; parent.append(b); return b; };
    button(view, 'Zoom +', () => this.zoom(0.5)); button(view, 'Zoom −', () => this.zoom(2));
    button(view, 'Show all', () => { this.viewStart = 0; this.viewEnd = 1; this.changed(); });
    button(view, 'Zoom selection', () => { this.viewStart = this.start; this.viewEnd = this.end; this.changed(); });
    this.pan.type = 'range'; this.pan.min = '0'; this.pan.max = '1'; this.pan.step = 'any'; this.pan.value = '0'; this.pan.setAttribute('aria-label', 'Waveform horizontal scroll');
    this.pan.oninput = () => { const width = this.viewEnd - this.viewStart; this.viewStart = Number(this.pan.value) * (1 - width); this.viewEnd = this.viewStart + width; this.draw(); }; view.append(this.pan);
    this.snapMode.setAttribute('aria-label', 'Waveform snapping');
    for (const [value, text] of [['off', 'No snap'], ['zero', 'Zero crossings'], ['hits', 'Transients']]) { const o = document.createElement('option'); o.value = value; o.textContent = text; this.snapMode.append(o); }
    this.snapMode.onchange = () => { if (this.snapMode.value === 'hits' && this.file) void this.actions.detect(this.file, 256).then(hits => { this.hits = hits; }).catch(e => { this.message.textContent = String(e); }); };
    view.append(this.snapMode);
    for (const [input, label] of [[this.from, 'Selection start frame'], [this.to, 'Selection end frame']] as const) {
      const l = document.createElement('label'); l.textContent = label.replace('Selection ', ''); input.type = 'number'; input.min = '0'; input.step = '1'; input.setAttribute('aria-label', label); l.append(input); selection.append(l);
      input.onchange = () => { const frames = this.analysis?.frames; if (!frames) return; this.select(this.from.valueAsNumber / frames, this.to.valueAsNumber / frames); };
    }
    selection.append(this.readout);
    button(selection, 'Hear selection', () => this.preview(false)); button(selection, 'Repeat selection', () => this.preview(true)); button(selection, 'Stop preview', () => this.actions.stop?.());
    const gain = document.createElement('input'); gain.type = 'number'; gain.min = '-60'; gain.max = '24'; gain.value = '0'; gain.setAttribute('aria-label', 'Sample gain dB');
    const fadeIn = document.createElement('input'), fadeOut = document.createElement('input');
    for (const [input, label] of [[fadeIn, 'Fade in ms'], [fadeOut, 'Fade out ms']] as const) { input.type = 'number'; input.min = '0'; input.max = '60000'; input.value = '5'; input.setAttribute('aria-label', label); }
    const defaults = (): SampleEdit => ({ start: 0, end: 1, gain_db: 0, reverse: false, normalize: false, fade_in_ms: 0, fade_out_ms: 0 });
    button(edits, 'Keep selection', () => void this.prepare({ ...defaults(), start: this.start, end: this.end }));
    button(edits, 'Trim silence', () => { if (this.analysis) void this.prepare({ ...defaults(), start: this.analysis.silence_start, end: this.analysis.silence_end }); });
    button(edits, 'Reverse', () => void this.prepare({ ...defaults(), reverse: true }));
    button(edits, 'Normalize −1 dB', () => void this.prepare({ ...defaults(), normalize: true }));
    const gainLabel = document.createElement('label'); gainLabel.append('Gain dB ', gain); edits.append(gainLabel);
    button(edits, 'Apply gain', () => void this.prepare({ ...defaults(), gain_db: gain.valueAsNumber }));
    for (const [input, label] of [[fadeIn, 'In ms '], [fadeOut, 'Out ms ']] as const) { const l = document.createElement('label'); l.append(label, input); edits.append(l); }
    button(edits, 'Apply fades', () => void this.prepare({ ...defaults(), fade_in_ms: fadeIn.valueAsNumber, fade_out_ms: fadeOut.valueAsNumber }));
    button(edits, 'Suggest loop', () => {
      if (!this.analysis || !this.definition) return;
      if (/\.slices\(/.test(this.definition.source)) { this.message.textContent = 'Choose Play as one sample before setting a sustain loop.'; return; }
      const loop = readLoop(this.definition.source);
      const source = writeLoop(this.definition.source, { ...loop, mode: 'forward', start: this.analysis.loop_start, end: this.analysis.loop_end, crossfade: 3 });
      this.actions.edit(this.definition, source); this.select(this.analysis.loop_start, this.analysis.loop_end);
      this.message.textContent = 'Suggested sustain loop. Repeat selection to check the join.';
    });
    this.message.className = 'sample-note'; this.root.append(this.message);
  }
  async refresh(definition: Definition | null) {
    const sameDefinition = definition === this.definition;
    this.definition = definition;
    const file = definition ? sampleFile(definition) : undefined;
    if (file === this.file && sameDefinition) return;
    this.file = file; this.analysis = null; this.hits = []; this.start = this.viewStart = 0; this.end = this.viewEnd = 1; this.message.textContent = ''; this.changed();
    const revision = ++this.revision;
    if (!file || !this.actions.analyze) return;
    try { const analysis = await this.actions.analyze(file); if (revision !== this.revision) return; this.analysis = analysis; this.changed(); }
    catch (e) { if (revision === this.revision) this.message.textContent = `Could not analyze sample: ${e}`; }
  }
  select(start: number, end: number) {
    if (!Number.isFinite(start) || !Number.isFinite(end)) { this.changed(); return; }
    const frames = this.analysis?.frames ?? 48000, frame = 1 / frames;
    start = Math.round(start * frames) / frames; end = Math.round(end * frames) / frames;
    this.start = Math.max(0, Math.min(1 - frame, Math.min(start, end)));
    this.end = Math.max(this.start + frame, Math.min(1, Math.max(start, end))); this.changed();
  }
  snap(share: number) {
    const points = this.snapMode.value === 'zero' ? this.analysis?.crossings : this.snapMode.value === 'hits' ? this.hits : null;
    if (!points?.length) return share;
    // Snap only within 12 screen pixels, so dragging can still reach sparse regions.
    const threshold = (this.viewEnd - this.viewStart) * 0.012;
    let best = share, distance = threshold;
    for (const point of points) { if (Math.abs(point - share) < distance) { best = point; distance = Math.abs(point - share); } }
    return best;
  }
  private zoom(factor: number) {
    const span = Math.min(1, Math.max(1 / (this.analysis?.frames ?? 8192), (this.viewEnd - this.viewStart) * factor));
    const middle = (this.viewStart + this.viewEnd) / 2;
    this.viewStart = Math.max(0, Math.min(1 - span, middle - span / 2)); this.viewEnd = this.viewStart + span; this.changed();
  }
  private changed() {
    const frames = this.analysis?.frames ?? 0, rate = this.analysis?.rate ?? 48000;
    this.from.value = String(Math.round(this.start * frames)); this.to.value = String(Math.round(this.end * frames));
    this.from.max = this.to.max = String(frames);
    this.readout.textContent = `${(this.start * frames / rate).toFixed(4)}–${(this.end * frames / rate).toFixed(4)} s`;
    const span = this.viewEnd - this.viewStart;
    this.pan.disabled = span >= 1; this.pan.value = String(span < 1 ? this.viewStart / (1 - span) : 0); this.draw();
  }
  private preview(repeat: boolean) { if (this.file) this.actions.preview?.(this.file, this.start, this.end, repeat); }
  private async prepare(edit: SampleEdit) {
    const definition = this.definition, file = this.file;
    if (!definition || !file || !this.actions.prepare || this.busy) return;
    this.busy = true; this.message.textContent = 'Preparing sample…';
    try {
      const next = await this.actions.prepare(file, edit);
      if (this.definition !== definition || this.file !== file) return;
      const source = withSample(definition.source, next);
      if (source) this.actions.edit(definition, `// Original sample: ${file}\n${source}`);
      this.message.textContent = 'Prepared as a new file. Undo restores the previous sample.';
    } catch (e) { this.message.textContent = `Could not prepare sample: ${e}`; }
    finally { this.busy = false; }
  }
}
