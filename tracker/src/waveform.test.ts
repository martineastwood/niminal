// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { SamplePanel, type SampleActions } from './waveform';
import { addInstrument, sliceInstrument } from './instruments';
import { readCuts } from './slices';
import { newProject } from './project';

afterEach(() => document.body.replaceChildren());

/** Presses a button of the panel by its label; "Fit to tempo" and "Original" are the two states of its checkbox. */
const pressIn = (root: HTMLElement) => (label: string) => {
  if (label === 'Fit to tempo' || label === 'Original') { const box = root.querySelector<HTMLInputElement>('[aria-label="Fit to tempo"]')!; box.checked = label === 'Fit to tempo'; box.dispatchEvent(new Event('change')); return; }
  [...root.querySelectorAll('button')].find(b => b.textContent === label)!.click();
};

async function setup(cuts: number | number[] | null = 4) {
  const project = newProject(), sampler = addInstrument(project, 'samples/amen.wav'), definition = cuts === null ? sampler : sliceInstrument(project, sampler, cuts)!;
  const root = document.createElement('div'); document.body.append(root);
  const actions = {
    current: () => definition,
    loadPeaks: vi.fn(async () => ({ frames: 48000, rate: 48000, peaks: Array.from({ length: 200 }, (_, i) => (i % 2 ? 0.5 : -0.5)) })),
    edit: vi.fn((target: typeof definition, source: string) => { target.source = source; }),
    hear: vi.fn(),
    detect: vi.fn(async () => [0, 0.1, 0.4]),
    cut: vi.fn(),
    tempo: vi.fn(() => 120),
    uncut: vi.fn(async () => true),
  } satisfies SampleActions;
  const panel = new SamplePanel(root, actions), canvas = root.querySelector<HTMLCanvasElement>('canvas')!;
  canvas.getBoundingClientRect = () => ({ left: 0, top: 0, width: 1000, height: 150, right: 1000, bottom: 150, x: 0, y: 0, toJSON() {} });
  await panel.refresh();
  const mouse = (type: string, x: number) => canvas.dispatchEvent(new MouseEvent(type, { clientX: x, bubbles: true }));
  return { root, canvas, actions, definition, mouse, panel, text: () => root.textContent ?? '' };
}

it('shows the sample and how it is cut, or says what to do when it is not a sampler', async () => {
  const { text, actions } = await setup(4);
  expect(actions.loadPeaks).toHaveBeenCalledWith('samples/amen.wav');
  expect(text()).toContain('1.00 s · 4 pieces'); expect(text()).toContain('Click a piece to hear it');
  const uncut = await setup(null);
  expect(uncut.text()).toContain('1.00 s'); expect(uncut.text()).not.toMatch(/\d pieces/); expect(uncut.text()).toContain('Find hits or Cut equally');
  const synth = newProject().definitions[0], root = document.createElement('div'); document.body.append(root);
  const none = new SamplePanel(root, { current: () => synth, loadPeaks: vi.fn(), edit: vi.fn(), hear: vi.fn(), detect: vi.fn(), cut: vi.fn(), tempo: () => 120, uncut: async () => true });
  await none.refresh(); expect(root.textContent).toContain('Choose an instrument that plays a sample');
});

it('plays the piece that is clicked, and the whole sample when it is not cut', async () => {
  const { mouse, actions, definition } = await setup(4);
  mouse('pointerdown', 100); mouse('pointerup', 100); expect(actions.hear).toHaveBeenLastCalledWith(definition, 0);
  mouse('pointerdown', 600); mouse('pointerup', 600); expect(actions.hear).toHaveBeenLastCalledWith(definition, 2);
  const uncut = await setup(null); uncut.mouse('pointerdown', 800); expect(uncut.actions.hear).toHaveBeenLastCalledWith(uncut.definition, 0);
});

