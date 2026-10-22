// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { PatternGrid } from './grid';
import { newProject, emptyFxLane, blankColumn } from './project';

Element.prototype.scrollIntoView = () => {};
// happy-dom does not expose the browser's Option convenience constructor.
vi.stubGlobal('Option', function (text: string, value: string) {
  const option = document.createElement('option'); option.textContent = text; option.value = value; return option;
});
afterEach(() => document.body.replaceChildren());

function setup() {
  const parent = document.createElement('div'); parent.tabIndex = 0; document.body.append(parent);
  const project = newProject(), edited = vi.fn(), audition = vi.fn(), play = vi.fn();
  const grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, edited, vi.fn(), audition, play);
  grid.recording = true;
  grid.render();
  const key = (key: string, options: KeyboardEventInit = {}) => parent.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...options }));
  const click = (row: number, track: number) => parent.querySelector<HTMLElement>(`#cell-${row}-${track}`)!.click();
  return { parent, project, grid, key, click, edited, audition, play };
}

function clipboard(parent: HTMLElement, type: 'copy' | 'paste', text = '') {
  let copied = '';
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(event, 'clipboardData', { value: { getData: () => text, setData: (_: string, value: string) => { copied = value; } } });
  parent.dispatchEvent(event);
  return copied;
}

it('enters and auditions a note, then undoes/redoes it through grid keyboard shortcuts', () => {
  const { project, key, click, audition, edited } = setup();
  click(1, 0); key('z');
  expect(project.patterns[0].notes[0][0].notes[1]).toBe('C-4');
  expect(audition).toHaveBeenCalledWith('tone', 'C-4', 0);
  key('z', { metaKey: true }); expect(project.patterns[0].notes[0][0].notes[1]).toBeNull();
  key('z', { metaKey: true, shiftKey: true }); expect(project.patterns[0].notes[0][0].notes[1]).toBe('C-4');
  expect(edited).toHaveBeenCalledTimes(3);
});

it('copies a rectangular selection and treats a multi-cell paste as one undoable edit', () => {
  const { parent, project, key, click } = setup();
  click(0, 0); key('ArrowDown', { shiftKey: true }); key('ArrowDown', { shiftKey: true }); key('ArrowRight', { shiftKey: true }); key('ArrowRight', { shiftKey: true });
  expect(clipboard(parent, 'copy')).toBe('C-4\t---\t---\n---\t---\t---\n---\t---\t---');
  key('Delete'); expect(project.patterns[0].notes[0][0].notes[0]).toBeNull();
  key('z', { ctrlKey: true }); expect(project.patterns[0].notes[0][0].notes[0]).toBe('C-4');
  clipboard(parent, 'paste', 'F#3\t---\tA-3\n---\t---\tC-4');
  expect(project.patterns[0].notes[0][0].notes[0]).toBe('F#3'); expect(project.patterns[0].notes[1][0].notes[0]).toBe('A-3');
  expect(project.patterns[0].notes[1][0].notes[1]).toBe('C-4');
  key('z', { ctrlKey: true });
  expect(project.patterns[0].notes[0][0].notes[0]).toBe('C-4'); expect(project.patterns[0].notes[1][0].notes[0]).toBeNull(); expect(project.patterns[0].notes[1][0].notes[1]).toBeNull();
});

it('clips a pasted block at the pattern edge and ignores malformed or empty clipboard data', () => {
  const { parent, project, click, grid } = setup();
  click(31, 3); clipboard(parent, 'paste', 'D-3\t-6 dB\nF-3\t6 dB');
  expect(project.patterns[0].notes[3][0].notes[31]).toBe('D-3');
  expect(project.patterns[0].notes[3][0].notes).toHaveLength(32);
  clipboard(parent, 'paste', 'not notes'); clipboard(parent, 'paste', '');
  expect(project.patterns[0].notes[3][0].notes[31]).toBe('D-3');
  grid.undo(); expect(project.patterns[0].notes[3][0].notes[31]).toBeNull();
});

it('keeps selector typing out of the note grid and space playback local to the grid', () => {
  const { parent, project, key, play } = setup();
  const select = parent.querySelector('select')!;
  select.dispatchEvent(new KeyboardEvent('keydown', { key: 's', bubbles: true }));
  expect(project.patterns[0].notes[0][0].notes[0]).toBe('C-4');
  key(' '); expect(play).toHaveBeenCalledTimes(1);
  const code = document.createElement('textarea'); document.body.append(code);
  code.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true }));
  expect(play).toHaveBeenCalledTimes(1);
});

