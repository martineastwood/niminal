import { readLoop, writeLoop, type SampleLoop } from './loops';
import type { Definition } from './project';
import { sampleFile } from './instruments';
import { readPlayback, writePlayback, playbackNames, playbackHints, type Playback } from './playback';
import { withFit, readFit, readFitMode, withFitMode, sourceTempo, estimateBeats, type FitMode } from './stretch';
import { noteName, readRoot, writeRoot, readRelease, writeRelease, readInterpolation, writeInterpolation, type Interpolation } from './sampleprops';
import { unslice, addCut, cutNear, moveCut, pieceAt, pieceKey, readCuts, removeCut, writeCuts } from './slices';

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
  /** Makes a new instrument that plays an uncut sampler's sample in pieces starting at these shares of it, and shows that one. */
  cut(definition: Definition, pieces: number[]): void;
  /** Asks whether a cut instrument may play its whole sample as one (which loses its pieces); true when it may. */
  uncut(definition: Definition): Promise<boolean>;
  /** The song's tempo now, in beats a minute. */
  tempo(): number;
  /** The shares of the sample at which its hits begin. */
  detect(file: string, max?: number, sensitivity?: number): Promise<number[]>;
}

const height = 150, reach = 6, ruleSteps = [0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1, 2, 5, 10, 30, 60];

/**
 * The waveform of the current sampler, with a marker where each piece of a sliced one starts. Drag a marker to move a cut, double-click
 * the waveform to add one, double-click a marker (or select it and press Delete) to remove it, and click to hear the piece under the pointer.
 */
export class SamplePanel {
  private readonly canvas = document.createElement('canvas');
  private readonly title = document.createElement('span');
  private readonly stretch = document.createElement('input');
  private readonly stretchMode = document.createElement('select');
  private readonly playbackMode = document.createElement('select');
  /** How the instrument plays now, or null when its code is not a sampler of the tracker's; a loop can be set only on a whole sample. */
  private playback: Playback | null = null;
  private canLoop = false;
  /** The instrument plays pieces of its sample: it can loop only once it plays the whole of it, which asks first. */
  private cutUp = false;
  private loopCard!: HTMLElement;
  private readonly loopStart = document.createElement('input');
  private readonly loopEnd = document.createElement('input');
  private readonly loopCrossfade = document.createElement('input');
  private loop: SampleLoop = { mode: 'none', start: 0, end: 1 };
  private loopDrag: 'start' | 'end' | null = null;
  private readonly beats = document.createElement('input');
  private estimating = false;
  private beatDraft: { definition: Definition; value: number } | null = null;
  private readonly fitted = document.createElement('span');
  private readonly fitBox = document.createElement('input');
  private readonly sensitivity = document.createElement('input');
  private readonly uncutButton = document.createElement('button');
  private readonly rootNote = document.createElement('select');
  private readonly finetune = document.createElement('input');
  private readonly matchButton = document.createElement('button');
  private readonly fitMode = document.createElement('select');
  private fitModeRow!: HTMLElement;
  private readonly release = document.createElement('input');
  private readonly interpolation = document.createElement('select');
  private readonly fitButtons: HTMLInputElement[] = [];
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
    const toolbar = document.createElement('div'); toolbar.className = 'sample-toolbar';
    const props = document.createElement('div'); props.className = 'sample-props';
    /** A row of the settings column: a label, and what sets it. */
    const prop = (name: string, ...controls: (Node | string)[]) => {
      const row = document.createElement('label'); row.className = 'prop'; const text = document.createElement('span'); text.textContent = name; row.append(text, ...controls); props.append(row); return row;
    };
    this.title.className = 'sample-title';
    const find = document.createElement('button'); find.textContent = 'Find hits'; find.title = 'Put a cut at each hit in the sample; a higher sensitivity finds softer hits';
    find.onclick = () => void this.findHits();
    this.sensitivity.type = 'number'; this.sensitivity.min = '0'; this.sensitivity.max = '100'; this.sensitivity.step = '5'; this.sensitivity.value = '50';
    this.sensitivity.setAttribute('aria-label', 'Hit sensitivity percent'); this.sensitivity.title = 'How soft a hit can be and still be found, 0 to 100: 50 is the usual';
    const equal = document.createElement('input'); equal.type = 'number'; equal.min = '2'; equal.max = '48'; equal.value = '16'; equal.setAttribute('aria-label', 'Equal pieces');
    const cut = document.createElement('button'); cut.textContent = 'Cut equally'; cut.title = 'Cut the sample into this many equal pieces';
    cut.onclick = () => { const count = Math.trunc(equal.valueAsNumber); if (count >= 2 && count <= 48) this.setPoints(Array.from({ length: count }, (_, i) => i / count)); };
    const sensitivityLabel = document.createElement('label'); sensitivityLabel.append(this.sensitivity, '%');
    this.uncutButton.textContent = 'Play as one sample'; this.uncutButton.title = 'Remove the cuts: the instrument plays its whole sample again, which you can then loop';
    this.uncutButton.onclick = () => void this.playAsOne();
    toolbar.append(this.title, find, sensitivityLabel, cut, equal, this.uncutButton);