it('moves a cut by dragging its marker and writes the new place into the code once, on release', async () => {
  const { mouse, actions, definition } = await setup(4);
  mouse('pointerdown', 502); // the cut at 0.5
  mouse('pointermove', 600); mouse('pointermove', 700);
  expect(actions.edit).not.toHaveBeenCalled(); expect(actions.hear).not.toHaveBeenCalled();
  mouse('pointerup', 700);
  expect(actions.edit).toHaveBeenCalledOnce(); expect(readCuts(definition.source)!.points).toEqual([0, 0.25, 0.7, 0.75]);
});

it('adds a cut with a double-click, removes one with a double-click on its marker or Delete, and keeps the first', async () => {
  const { mouse, canvas, actions, definition } = await setup([0, 0.5]);
  mouse('dblclick', 250); expect(readCuts(definition.source)!.points).toEqual([0, 0.25, 0.5]);
  mouse('dblclick', 251); expect(readCuts(definition.source)!.points).toEqual([0, 0.5]);
  mouse('dblclick', 1); expect(actions.edit).toHaveBeenCalledTimes(2); // too near the start to add
  mouse('pointerdown', 500); mouse('pointerup', 500);
  canvas.dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true, cancelable: true }));
  expect(readCuts(definition.source)!.points).toEqual([0, 0.5]); // a sample keeps at least two pieces
});

it('moves the cuts to the hits it finds', async () => {
  const { root, actions, definition } = await setup(4);
  [...root.querySelectorAll('button')].find(b => b.textContent === 'Find hits')!.click();
  await vi.waitFor(() => expect(readCuts(definition.source)!.points).toEqual([0, 0.1, 0.4]));
  expect(actions.detect).toHaveBeenCalledWith('samples/amen.wav', 48, 0.5);
  root.querySelector<HTMLInputElement>('[aria-label="Equal pieces"]')!.value = '8';
  [...root.querySelectorAll('button')].find(b => b.textContent === 'Cut equally')!.click();
  expect(readCuts(definition.source)!.points).toHaveLength(8);
});

it('edits and restores the duration multiplier for whole samples and slices', async () => {
  for (const cuts of [null, 4]) {
    const { root, definition, actions, panel } = await setup(cuts);
    const stretch = root.querySelector<HTMLInputElement>('[aria-label="Time stretch"]')!;
    expect(stretch.disabled).toBe(false);
    stretch.value = '2'; stretch.dispatchEvent(new Event('change'));
    expect(actions.edit).toHaveBeenCalledOnce();
    expect(definition.source).toContain('stretch: 0.25..4 = 2');
    expect(definition.source).toContain('stretch: stretch, stretch_mode: stretch_mode)');
    await panel.refresh(); expect(stretch.value).toBe('2');
    stretch.value = '0'; stretch.dispatchEvent(new Event('change'));
    expect(actions.edit).toHaveBeenCalledOnce();
  }
});


it('selects the stretch algorithm without changing duration or cuts', async () => {
  const { root, definition, actions, panel } = await setup(4);
  const mode = root.querySelector<HTMLSelectElement>('[aria-label="Stretch algorithm"]')!;
  expect([...mode.options].map(option => option.textContent)).toEqual(['Granular', 'WSOLA', 'Transient WSOLA']);
  expect(mode.value).toBe('0');
  mode.value = '1'; mode.dispatchEvent(new Event('change'));
  expect(actions.edit).toHaveBeenCalledOnce();
  expect(definition.source).toContain('stretch_mode: 0..2 = 1');
  expect(definition.source).toContain('stretch_mode: stretch_mode)');
  expect(definition.source).toContain('stretch: 0.25..4 = 1');
  expect(readCuts(definition.source)!.points).toEqual([0, 0.25, 0.5, 0.75]);
  await panel.refresh(); expect(mode.value).toBe('1');
  mode.value = '0'; mode.dispatchEvent(new Event('change'));
  expect(definition.source).toContain('stretch_mode: 0..2 = 0');
});