it('keeps note undo history independent for each pattern and supports longer patterns', () => {
  const parent = document.createElement('div'); document.body.append(parent);
  const project = newProject();
  const second = { id: 'second', name: 'Second', rows: 64, fx: Array.from({ length: 4 }, () => [emptyFxLane(64)]), notes: Array.from({ length: 4 }, () => [blankColumn(64)]) };
  project.patterns.push(second);
  let pattern = project.patterns[0];
  const grid = new PatternGrid(parent, () => project, () => pattern, () => 4, vi.fn(), vi.fn(), vi.fn(), vi.fn());
  grid.recording = true;
  const key = (key: string) => parent.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
  grid.render(); key('s'); // change first pattern's C to C#
  pattern = second; grid.render();
  parent.querySelector<HTMLElement>('#cell-63-0')!.click(); key('x');
  expect(second.notes[0][0].notes[63]).toBe('D-4');
  pattern = project.patterns[0]; grid.render(); grid.undo();
  expect(project.patterns[0].notes[0][0].notes[0]).toBe('C-4');
  expect(second.notes[0][0].notes[63]).toBe('D-4');
  pattern = second; grid.render(); grid.undo(); expect(second.notes[0][0].notes[63]).toBeNull();
});

it('opens track devices without entering notes and navigates beyond the starter tracks', () => {
  const parent = document.createElement('div'); document.body.append(parent);
  const project = newProject(), selected = vi.fn(), open = vi.fn();
  for (let i = 4; i < 8; i++) {
    project.tracks.push({ name: `Track ${i + 1}`, instrument: 'tone', fx: [], pan: 0, volume_db: 0, muted: false });
    project.patterns[0].notes.push([blankColumn(32)]); project.patterns[0].fx.push([emptyFxLane(32)]);
  }
  const grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, vi.fn(), vi.fn(), vi.fn(), vi.fn(), selected, open);
  grid.recording = true;
  grid.render();
  parent.querySelector<HTMLButtonElement>('[aria-label="Show Track 8 effects"]')!.click();
  expect(parent.querySelector('#cell-0-7')?.getAttribute('aria-selected')).toBe('true');
  expect(grid.activeTrack).toBe(7); expect(open).toHaveBeenCalledWith(7, 'effects', false);
  parent.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', bubbles: true, cancelable: true }));
  expect(project.patterns[0].notes[7][0].notes[0]).toBe('C-4');
  parent.querySelector<HTMLButtonElement>('[aria-label="Mute Track 8"]')!.click();
  expect(project.tracks[7].muted).toBe(true);
  parent.querySelector<HTMLButtonElement>('[aria-label="Edit Track 8 instrument"]')!.click();
  expect(open).toHaveBeenLastCalledWith(7, 'instrument', true);
  parent.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true }));
  expect(grid.activeTrack).toBe(0); expect(selected).toHaveBeenLastCalledWith(0);
});

it('advances notes by the entry step, wraps, and supports step zero', () => {
  const parent = document.createElement('div'); document.body.append(parent);
  const project = newProject(); let step = 4;
  const grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, vi.fn(), vi.fn(), vi.fn(), vi.fn(), vi.fn(), vi.fn(), () => step);
  grid.recording = true;
  grid.render();
  const key = () => parent.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', bubbles: true, cancelable: true }));
  key(); expect(parent.getAttribute('aria-activedescendant')).toBe('cell-4-0');
  parent.querySelector<HTMLElement>('#cell-28-0')!.click(); key();
  expect(parent.getAttribute('aria-activedescendant')).toBe('cell-0-0');
  step = 0; key(); expect(parent.getAttribute('aria-activedescendant')).toBe('cell-0-0');
});

it('selects a rectangle by dragging without entering or erasing notes', () => {
  const { parent, project } = setup();
  const before = JSON.stringify(project.patterns);
  const start = parent.querySelector<HTMLElement>('#cell-1-0')!;
  const end = parent.querySelector<HTMLElement>('#cell-4-2')!;
  vi.spyOn(document, 'elementFromPoint').mockReturnValue(end);
  start.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 0, pointerId: 1 }));
  parent.dispatchEvent(new PointerEvent('pointermove', { bubbles: true, buttons: 1, pointerId: 1, clientX: 10, clientY: 20 }));
  parent.dispatchEvent(new PointerEvent('pointerup', { bubbles: true, pointerId: 1 }));
  expect(parent.querySelectorAll('[role=gridcell][aria-selected=true]')).toHaveLength(20);
  expect(clipboard(parent, 'copy').split('\n')).toHaveLength(4);
  expect(JSON.stringify(project.patterns)).toBe(before);
  vi.restoreAllMocks();
});