    this.playbackMode.setAttribute('aria-label', 'Playback');
    for (const value of Object.keys(playbackNames) as Playback[]) {
      const option = document.createElement('option'); option.value = value; option.textContent = playbackNames[value]; option.title = playbackHints[value]; this.playbackMode.append(option);
    }
    this.playbackMode.onchange = () => void this.commitPlayback();
    prop('Plays', this.playbackMode);

    const loopRows: HTMLElement[] = [];
    this.loopCard = document.createElement('div'); this.loopCard.className = 'prop-group';
    for (const [input, name, unit] of [[this.loopStart, 'Loop start', '%'], [this.loopEnd, 'Loop end', '%'], [this.loopCrossfade, 'Crossfade', 'ms']] as const) {
      input.type = 'number'; input.min = '0'; input.step = name === 'Crossfade' ? '1' : '0.1'; if (unit === '%') input.max = '100';
      input.setAttribute('aria-label', name === 'Crossfade' ? 'Loop crossfade milliseconds' : `${name} percent`);
      input.onchange = () => this.commitLoop();
      const row = document.createElement('label'); row.className = 'prop'; const text = document.createElement('span'); text.textContent = name;
      const unitText = document.createElement('small'); unitText.textContent = unit; row.append(text, input, unitText); loopRows.push(row);
    }
    this.loopCard.append(...loopRows); props.append(this.loopCard);

    this.rootNote.setAttribute('aria-label', 'Root note'); this.rootNote.title = 'The note at which the sample plays as recorded; playing it lower or higher changes its pitch (and speed)';
    for (let midi = 12; midi < 120; midi++) { const option = document.createElement('option'); option.value = String(midi); option.textContent = noteName(midi); this.rootNote.append(option); }
    this.rootNote.onchange = () => this.commitRoot();
    prop('Root note', this.rootNote);
    this.finetune.type = 'number'; this.finetune.min = '-50'; this.finetune.max = '50'; this.finetune.step = '1'; this.finetune.value = '0';
    this.finetune.setAttribute('aria-label', 'Finetune cents'); this.finetune.title = 'Retunes the root by up to half a semitone, in cents';
    this.finetune.onchange = () => this.commitRoot();
    const cents = document.createElement('small'); cents.textContent = 'ct';
    prop('Finetune', this.finetune, cents);
    this.matchButton.textContent = 'Match tempo'; this.matchButton.title = 'Once, retune the root so the sample lasts its number of beats at the song\'s tempo now. It changes the pitch, and does not follow later tempo changes; Fit to tempo does';
    this.matchButton.onclick = () => void this.matchTempo();
    const matchRow = document.createElement('div'); matchRow.className = 'prop'; matchRow.append(document.createElement('span'), this.matchButton); props.append(matchRow);