it('adds the algorithm control to an existing generated sampler when selected', async () => {
  const { root, definition, panel } = await setup(null);
  definition.source = definition.source.replace(',\n  stretch_mode: 0..2 = 0', '').replaceAll(', stretch_mode: stretch_mode', '');
  await panel.refresh();
  const mode = root.querySelector<HTMLSelectElement>('[aria-label="Stretch algorithm"]')!;
  mode.value = '1'; mode.dispatchEvent(new Event('change'));
  expect(definition.source).toContain('stretch_mode: 0..2 = 1');
  expect(definition.source).toContain('stretch_mode: stretch_mode)');
  expect(definition.source.match(/stretch_mode: 0\.\.2/g)).toHaveLength(1);
});

it('fits the sample to the tempo from the number of beats it lasts, refits when that changes, and turns the fit off', async () => {
  const { root, actions, definition, text } = await setup(null);
  expect(text()).not.toContain('recorded at');
  const box = root.querySelector<HTMLInputElement>('[aria-label="Fit to tempo"]')!, beats = root.querySelector<HTMLInputElement>('[aria-label="Beats in the sample"]')!;
  const tick = (on: boolean) => { box.checked = on; box.dispatchEvent(new Event('change')); };
  const type = (value: string) => { beats.value = value; beats.dispatchEvent(new Event('input')); beats.dispatchEvent(new Event('change')); };
  expect(box.checked).toBe(false);
  // the sample lasts one second: 2 beats is 120 bpm, and 4 beats 240 bpm
  beats.value = '2'; beats.dispatchEvent(new Event('input')); tick(true);
  expect(definition.source).toContain('from_bpm: 0..400 = 120'); expect(text()).toContain('recorded at 120 bpm'); expect(box.checked).toBe(true);
  type('4'); expect(definition.source).toContain('from_bpm: 0..400 = 240'); expect(beats.value).toBe('4'); // a fitted sample follows the beat count
  actions.edit.mockClear(); type('1000'); expect(actions.edit).not.toHaveBeenCalled(); expect(text()).toContain('check the number of beats');
  tick(false); expect(definition.source).toContain('from_bpm: 0..400 = 0'); expect(text()).not.toContain('recorded at'); expect(box.checked).toBe(false);
  expect(actions.detect).not.toHaveBeenCalled(); // a beat count that was entered is never worked out again
});

it('chooses how a sampler plays, and sets and drags sustain loop points on whole samples without creating slices', async () => {
  const { root, definition, panel, mouse, actions } = await setup(null);
  const mode = root.querySelector<HTMLSelectElement>('[aria-label="Playback"]')!;
  const start = root.querySelector<HTMLInputElement>('[aria-label="Loop start percent"]')!;
  const end = root.querySelector<HTMLInputElement>('[aria-label="Loop end percent"]')!;
  const crossfade = root.querySelector<HTMLInputElement>('[aria-label="Loop crossfade milliseconds"]')!;
  expect(crossfade.disabled).toBe(true);
  expect(mode.disabled).toBe(false); expect(start.disabled).toBe(true);
  expect(mode.value).toBe('one-shot'); mode.value = 'loop'; mode.dispatchEvent(new Event('change'));
  expect(definition.source).toContain('gate: 0..1 = 1');
  expect(crossfade.disabled).toBe(false);
  crossfade.value = '12.5'; crossfade.dispatchEvent(new Event('change'));
  expect(definition.source).toContain('loop_crossfade: sec = 12.5ms');
  start.value = '20'; start.dispatchEvent(new Event('change'));
  end.value = '80'; end.dispatchEvent(new Event('change'));
  expect(definition.source).toContain('.loop(forward)');
  expect(definition.source).toContain('loop_start: 0..1 = 0.2');
  expect(definition.source).toContain('loop_end: 0..1 = 0.8');
  expect(definition.source).not.toContain('.slices(');
  actions.edit.mockClear(); mouse('pointerdown', 200); mouse('pointermove', 300);
  expect(actions.edit).not.toHaveBeenCalled(); mouse('pointerup', 300);
  expect(actions.edit).toHaveBeenCalledOnce(); expect(definition.source).toContain('loop_start: 0..1 = 0.3');
  await panel.refresh(); expect(start.value).toBe('30'); expect(end.value).toBe('80'); expect(crossfade.value).toBe('12.5');
  start.value = '90'; start.dispatchEvent(new Event('change'));
  expect(definition.source).toContain('loop_start: 0..1 = 0.3');
  expect(start.closest<HTMLElement>('.prop-group')!.hidden).toBe(false);
  mode.value = 'one-shot'; mode.dispatchEvent(new Event('change'));
  expect(start.closest<HTMLElement>('.prop-group')!.hidden).toBe(true); // the loop's controls show only for a loop
  expect(definition.source).toContain('.loop(none)'); expect(definition.source).toContain('gate: 0..1 = 0'); expect(start.disabled).toBe(true);
  mode.value = 'hold'; mode.dispatchEvent(new Event('change')); expect(definition.source).toContain('gate: 0..1 = 1'); expect(start.disabled).toBe(true);
  const sliced = await setup(4);
  const slicedMode = sliced.root.querySelector<HTMLSelectElement>('[aria-label="Playback"]')!;
  expect(slicedMode.disabled).toBe(false); expect([...slicedMode.options].filter(o => o.disabled)).toEqual([]); // a cut sample can be looped too, once it plays as one sample
});