it('edits readable values, advances by step, and undoes without auditioning notes', () => {
  const { parent, project, audition, key } = setup();
  parent.querySelector<HTMLElement>('#fx-2-0-0')!.click();
  const input = parent.querySelector<HTMLInputElement>('.lane-value')!; input.value = '-6 dB';
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  expect(project.patterns[0].fx[0][0].values[2]).toBe('-6');
  expect(parent.getAttribute('aria-activedescendant')).toBe('fx-3-0-0');
  expect(audition).not.toHaveBeenCalled();
  key('z', { metaKey: true }); expect(project.patterns[0].fx[0][0].values[2]).toBeNull();
  key('z', { metaKey: true, shiftKey: true }); expect(project.patterns[0].fx[0][0].values[2]).toBe('-6');
});

it('copies and pastes notes with FX as one undoable rectangle and rejects mismatched data', () => {
  const { parent, project, key, click } = setup();
  project.patterns[0].fx[0][0].values[0] = '0';
  click(0, 0); key('ArrowRight', { shiftKey: true });
  expect(clipboard(parent, 'copy')).toBe('C-4\t0 db');
  click(1, 0); clipboard(parent, 'paste', 'D-4\t6 dB');
  expect(project.patterns[0].notes[0][0].notes[1]).toBe('D-4'); expect(project.patterns[0].fx[0][0].values[1]).toBe('6');
  key('z', { metaKey: true });
  expect(project.patterns[0].notes[0][0].notes[1]).toBeNull(); expect(project.patterns[0].fx[0][0].values[1]).toBeNull();
  clipboard(parent, 'paste', '-6'); expect(project.patterns[0].notes[0][0].notes[1]).toBeNull();
});

it('keeps non-empty FX columns when asked to hide them', () => {
  const { parent, project } = setup();
  parent.querySelector<HTMLButtonElement>('[aria-label="Add Lead parameter lane"]')!.click();
  expect(project.patterns[0].fx[0]).toHaveLength(2);
  project.patterns[0].fx[0][1].values[4] = '6';
  parent.querySelector<HTMLButtonElement>('[aria-label="Hide Lead parameter lane"]')!.click();
  expect(project.patterns[0].fx[0]).toHaveLength(2);
  project.patterns[0].fx[0][1].values[4] = null;
  parent.querySelector<HTMLButtonElement>('[aria-label="Hide Lead parameter lane"]')!.click();
  expect(project.patterns[0].fx[0]).toHaveLength(1);
});

it('renames a track inline without interpreting typing as notes, and cancels with Escape', () => {
  const { parent, project, edited } = setup();
  const rename = () => {
    const name = parent.querySelector<HTMLButtonElement>('.track-name')!;
    name.click(); // The first click must keep the target alive for a native double-click.
    expect(name.isConnected).toBe(true);
    name.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    return parent.querySelector<HTMLInputElement>('.track-rename')!;
  };
  let input = rename(); input.value = '  Echo lead  ';
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  expect(project.tracks[0].name).toBe('Echo lead'); expect(edited).toHaveBeenCalledOnce();
  input = rename(); input.value = 'Discard'; input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  expect(project.tracks[0].name).toBe('Echo lead');
  input = rename(); input.value = '   '; input.blur();
  expect(project.tracks[0].name).toBe('Echo lead');
  expect(project.patterns[0].notes[0][0].notes[0]).toBe('C-4');
});

it('records a slider into its matching track lane without replacing another parameter', () => {
  const { project, grid } = setup();
  grid.writeParameter(1, { kind: 'pan' }, 0.25);
  expect(project.patterns[0].fx[1][1].target).toEqual({ kind: 'pan' });
  expect(project.patterns[0].fx[1][1].values[0]).toBe('0.25');
  expect(project.patterns[0].fx[0][0].values[0]).toBeNull();
  grid.writeParameter(1, { kind: 'pan' }, -0.5);
  expect(project.patterns[0].fx[1]).toHaveLength(2);
  expect(project.patterns[0].fx[1][1].values[1]).toBe('-0.5');
});

