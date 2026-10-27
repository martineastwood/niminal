// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { SamplePanel, type SampleActions } from './waveform';
import { addInstrument, sliceInstrument } from './instruments';
import { readCuts } from './slices';
import { newProject } from './project';

afterEach(() => document.body.replaceChildren());

async function setup(cuts: number | number[] | null = 4) {
  const project = newProject(), sampler = addInstrument(project, 'samples/amen.wav'), definition = cuts === null ? sampler : sliceInstrument(project, sampler, cuts)!;
  const root = document.createElement('div'); document.body.append(root);
  const actions = {
    current: () => definition,
    loadPeaks: vi.fn(async () => ({ frames: 48000, rate: 48000, peaks: Array.from({ length: 200 }, (_, i) => (i % 2 ? 0.5 : -0.5)) })),
    edit: vi.fn((target: typeof definition, source: string) => { target.source = source; }),
    hear: vi.fn(),
    detect: vi.fn(async () => [0, 0.1, 0.4]),
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
  expect(uncut.text()).toContain('1.00 s'); expect(uncut.text()).not.toMatch(/\d pieces/); expect(uncut.text()).toContain('scissors');
  const synth = newProject().definitions[0], root = document.createElement('div'); document.body.append(root);
  const none = new SamplePanel(root, { current: () => synth, loadPeaks: vi.fn(), edit: vi.fn(), hear: vi.fn(), detect: vi.fn() });
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
  expect(actions.detect).toHaveBeenCalledWith('samples/amen.wav');
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

it('fits the sample to the tempo from the number of beats it lasts, and turns the fit off', async () => {
  const { root, actions, definition, text } = await setup(null);
  expect(text()).toContain('not fitted to the tempo');
  const press = (label: string) => [...root.querySelectorAll('button')].find(b => b.textContent === label)!.click();
  const beats = root.querySelector<HTMLInputElement>('[aria-label="Beats in the sample"]')!;
  // the sample lasts one second: 2 beats is 120 bpm, and 4 beats 240 bpm
  beats.value = '2'; press('Fit to tempo');
  expect(definition.source).toContain('from_bpm: 0..400 = 120'); expect(text()).toContain('fitted: recorded at 120 bpm');
  beats.value = '4'; press('Fit to tempo'); expect(definition.source).toContain('from_bpm: 0..400 = 240'); expect(beats.value).toBe('4');
  beats.value = '1000'; actions.edit.mockClear(); press('Fit to tempo'); expect(actions.edit).not.toHaveBeenCalled(); expect(text()).toContain('check the number of beats');
  press('Off'); expect(definition.source).toContain('from_bpm: 0..400 = 0'); expect(text()).toContain('not fitted');
});

it('sets and drags sustain loop points on whole samples without creating slices', async () => {
  const { root, definition, panel, mouse, actions } = await setup(null);
  const mode = root.querySelector<HTMLSelectElement>('[aria-label="Sample loop mode"]')!;
  const start = root.querySelector<HTMLInputElement>('[aria-label="Loop start percent"]')!;
  const end = root.querySelector<HTMLInputElement>('[aria-label="Loop end percent"]')!;
  const crossfade = root.querySelector<HTMLInputElement>('[aria-label="Loop crossfade milliseconds"]')!;
  expect(crossfade.disabled).toBe(true);
  expect(mode.disabled).toBe(false); expect(start.disabled).toBe(true);
  mode.value = 'forward'; mode.dispatchEvent(new Event('change'));
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
  mode.value = 'none'; mode.dispatchEvent(new Event('change'));
  expect(definition.source).toContain('.loop(none)'); expect(start.disabled).toBe(true);
  const sliced = await setup(4);
  expect(sliced.root.querySelector<HTMLSelectElement>('[aria-label="Sample loop mode"]')!.disabled).toBe(true);
});

it('saves the transient algorithm and keeps it when fitting the sample', async () => {
  const { root, definition, panel } = await setup(null);
  const mode = root.querySelector<HTMLSelectElement>('[aria-label="Stretch algorithm"]')!;
  mode.value = '2'; mode.dispatchEvent(new Event('change'));
  expect(definition.source).toContain('stretch_mode: 0..2 = 2');
  await panel.refresh(); expect(mode.value).toBe('2');
  [...root.querySelectorAll('button')].find(button => button.textContent === 'Fit to tempo')!.click();
  expect(definition.source).toContain('stretch_mode: 0..2 = 2');
});

it('suggests beats without fitting or editing cuts, and lets users halve/double before fitting', async () => {
  const { root, definition, actions, panel, text } = await setup(4);
  actions.detect.mockResolvedValue([0, 0.25, 0.5, 0.75]);
  const button = [...root.querySelectorAll('button')].find(b => b.textContent === 'Estimate beats')!;
  const beats = root.querySelector<HTMLInputElement>('[aria-label="Beats in the sample"]')!;
  button.click();
  await vi.waitFor(() => expect(beats.value).toBe('2'));
  expect(actions.detect).toHaveBeenCalledWith('samples/amen.wav', 256);
  expect(text()).toContain('120 bpm'); expect(actions.edit).not.toHaveBeenCalled();
  expect(readCuts(definition.source)!.points).toEqual([0, 0.25, 0.5, 0.75]);
  root.querySelector<HTMLButtonElement>('[aria-label="Halve beat count"]')!.click();
  expect(beats.value).toBe('1'); expect(text()).toContain('60 bpm');
  await panel.refresh(); expect(beats.value).toBe('1');
  root.querySelector<HTMLButtonElement>('[aria-label="Double beat count"]')!.click();
  expect(beats.value).toBe('2'); expect(actions.edit).not.toHaveBeenCalled();
  [...root.querySelectorAll('button')].find(b => b.textContent === 'Fit to tempo')!.click();
  expect(definition.source).toContain('from_bpm: 0..400 = 120');
});

it('leaves a fitted sample unchanged when estimation fails or there are too few hits', async () => {
  const { root, definition, actions, text } = await setup(null);
  const press = (label: string) => [...root.querySelectorAll('button')].find(b => b.textContent === label)!.click();
  const beats = root.querySelector<HTMLInputElement>('[aria-label="Beats in the sample"]')!;
  beats.value = '2'; press('Fit to tempo'); actions.edit.mockClear();
  actions.detect.mockResolvedValue([0, 0.5]); press('Estimate beats');
  await vi.waitFor(() => expect(text()).toContain('Not enough regular hits'));
  expect(beats.value).toBe('2'); expect(actions.edit).not.toHaveBeenCalled();
  expect(definition.source).toContain('from_bpm: 0..400 = 120');
  actions.detect.mockRejectedValue(new Error('read failed')); press('Estimate beats');
  await vi.waitFor(() => expect(text()).toContain('Could not estimate the tempo'));
  expect(beats.value).toBe('2'); expect(actions.edit).not.toHaveBeenCalled();
});

it('discards an estimate if the instrument changes while hits are being detected', async () => {
  const { root, actions, panel, text } = await setup(null);
  let resolve!: (hits: number[]) => void;
  actions.detect.mockImplementation(() => new Promise<number[]>(done => { resolve = done; }));
  const button = [...root.querySelectorAll('button')].find(b => b.textContent === 'Estimate beats')!;
  button.click(); expect(button.disabled).toBe(true);
  const other = addInstrument(newProject(), 'samples/other.wav');
  vi.spyOn(actions, 'current').mockReturnValue(other);
  await panel.refresh(); resolve([0, 0.25, 0.5, 0.75]);
  await vi.waitFor(() => expect(button.disabled).toBe(false));
  expect(root.querySelector<HTMLInputElement>('[aria-label="Beats in the sample"]')!.value).toBe('4');
  expect(text()).not.toContain('Estimated'); expect(actions.edit).not.toHaveBeenCalled();
});

it('keeps a proposed beat count through refresh without changing an existing fit', async () => {
  const { root, actions, definition, panel } = await setup(null);
  const press = (label: string) => [...root.querySelectorAll('button')].find(b => b.textContent === label)!.click();
  const beats = root.querySelector<HTMLInputElement>('[aria-label="Beats in the sample"]')!;
  beats.value = '3'; press('Fit to tempo'); actions.edit.mockClear();
  actions.detect.mockResolvedValue([0, 0.25, 0.5, 0.75]); press('Estimate beats');
  await vi.waitFor(() => expect(beats.value).toBe('2'));
  await panel.refresh(); expect(beats.value).toBe('2');
  expect(definition.source).toContain('from_bpm: 0..400 = 180'); expect(actions.edit).not.toHaveBeenCalled();
  press('Fit to tempo'); expect(definition.source).toContain('from_bpm: 0..400 = 120');
});