it('saves the transient algorithm and keeps it when fitting the sample', async () => {
  const { root, definition, panel } = await setup(null);
  const mode = root.querySelector<HTMLSelectElement>('[aria-label="Stretch algorithm"]')!;
  mode.value = '2'; mode.dispatchEvent(new Event('change'));
  expect(definition.source).toContain('stretch_mode: 0..2 = 2');
  await panel.refresh(); expect(mode.value).toBe('2');
  pressIn(root)('Fit to tempo');
  await vi.waitFor(() => expect(definition.source).toContain('from_bpm'));
  expect(definition.source).toContain('stretch_mode: 0..2 = 2');
});

it('works out the beat count from the hits the first time it fits, and keeps it for later fits', async () => {
  const { root, definition, actions, panel, text } = await setup(4);
  actions.detect.mockResolvedValue([0, 0.25, 0.5, 0.75]);
  const beats = root.querySelector<HTMLInputElement>('[aria-label="Beats in the sample"]')!;
  const press = pressIn(root);
  press('Fit to tempo');
  await vi.waitFor(() => expect(definition.source).toContain('from_bpm: 0..400 = 120'));
  expect(actions.detect).toHaveBeenCalledWith('samples/amen.wav', 256); expect(beats.value).toBe('2'); expect(text()).toContain('half or double');
  expect(readCuts(definition.source)!.points).toEqual([0, 0.25, 0.5, 0.75]);
  press('Original'); actions.detect.mockClear(); await panel.refresh(); press('Fit to tempo');
  expect(definition.source).toContain('from_bpm: 0..400 = 120'); expect(actions.detect).not.toHaveBeenCalled();
});

it('does not fit when the beat count cannot be worked out, and fits once one is entered', async () => {
  const { root, definition, actions, text } = await setup(null);
  const press = pressIn(root);
  const beats = root.querySelector<HTMLInputElement>('[aria-label="Beats in the sample"]')!;
  actions.detect.mockResolvedValue([0, 0.5]); press('Fit to tempo');
  await vi.waitFor(() => expect(text()).toContain('Not enough regular hits'));
  expect(actions.edit).not.toHaveBeenCalled(); expect(definition.source).not.toContain('from_bpm: 0..400 = 120');
  beats.value = '2'; beats.dispatchEvent(new Event('input')); press('Fit to tempo'); expect(definition.source).toContain('from_bpm: 0..400 = 120');
  press('Original'); actions.detect.mockRejectedValue(new Error('read failed')); actions.edit.mockClear();
  beats.value = '4'; beats.dispatchEvent(new Event('input')); press('Fit to tempo'); expect(definition.source).toContain('from_bpm: 0..400 = 240'); // a typed count is used without detecting
});