it('adjusts values with one undoable vertical drag and cancels invalid text', () => {
  const { parent, project, grid } = setup();
  project.patterns[0].fx[0][0].values[0] = '-6'; grid.render();
  const handle = parent.querySelector<HTMLElement>('#fx-0-0-0 .value-drag')!;
  handle.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerId: 1, clientY: 100 }));
  handle.dispatchEvent(new PointerEvent('pointermove', { bubbles: true, pointerId: 1, clientY: 90 }));
  handle.dispatchEvent(new PointerEvent('pointerup', { bubbles: true, pointerId: 1, clientY: 90 }));
  expect(Number(project.patterns[0].fx[0][0].values[0])).toBeCloseTo(-2.7);
  grid.undo(); expect(project.patterns[0].fx[0][0].values[0]).toBe('-6');
  parent.querySelector<HTMLElement>('#fx-0-0-0')!.click(); const input = parent.querySelector<HTMLInputElement>('.lane-value')!;
  input.value = '100 dB'; input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  expect(project.patterns[0].fx[0][0].values[0]).toBe('-6');
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); expect(parent.querySelector('.lane-value')).toBeNull();
});

it('only previews notes until record mode is on, and marks the selected track', () => {
  const { parent, project, grid, key, edited, audition } = setup();
  grid.recording = false; grid.selectTrack(0);
  project.tracks[0].instrument = 'lead';
  const original = project.patterns[0].notes[0][0].notes[0];
  key('s');
  expect(audition).toHaveBeenCalledTimes(1);
  expect(project.patterns[0].notes[0][0].notes[0]).toBe(original); expect(edited).not.toHaveBeenCalled();
  key('Delete'); parent.querySelector<HTMLElement>('#cell-0-0')!.click();
  expect(edited).not.toHaveBeenCalled();
  const changed = vi.fn(); grid.modeChanged = changed;
  key('Escape'); expect(grid.recording).toBe(true); expect(changed).toHaveBeenCalledWith(true);
  key('s'); expect(project.patterns[0].notes[0][0].notes[0]).toBe('C#4');
  key('Escape'); expect(grid.recording).toBe(false);
  expect(document.querySelector('.track-frame')).not.toBeNull();
});

it('lets a track pick its colour from the strip above its header', () => {
  const { parent, project, edited } = setup();
  expect(project.tracks[1].colour).toBeUndefined();
  parent.querySelector<HTMLElement>('.track-header[data-track="1"] .track-colour')!.click();
  document.querySelector<HTMLElement>('[aria-label="Colour #8f8fd8"]')!.click();
  expect(project.tracks[1].colour).toBe('#8f8fd8'); expect(edited).toHaveBeenCalled();
  expect(parent.querySelector<HTMLElement>('.track-header[data-track="1"]')!.style.getPropertyValue('--track-colour')).toBe('#8f8fd8');
  expect(document.querySelector('.colour-popover')).toBeNull();
  parent.querySelector<HTMLElement>('.track-header[data-track="1"] .track-colour')!.click();
  [...document.querySelectorAll<HTMLElement>('.swatch-reset')].at(-1)!.click();
  expect(project.tracks[1].colour).toBeUndefined();
});

it('enters OFF, and plain numbers in the sub-columns, shown as people count them', () => {
  const { parent, project, grid, key, click } = setup();
  project.tracks[0].show = { inst: true, vol: true, pan: true, delay: true }; grid.render();
  const column = project.patterns[0].notes[0][0], cell = (row: number, kind: string) => parent.querySelector<HTMLElement>(`#cell-${row}-0-0-${kind}`)!;
  const type = (row: number, kind: string, text: string) => {
    cell(row, kind).click(); const input = parent.querySelector<HTMLInputElement>('.lane-value')!; input.value = text;
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  };
  click(1, 0); key('1'); expect(column.notes[1]).toBe('OFF'); expect(parent.querySelector('#cell-1-0')!.textContent).toBe('OFF');
  type(0, 'vol', '50'); expect(column.vol[0]).toBe('64'); expect(cell(0, 'vol').textContent).toBe('50');
  type(1, 'vol', '100'); expect(column.vol[1]).toBe('128'); type(2, 'vol', '101'); expect(column.vol[2]).toBeNull();
  type(2, 'pan', '-50'); expect(column.pan[2]).toBe('0'); expect(cell(2, 'pan').textContent).toBe('-50');
  type(3, 'pan', '+25'); expect(column.pan[3]).toBe('96'); expect(cell(3, 'pan').textContent).toBe('+25');
  type(4, 'pan', 'c'); expect(column.pan[4]).toBe('64'); expect(cell(4, 'pan').textContent).toBe('C'); type(5, 'pan', '51'); expect(column.pan[5]).toBeNull();
  type(5, 'delay', '50'); expect(column.delay[5]).toBe('128'); expect(cell(5, 'delay').textContent).toBe('50'); type(6, 'delay', '100'); expect(column.delay[6]).toBeNull();
  type(7, 'inst', '0'); expect(column.inst[7]).toBe(project.definitions.filter(d => d.kind === 'instr')[0].id); expect(cell(7, 'inst').textContent).toBe('00');
  type(8, 'inst', '7'); expect(column.inst[8]).toBeNull();
  // Typing a digit on a selected cell starts the same editor.
  cell(9, 'vol').click(); parent.querySelector<HTMLInputElement>('.lane-value')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  key('7'); const input = parent.querySelector<HTMLInputElement>('.lane-value')!; expect(input.value).toBe('7');
  input.value = '75'; input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); expect(column.vol[9]).toBe('96');
  grid.undo(); expect(column.vol[9]).toBeNull();
});