    this.fitBox.type = 'checkbox'; this.fitBox.setAttribute('aria-label', 'Fit to tempo');
    this.fitBox.title = 'Play the sample in time with the song, at any tempo, without changing its pitch. The first time, the beat count is worked out from the hits in the sample';
    this.fitBox.onchange = () => { if (this.fitBox.checked) void this.applyFit(); else this.fit(0); };
    this.fitted.className = 'sample-note';
    prop('Fit to tempo', this.fitBox, this.fitted);
    this.beats.type = 'number'; this.beats.min = '0.25'; this.beats.max = '256'; this.beats.step = 'any'; this.beats.value = '4';
    this.beats.setAttribute('aria-label', 'Beats in the sample'); this.beats.title = 'How many beats the whole sample lasts, as recorded. Once fitted, changing it refits the sample; use half or double if it sounds twice too slow or fast';
    this.beats.oninput = () => { if (this.definition) this.beatDraft = { definition: this.definition, value: this.beats.valueAsNumber }; };
    // once the sample is fitted, the beat count it lasts is what the fit follows
    this.beats.onchange = () => { if (this.definition && readFit(this.definition.source)) this.fit(this.beats.valueAsNumber); };
    const beatsUnit = document.createElement('small'); beatsUnit.textContent = 'beats';
    prop('Lasts', this.beats, beatsUnit);
    this.fitMode.setAttribute('aria-label', 'Fit by'); this.fitMode.title = 'Stretch keeps the pitch when the tempo changes; Repitch plays faster or slower, as tape would, and the pitch follows';
    for (const [value, text] of [['stretch', 'Stretch (keeps pitch)'], ['repitch', 'Repitch (tape)']]) { const option = document.createElement('option'); option.value = value; option.textContent = text; this.fitMode.append(option); }
    this.fitMode.onchange = () => this.rewrite(source => withFitMode(source, this.fitMode.value as FitMode));
    this.fitModeRow = prop('Fit by', this.fitMode);