it('discards an estimate if the instrument changes while hits are being detected', async () => {
  const { root, actions, panel, text } = await setup(null);
  let resolve!: (hits: number[]) => void;
  actions.detect.mockImplementation(() => new Promise<number[]>(done => { resolve = done; }));
  pressIn(root)('Fit to tempo');
  const other = addInstrument(newProject(), 'samples/other.wav');
  vi.spyOn(actions, 'current').mockReturnValue(other);
  await panel.refresh(); resolve([0, 0.25, 0.5, 0.75]);
  await new Promise(done => setTimeout(done));
  expect(root.querySelector<HTMLInputElement>('[aria-label="Beats in the sample"]')!.value).toBe('4');
  expect(text()).not.toContain('Fitted as'); expect(actions.edit).not.toHaveBeenCalled();
});

it('cuts an uncut sampler into a new instrument, from Find hits or Cut equally, and leaves the sampler as it was', async () => {
  const { root, actions, definition } = await setup(null);
  const press = pressIn(root);
  const before = definition.source;
  press('Find hits'); await vi.waitFor(() => expect(actions.cut).toHaveBeenCalledWith(definition, [0, 0.1, 0.4]));
  press('Cut equally'); expect(actions.cut).toHaveBeenLastCalledWith(definition, Array.from({ length: 16 }, (_, i) => i / 16));
  expect(actions.edit).not.toHaveBeenCalled(); expect(definition.source).toBe(before);
});

it('passes the hit sensitivity to the detection, 0 to 100 as a share', async () => {
  const { root, actions } = await setup(4);
  const sensitivity = root.querySelector<HTMLInputElement>('[aria-label="Hit sensitivity percent"]')!;
  sensitivity.value = '80'; pressIn(root)('Find hits');
  await vi.waitFor(() => expect(actions.detect).toHaveBeenLastCalledWith('samples/amen.wav', 48, 0.8));
  sensitivity.value = '250'; pressIn(root)('Find hits');
  await vi.waitFor(() => expect(actions.detect).toHaveBeenLastCalledWith('samples/amen.wav', 48, 1));
});

it('sets the root pitch, release and interpolation from the settings column, and shows how the code has them', async () => {
  const { root, definition, panel } = await setup(null);
  const control = <T extends HTMLElement>(label: string) => root.querySelector<T>(`[aria-label="${label}"]`)!;
  const change = (element: HTMLInputElement | HTMLSelectElement, value: string) => { element.value = value; element.dispatchEvent(new Event('change')); };
  const note = control<HTMLSelectElement>('Root note'), cents = control<HTMLInputElement>('Finetune cents');
  expect(note.value).toBe('60'); expect(cents.value).toBe('0'); expect(control<HTMLInputElement>('Release milliseconds').value).toBe('10');
  change(note, '57'); expect(definition.source).toContain('.root(a3)');
  change(cents, '20'); expect(definition.source).toMatch(/\.root\(2\d\d\.\d+hz\)/); await panel.refresh(); expect(note.value).toBe('57'); expect(cents.value).toBe('20');
  change(cents, '80'); expect(cents.value).toBe('20'); // beyond half a semitone is another note
  change(control<HTMLInputElement>('Release milliseconds'), '250'); expect(definition.source).toContain('release: 0ms..2sec = 250ms');
  expect([...control<HTMLSelectElement>('Interpolation').options].map(o => o.value)).toEqual(['linear', 'cubic', 'sinc']);
  change(control<HTMLSelectElement>('Interpolation'), 'sinc'); expect(definition.source).toContain('.interp(sinc)');
  change(control<HTMLSelectElement>('Interpolation'), 'linear'); expect(definition.source).toContain('.interp(linear)'); expect(definition.source).not.toContain('sinc');
});