it('adds note columns to every pattern and shows or hides sub-columns from the track menu', () => {
  const { parent, project, grid } = setup();
  const second = { id: 'second', name: 'Second', rows: 16, fx: Array.from({ length: 4 }, () => [emptyFxLane(16)]), notes: Array.from({ length: 4 }, () => [blankColumn(16)]) };
  project.patterns.push(second); grid.render();
  parent.querySelector<HTMLButtonElement>('[aria-label="Lead columns"]')!.click();
  document.querySelector<HTMLButtonElement>('[aria-label="More note columns"]')!.click();
  expect(project.patterns.map(p => p.notes[0].length)).toEqual([2, 2]); expect(second.notes[0][1].notes).toHaveLength(16);
  expect(parent.querySelectorAll('.track-header[data-track="0"] .col-label')).toHaveLength(2);
  const box = document.querySelector<HTMLInputElement>('[aria-label="Show Vol column"]')!; box.checked = true; box.dispatchEvent(new Event('change'));
  expect(project.tracks[0].show?.vol).toBe(true); expect(parent.querySelector('#cell-0-0-1-vol')).not.toBeNull();
  project.patterns[0].notes[0][1].notes[3] = 'C-4'; grid.render();
  document.querySelector<HTMLButtonElement>('[aria-label="Fewer note columns"]')!.click();
  expect(project.patterns[0].notes[0]).toHaveLength(2);
  project.patterns[0].notes[0][1].notes[3] = null;
  document.querySelector<HTMLButtonElement>('[aria-label="Fewer note columns"]')!.click();
  expect(project.patterns.map(p => p.notes[0].length)).toEqual([1, 1]);
});

it('copies and pastes sub-column cells as the numbers shown, and rejects text out of range', () => {
  const { parent, project, grid, key } = setup();
  project.tracks[0].show = { vol: true, pan: true }; grid.render();
  const column = project.patterns[0].notes[0][0];
  parent.querySelector<HTMLElement>('#cell-0-0-0-vol')!.click();
  clipboard(parent, 'paste', '50'); expect(column.vol[0]).toBe('64');
  clipboard(parent, 'paste', '101'); expect(column.vol[0]).toBe('64');
  clipboard(parent, 'paste', 'OFF'); expect(column.vol[0]).toBe('64');
  expect(clipboard(parent, 'copy')).toBe('50');
  parent.querySelector<HTMLElement>('#cell-0-0-0-pan')!.click(); clipboard(parent, 'paste', 'C'); expect(column.pan[0]).toBe('64');
  clipboard(parent, 'paste', '-25'); expect(column.pan[0]).toBe('32'); expect(clipboard(parent, 'copy')).toBe('-25');
  key('z', { metaKey: true }); key('z', { metaKey: true }); expect(column.pan[0]).toBeNull();
});

it('records a parameter into a new lane that spans every row', () => {
  const { project, grid } = setup();
  grid.writeParameter(0, { kind: 'instrument', parameter: 'cutoff' }, 900);
  const lane = project.patterns[0].fx[0].find(l => l.target?.kind === 'instrument')!;
  expect(lane.values).toHaveLength(32); expect(lane.values.filter(v => v !== null)).toEqual(['900']);
});