    this.stretch.type = 'number'; this.stretch.min = '0.25'; this.stretch.max = '4'; this.stretch.step = '0.05'; this.stretch.value = '1';
    this.stretch.setAttribute('aria-label', 'Time stretch');
    this.stretch.title = 'Duration multiplier, preserving pitch: 2 is twice as long. It applies at the original speed and on top of a tempo fit';
    this.stretch.onchange = () => this.commitStretch();
    prop('Stretch ×', this.stretch);
    this.stretchMode.setAttribute('aria-label', 'Stretch algorithm');
    this.stretchMode.title = 'Granular: fixed windows; WSOLA: matches waveforms for smoother joins, using more CPU; Transient WSOLA: protects forward attacks';
    for (const [value, text] of [['0', 'Granular'], ['1', 'WSOLA'], ['2', 'Transient WSOLA']]) {
      const option = document.createElement('option'); option.value = value; option.textContent = text; this.stretchMode.append(option);
    }
    this.stretchMode.onchange = () => this.commitStretch();
    prop('Algorithm', this.stretchMode);
    this.release.type = 'number'; this.release.min = '0'; this.release.max = '2000'; this.release.step = '5'; this.release.value = '10';
    this.release.setAttribute('aria-label', 'Release milliseconds'); this.release.title = 'How long the sample takes to fade out when its note ends, to avoid a click; it matters for Hold and Loop';
    this.release.onchange = () => { const ms = this.release.valueAsNumber; if (Number.isFinite(ms) && ms >= 0 && ms <= 2000) this.rewrite(source => writeRelease(source, ms)); else this.showProps(); };
    const ms = document.createElement('small'); ms.textContent = 'ms';
    prop('Release', this.release, ms);
    this.interpolation.setAttribute('aria-label', 'Interpolation'); this.interpolation.title = 'How the sample is read between its frames when it is played at another pitch. Linear is the lightest and roughest at low pitches, Sinc has the least aliasing and costs the most CPU';
    for (const [value, text] of [['linear', 'Linear (rough, light)'], ['cubic', 'Cubic'], ['sinc', 'Sinc (best, more CPU)']]) { const option = document.createElement('option'); option.value = value; option.textContent = text; this.interpolation.append(option); }
    this.interpolation.onchange = () => this.rewrite(source => writeInterpolation(source, this.interpolation.value as Interpolation));
    prop('Interpolation', this.interpolation);
    this.fitButtons.push(this.fitBox);
    this.canvas.tabIndex = 0; this.canvas.className = 'sample-canvas'; this.canvas.setAttribute('aria-label', 'Sample waveform');
    this.hint.className = 'device-note';
    const stage = document.createElement('div'); stage.className = 'sample-stage'; stage.append(this.canvas);
    const main = document.createElement('div'); main.className = 'sample-main'; main.append(toolbar, stage, this.hint);
    root.replaceChildren(main, props);
    this.canvas.onpointerdown = event => this.down(event);
    this.canvas.onpointermove = event => this.move(event);
    this.canvas.onpointerup = () => this.up();
    this.canvas.ondblclick = event => this.doubleClick(event);
    this.canvas.onkeydown = event => { if ((event.key === 'Delete' || event.key === 'Backspace') && this.selected > 0) { event.preventDefault(); this.setPoints(removeCut(this.points, this.selected)); this.selected = -1; } };
    if (typeof ResizeObserver !== 'undefined') new ResizeObserver(() => this.draw()).observe(stage);
  }

  /** Reads the current instrument again: its sample, and where its code cuts it. Does nothing while a marker is being dragged. */
  async refresh() {
    if (this.drag || this.loopDrag) return;
    const revision = ++this.revision, definition = this.actions.current(), file = definition && sampleFile(definition);
    if (definition !== this.definition) { this.beatDraft = null; this.beats.value = '4'; }
    this.definition = definition;
    this.loop = readLoop(definition?.source ?? '');
    const wholeSample = !!file && !readCuts(definition?.source ?? '') && /begin: 0\.\.1 = 0/.test(definition?.source ?? '') && !!definition?.source.includes('begin: begin');
    this.canLoop = wholeSample; this.cutUp = !!file && !!readCuts(definition?.source ?? ''); this.showCutTools(); this.playback = definition ? readPlayback(definition.source) : null;
    this.showProps(definition?.source ?? '');
    this.showLoop();
    this.stretch.value = /stretch: 0\.25\.\.4 = ([\d.]+)/.exec(definition?.source ?? '')?.[1] ?? '1';
    this.stretch.disabled = !file || !definition?.source.includes('rate: rate');
    this.stretchMode.value = /stretch_mode: 0\.\.2 = ([012])/.exec(definition?.source ?? '')?.[1] ?? '0';
    this.stretchMode.disabled = this.stretch.disabled;
    for (const control of [this.beats, ...this.fitButtons]) control.disabled = this.stretch.disabled;
    if (!definition || !file) { this.showFit(''); this.peaks = null; this.points = []; this.title.textContent = definition?.name ?? ''; this.hint.textContent = 'Choose an instrument that plays a sample (drop an audio file on the right bar to import one) to see its waveform.'; this.draw(); return; }
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
      : 'Click to hear the sample. Find hits or Cut equally make a new instrument that plays it in pieces, which you can then move here.';
    this.draw();
  }

  /** Fits the sample to the song's tempo, given how many beats it lasts as recorded; 0 turns the fit off. */
  private fit(beats: number) {
    if (!this.definition || !this.peaks) return;
    const bpm = beats === 0 ? 0 : sourceTempo(beats, this.peaks.frames / this.peaks.rate);
    if (bpm === null || beats < 0 || !Number.isFinite(beats)) { this.hint.textContent = 'That many beats is not a tempo between 0 and 400 for this sample: check the number of beats.'; this.showFit(this.definition.source); return; }
    this.beatDraft = beats === 0 ? { definition: this.definition, value: this.beats.valueAsNumber } : null; // turning it off keeps the beat count for the next fit
    const source = withFit(this.definition.source, bpm);
    if (source !== this.definition.source) this.actions.edit(this.definition, source);
    this.showFit(source);
  }
  private showFit(source: string) {
    const bpm = readFit(source), seconds = this.peaks ? this.peaks.frames / this.peaks.rate : 0;
    if (this.beatDraft?.definition === this.definition) this.beats.value = String(this.beatDraft.value);
    else if (bpm && seconds) this.beats.value = String(Math.round(bpm * seconds / 60 * 100) / 100);
    this.fitBox.checked = !!bpm; this.fitModeRow.hidden = !bpm;
    this.fitted.textContent = bpm ? `recorded at ${bpm} bpm` : '';
  }

  /** Fits to the song's tempo. The first time, with no beat count entered, it is worked out from the hits in the sample. */
  private async applyFit() {
    const definition = this.definition;
    if (!definition || !this.peaks) { this.fitBox.checked = false; return; }
    if (!readFit(definition.source) && this.beatDraft?.definition !== definition && !(await this.estimateTempo())) { this.showFit(definition.source); return; }
    if (this.definition === definition) this.fit(this.beats.valueAsNumber);
  }

  /** Works out how many beats the sample lasts from its hits and puts it in the beat count; false, with a note, when it cannot. */
  private async estimateTempo(): Promise<boolean> {
    const definition = this.definition, peaks = this.peaks, file = definition && sampleFile(definition), revision = this.revision;
    if (!definition || !peaks || !file || this.estimating) return false;
    this.estimating = true;
    this.hint.textContent = 'Working out the beat count from the hits…';
    const current = () => this.definition === definition && this.revision === revision && sampleFile(definition) === file;
    // whatever the outcome, the next press of Fit to tempo uses the number in the box
    const keep = () => { this.beatDraft = { definition, value: this.beats.valueAsNumber }; };
    try {
      const hits = await this.actions.detect(file, 256);
      if (!current()) return false;
      const result = estimateBeats(hits, peaks.frames / peaks.rate);
      if (!result) { keep(); this.hint.textContent = 'Not enough regular hits to work out the tempo: enter how many beats the sample lasts, then press Fit to tempo again.'; return false; }
      this.beats.value = String(result.beats); this.beatDraft = { definition, value: result.beats };
      this.hint.textContent = `Fitted as ${result.beats} beats (${result.bpm} bpm). If it sounds half or double speed, halve or double the beat count.`;
      return true;
    } catch (error) {
      if (current()) { keep(); this.hint.textContent = `Could not work out the tempo: ${error}. Enter the beat count, then press Fit to tempo again.`; }
      return false;
    } finally { this.estimating = false; }
  }

  /** Rewrites the instrument's code with `transform`, and applies it when that changes it. */
  private rewrite(transform: (source: string) => string) {
    if (!this.definition) return;
    const source = transform(this.definition.source);
    if (source !== this.definition.source) this.actions.edit(this.definition, source);
    this.showProps(source);
  }

  /** Shows the properties the code sets: root pitch, release, interpolation, and how a fit follows the tempo. */
  private showProps(source = this.definition?.source ?? '') {
    const root = readRoot(source), release = readRelease(source), interpolation = readInterpolation(source);
    this.rootNote.value = String(root?.midi ?? 60); this.finetune.value = String(root?.cents ?? 0);
    this.rootNote.disabled = this.finetune.disabled = this.matchButton.disabled = !root;
    this.release.value = String(release ?? 10); this.release.disabled = release === null;
    this.interpolation.value = interpolation ?? 'cubic'; this.interpolation.disabled = interpolation === null;
    this.fitMode.value = readFitMode(source); this.fitModeRow.hidden = !readFit(source);
  }

  private commitRoot() {
    const midi = Number(this.rootNote.value), cents = Math.round(this.finetune.valueAsNumber);
    if (!Number.isFinite(midi) || !Number.isFinite(cents) || cents < -50 || cents > 50) { this.showProps(); return; }
    this.rewrite(source => writeRoot(source, { midi, cents }));
  }

  /** Retunes the root, once, so the sample lasts the beats it was given at the song's tempo now: faster playback is a lower root. */
  private async matchTempo() {
    const definition = this.definition, peaks = this.peaks, root = definition && readRoot(definition.source);
    if (!definition || !peaks || !root) return;
    if (readFit(definition.source)) { this.hint.textContent = 'Untick Fit to tempo first: it already follows the tempo, and this would apply it twice.'; return; }
    if (this.beatDraft?.definition !== definition && !(await this.estimateTempo())) return;
    if (this.definition !== definition) return;
    const from = sourceTempo(this.beats.valueAsNumber, peaks.frames / peaks.rate), song = this.actions.tempo();
    if (!from || !(song > 0)) { this.hint.textContent = 'That many beats is not a tempo for this sample: check the number of beats.'; return; }
    const shift = 12 * Math.log2(song / from), exact = root.midi + root.cents / 100 - shift, midi = Math.round(exact);
    this.rewrite(source => writeRoot(source, { midi, cents: Math.round((exact - midi) * 100) }));
    this.hint.textContent = `The sample now plays ${Math.abs(shift).toFixed(2)} semitones ${shift >= 0 ? 'higher (faster)' : 'lower (slower)'} and lasts ${this.beats.valueAsNumber} beats at ${song} bpm. It will not follow tempo changes.`;
  }

  private commitStretch() {
    const value = this.stretch.valueAsNumber, mode = this.stretchMode.value;
    if (!this.definition || !Number.isFinite(value) || value < 0.25 || value > 4 || !['0', '1', '2'].includes(mode)) return;
    const source = this.definition.source.replace(/stretch: 0\.25\.\.4 = [\d.]+/, `stretch: 0.25..4 = ${value}`).replace(/stretch_mode: 0\.\.2 = [012]/, `stretch_mode: 0..2 = ${mode}`);
    if (source !== this.definition.source) this.actions.edit(this.definition, source);
  }

  private showLoop() {
    this.playbackMode.value = this.playback ?? 'one-shot';
    this.playbackMode.disabled = !this.playback;
    this.playbackMode.title = this.playback ? playbackHints[this.playback] : 'Choose an instrument that plays a sample';
    for (const option of this.playbackMode.options) option.disabled = (option.value === 'loop' || option.value === 'ping-pong') && !this.canLoop && !this.cutUp;
    const looping = this.canLoop && (this.playback === 'loop' || this.playback === 'ping-pong');
    this.loopStart.value = String(Number((this.loop.start * 100).toFixed(4)));
    this.loopEnd.value = String(Number((this.loop.end * 100).toFixed(4)));
    this.loopCrossfade.value = String(this.loop.crossfade ?? 0);
    this.loopCrossfade.disabled = this.loopStart.disabled = this.loopEnd.disabled = !looping; this.loopCard.hidden = !looping;
  }
  /** The Plays choice: rewrites the sampler's code, and shows the loop's handles when it is a loop. */
  private async commitPlayback() {
    const definition = this.definition;
    if (!definition || !this.playback) return;
    const playback = this.playbackMode.value as Playback;
    // a loop repeats the whole sample, so a cut instrument becomes a plain sampler of it first, once that is agreed
    if ((playback === 'loop' || playback === 'ping-pong') && this.cutUp && !(await this.playAsOne())) { this.showLoop(); return; }
    if (this.definition !== definition) return;
    const source = writePlayback(definition.source, playback);
    this.playback = readPlayback(source); this.loop = readLoop(source);
    this.showLoop(); this.draw();
    if (source !== definition.source) this.actions.edit(definition, source);
  }

  /** Rewrites a cut instrument as a sampler of its whole sample, after asking; false when it is declined or cannot be. */
  private async playAsOne(): Promise<boolean> {
    const definition = this.definition, whole = definition && unslice(definition.source);
    if (!definition || !whole || !(await this.actions.uncut(definition)) || this.definition !== definition) return false;
    this.actions.edit(definition, whole);
    this.cutUp = false; this.canLoop = true; this.points = []; this.selected = -1;
    this.playback = readPlayback(whole); this.loop = readLoop(whole);
    this.showLoop(); this.showCutTools(); this.draw();
    return true;
  }

  /** The tool that undoes the cuts is there only while the instrument is cut. */
  private showCutTools() { this.uncutButton.hidden = !this.cutUp; }
  private commitLoop() {
    if (!this.definition || !this.canLoop || !this.playback) return;
    const loop: SampleLoop = { mode: this.loop.mode, start: this.loopStart.valueAsNumber / 100, end: this.loopEnd.valueAsNumber / 100, crossfade: this.loopCrossfade.valueAsNumber };
    if (!Number.isFinite(loop.crossfade) || (loop.crossfade ?? 0) < 0 || !Number.isFinite(loop.start) || !Number.isFinite(loop.end) || loop.start < 0 || loop.end > 1 || loop.end - loop.start < 1 / (this.peaks?.frames || 1)) {
      this.hint.textContent = 'Loop Start must be before End, between 0 and 100%; Crossfade must be zero or greater.'; this.showLoop(); return;
    }
    const source = writeLoop(this.definition.source, loop);
    this.loop = loop; this.showLoop(); this.draw();
    if (source !== this.definition.source) this.actions.edit(this.definition, source);
  }
  private loopNear(share: number, near: number): 'start' | 'end' | null {
    if (!this.canLoop || this.loop.mode === 'none') return null;
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
    // a sampler that is not cut yet stays as it is: the pieces go to a new instrument of their own
    if (!readCuts(this.definition.source)) { this.hint.textContent = `Cut into ${points.length} pieces: made a new instrument.`; this.actions.cut(this.definition, points); return; }
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
    try {
      const sensitivity = Math.min(100, Math.max(0, this.sensitivity.valueAsNumber || 50)) / 100, places = await this.actions.detect(file, 48, sensitivity);
      if (places.length < 2) this.hint.textContent = 'No separate hits found in that sample.'; else this.setPoints(places.slice(0, 48));
    } catch (error) { this.hint.textContent = `Could not read the sample: ${error}`; }
  }

  /** A scale of seconds along the bottom, with a mark about every 70 pixels, at a round number of seconds. */
  private drawRuler(context: CanvasRenderingContext2D, width: number, ruler: number, ratio: number, colour: (name: string, fallback: string) => string) {
    const seconds = this.peaks ? this.peaks.frames / this.peaks.rate : 0;
    if (!seconds) return;
    const step = ruleSteps.find(candidate => candidate / seconds * width >= 70 * ratio) ?? ruleSteps.at(-1)!, top = this.canvas.height - ruler;
    context.fillStyle = colour('--bg-panel', '#16181b'); context.fillRect(0, top, width, ruler);
    context.strokeStyle = colour('--dim', '#666'); context.fillStyle = colour('--dim', '#888'); context.font = `${9 * ratio}px sans-serif`;
    context.beginPath();
    for (let t = 0; t <= seconds + 1e-9; t += step) {
      const x = Math.round(t / seconds * width) + 0.5;
      context.moveTo(x, top); context.lineTo(x, top + 4 * ratio);
      context.fillText(`${Number(t.toFixed(3))}s`, Math.min(x + 3, width - 30 * ratio), top + 11 * ratio);
    }
    context.stroke();
  }

  private draw() {
    const context = this.canvas.getContext?.('2d');
    const box = this.canvas.getBoundingClientRect(), ratio = (typeof devicePixelRatio === 'number' ? devicePixelRatio : 1) || 1;
    const width = Math.max(1, Math.round((box.width || this.root.clientWidth || 600) * ratio));
    this.canvas.width = width; this.canvas.height = Math.round((box.height || height) * ratio);
    if (!context) return;
    const style = getComputedStyle(this.canvas), colour = (name: string, fallback: string) => style.getPropertyValue(name).trim() || fallback;
    context.fillStyle = colour('--bg-bar', '#1b1b1f'); context.fillRect(0, 0, width, this.canvas.height);
    const ruler = 14 * ratio, middle = (this.canvas.height - ruler) / 2;
    context.strokeStyle = colour('--dim', '#666'); context.beginPath(); context.moveTo(0, middle); context.lineTo(width, middle); context.stroke();
    if (!this.peaks) return;
    this.drawRuler(context, width, ruler, ratio, colour);
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
    if (this.canLoop && this.loop.mode !== 'none') {
      context.fillStyle = colour('--accent', '#6af'); context.globalAlpha = 0.12;
      context.fillRect(this.loop.start * width, 0, (this.loop.end - this.loop.start) * width, this.canvas.height); context.globalAlpha = 1;
      for (const [point, text] of [[this.loop.start, 'Start'], [this.loop.end, 'End']] as const) {
        const x = Math.min(width - 1, Math.max(1, point * width));
        context.strokeStyle = colour('--accent-text', '#fff'); context.lineWidth = 2;
        context.beginPath(); context.moveTo(x, 0); context.lineTo(x, this.canvas.height); context.stroke(); context.lineWidth = 1;
        context.fillStyle = colour('--text', '#ddd'); context.fillText(text, Math.min(x + 4, width - 35 * ratio), 12 * ratio);
      }
    }
    let labelled = -Infinity; // a note name is drawn only where there is room after the last one, so close cuts do not pile them up
    this.points.forEach((point, index) => {
      const x = Math.round(point * width) + 0.5, active = index === this.selected || index === this.hovered;
      context.strokeStyle = active ? colour('--accent-text', '#fff') : colour('--text', '#ddd'); context.lineWidth = active ? 2 : 1;
      context.beginPath(); context.moveTo(x, 0); context.lineTo(x, this.canvas.height); context.stroke(); context.lineWidth = 1;
      if (active || x - labelled >= 30 * ratio) { context.fillStyle = colour('--text', '#ddd'); context.fillText(pieceKey(index), x + 3, 11 * ratio); labelled = x; }
    });
  }
}