it('follows the tempo by stretching or repitching, and shows that choice only while the sample is fitted', async () => {
  const { root, definition } = await setup(null);
  const row = () => root.querySelector<HTMLSelectElement>('[aria-label="Fit by"]')!.closest<HTMLElement>('.prop')!, box = root.querySelector<HTMLInputElement>('[aria-label="Fit to tempo"]')!;
  expect(row().hidden).toBe(true);
  const beats = root.querySelector<HTMLInputElement>('[aria-label="Beats in the sample"]')!; beats.value = '2'; beats.dispatchEvent(new Event('input'));
  box.checked = true; box.dispatchEvent(new Event('change')); expect(row().hidden).toBe(false);
  const mode = root.querySelector<HTMLSelectElement>('[aria-label="Fit by"]')!; mode.value = 'repitch'; mode.dispatchEvent(new Event('change'));
  expect(definition.source).toContain('fit_mode: 0..1 = 1'); expect(definition.source).toContain('from_bpm: 0..400 = 120');
  box.checked = false; box.dispatchEvent(new Event('change')); expect(row().hidden).toBe(true);
});

it('retunes the root once so the sample lasts its beats at the song tempo, and not while it is already fitted', async () => {
  const { root, definition, actions, text } = await setup(null);
  const match = [...root.querySelectorAll('button')].find(b => b.textContent === 'Match tempo')!, beats = root.querySelector<HTMLInputElement>('[aria-label="Beats in the sample"]')!;
  // one second of sample lasting 2 beats is 120 bpm: in a 120 bpm song nothing changes, and at 240 it plays twice as fast, an octave down for the root
  beats.value = '2'; beats.dispatchEvent(new Event('input')); match.click(); await vi.waitFor(() => expect(text()).toContain('0.00 semitones')); expect(definition.source).toContain('.root(c4)');
  actions.tempo.mockReturnValue(240); match.click();
  await vi.waitFor(() => expect(definition.source).toContain('.root(c3)')); expect(text()).toContain('12.00 semitones higher (faster)');
  const box = root.querySelector<HTMLInputElement>('[aria-label="Fit to tempo"]')!; box.checked = true; box.dispatchEvent(new Event('change'));
  actions.edit.mockClear(); match.click(); await vi.waitFor(() => expect(text()).toContain('Untick Fit to tempo first')); expect(actions.edit).not.toHaveBeenCalled();
});

it('turns a cut instrument into one that plays its whole sample when Loop is chosen, after asking, and keeps the cuts if declined', async () => {
  const { root, definition, actions } = await setup(8);
  const mode = root.querySelector<HTMLSelectElement>('[aria-label="Playback"]')!;
  actions.uncut.mockResolvedValueOnce(false);
  mode.value = 'loop'; mode.dispatchEvent(new Event('change'));
  await vi.waitFor(() => expect(actions.uncut).toHaveBeenCalledWith(definition));
  await vi.waitFor(() => expect(mode.value).toBe('hold')); expect(definition.source).toContain('.slices(8)'); expect(actions.edit).not.toHaveBeenCalled();
  mode.value = 'ping-pong'; mode.dispatchEvent(new Event('change'));
  await vi.waitFor(() => expect(definition.source).toContain('.loop(bidirectional)'));
  expect(definition.source).not.toContain('.slices('); expect(definition.source).toContain('gate: 0..1 = 1'); expect(definition.source).toContain('amen_slices_sample(freq: freq');
  expect(root.querySelector<HTMLInputElement>('[aria-label="Loop start percent"]')!.disabled).toBe(false);
});

it('has a Play as one sample button only on a cut instrument, which asks first and then removes the cuts', async () => {
  const { root, definition, actions } = await setup(8);
  const button = () => [...root.querySelectorAll('button')].find(b => b.textContent === 'Play as one sample')!;
  expect(button().hidden).toBe(false);
  actions.uncut.mockResolvedValueOnce(false); button().click();
  await vi.waitFor(() => expect(actions.uncut).toHaveBeenCalledTimes(1)); expect(definition.source).toContain('.slices(8)');
  button().click();
  await vi.waitFor(() => expect(definition.source).not.toContain('.slices(')); expect(button().hidden).toBe(true);
  expect(definition.source).toContain('amen_slices_sample(freq: freq');
  const whole = await setup(null); expect([...whole.root.querySelectorAll('button')].find(b => b.textContent === 'Play as one sample')!.hidden).toBe(true);
});
