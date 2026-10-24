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

function setup(sequence = vi.fn(), current: () => string | null = () => null) {
  const feedback = vi.fn();
  const parent = document.createElement('div'); parent.tabIndex = 0; document.body.append(parent);
  const project = newProject(), edited = vi.fn(), audition = vi.fn(), play = vi.fn();
  const grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, edited, audition, play, undefined, undefined, undefined, feedback, undefined, sequence, current);
  grid.recording = true;
  grid.render();
  const key = (key: string, options: KeyboardEventInit = {}) => parent.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...options }));
  const click = (row: number, track: number) => parent.querySelector<HTMLElement>(`#cell-${row}-${track}`)!.click();
  return { parent, project, grid, key, click, edited, audition, play, sequence, feedback };
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
  click(0, 0); key('ArrowDown', { shiftKey: true }); key('ArrowDown', { shiftKey: true }); key('ArrowRight', { shiftKey: true }); key('ArrowRight', { shiftKey: true }); key('ArrowRight', { shiftKey: true });
  expect(clipboard(parent, 'copy')).toBe('C-4\t00\t---\t---\n---\t---\t---\t---\n---\t---\t---\t---');
  key('Delete'); expect(project.patterns[0].notes[0][0].notes[0]).toBeNull();
  key('z', { ctrlKey: true }); expect(project.patterns[0].notes[0][0].notes[0]).toBe('C-4');
  clipboard(parent, 'paste', 'F#3\t---\t---\tA-3\n---\t---\t---\tC-4');
  expect(project.patterns[0].notes[0][0].notes[0]).toBe('F#3'); expect(project.patterns[0].notes[1][0].notes[0]).toBe('A-3');
  expect(project.patterns[0].notes[1][0].notes[1]).toBe('C-4');
  key('z', { ctrlKey: true });
  expect(project.patterns[0].notes[0][0].notes[0]).toBe('C-4'); expect(project.patterns[0].notes[1][0].notes[0]).toBeNull(); expect(project.patterns[0].notes[1][0].notes[1]).toBeNull();
});

it('clips a pasted block at the pattern edge and ignores malformed or empty clipboard data', () => {
  const { parent, project, click, grid } = setup();
  click(31, 3); clipboard(parent, 'paste', 'D-3\t---\t-6 dB\nF-3\t---\t6 dB');
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
  const second = { id: 'second', name: 'Second', rows: 64, fx: Array.from({ length: 4 }, () => [emptyFxLane(64)]), return_fx: [], master_fx: [emptyFxLane(64)], notes: Array.from({ length: 4 }, () => [blankColumn(64)]) };
  project.patterns.push(second);
  let pattern = project.patterns[0];
  const grid = new PatternGrid(parent, () => project, () => pattern, () => 4, vi.fn(), vi.fn(), vi.fn());
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

it('selects a track from its header buttons without entering notes, and navigates beyond the starter tracks', () => {
  const parent = document.createElement('div'); document.body.append(parent);
  const project = newProject(), selected = vi.fn();
  for (let i = 4; i < 8; i++) {
    project.tracks.push({ name: `Track ${i + 1}`, fx: [], pan: 0, volume_db: 0, muted: false });
    project.patterns[0].notes.push([blankColumn(32)]); project.patterns[0].fx.push([emptyFxLane(32)]);
  }
  const grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, vi.fn(), vi.fn(), vi.fn(), selected);
  grid.recording = true;
  grid.render();
  parent.querySelector<HTMLButtonElement>('[aria-label="Mute Track 8"]')!.click();
  expect(project.tracks[7].muted).toBe(true);
  parent.querySelector<HTMLElement>('#cell-0-7')!.click();
  expect(grid.activeTrack).toBe(7);
  parent.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', bubbles: true, cancelable: true }));
  expect(project.patterns[0].notes[7][0].notes[0]).toBe('C-4');
  const tab = () => parent.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true }));
  tab(); expect(grid.activeTrack).toBe(8); expect(selected).toHaveBeenLastCalledWith(8); // the master follows the last track
  tab(); expect(grid.activeTrack).toBe(0); expect(selected).toHaveBeenLastCalledWith(0);
});

it('has mute and solo buttons in the header, and marks a track that has effects', () => {
  const { parent, project, grid, edited } = setup();
  const button = (label: string) => parent.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)!;
  expect(parent.querySelector('.track-header[data-track="0"] .has-fx')).toBeNull(); expect(parent.querySelector('[aria-label^="Show"][aria-label$="effects"]')).toBeNull();
  button('Solo Bass').click(); expect(project.tracks[1].solo).toBe(true); expect(button('Solo Bass').getAttribute('aria-pressed')).toBe('true'); expect(edited).toHaveBeenCalled();
  button('Solo Bass').click(); expect(project.tracks[1].solo).toBe(false);
  project.tracks[0].fx.push({ id: 'd1', definition: 'echo', bypass: false, values: {}, modulations: {} }); grid.render();
  const dot = parent.querySelector('.track-header[data-track="0"] .has-fx')!; expect(dot.getAttribute('title')).toBe('1 effect');
});

it('advances notes by the entry step, wraps, and supports step zero', () => {
  const parent = document.createElement('div'); document.body.append(parent);
  const project = newProject(); let step = 4;
  const grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, vi.fn(), vi.fn(), vi.fn(), vi.fn(), () => step);
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
  expect(parent.querySelectorAll('[role=gridcell][aria-selected=true]')).toHaveLength(28);
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
  click(0, 0); key('ArrowRight', { shiftKey: true }); key('ArrowRight', { shiftKey: true });
  expect(clipboard(parent, 'copy')).toBe('C-4\t00\t0 db');
  click(1, 0); clipboard(parent, 'paste', 'D-4\t---\t6 dB');
  expect(project.patterns[0].notes[0][0].notes[1]).toBe('D-4'); expect(project.patterns[0].fx[0][0].values[1]).toBe('6');
  key('z', { metaKey: true });
  expect(project.patterns[0].notes[0][0].notes[1]).toBeNull(); expect(project.patterns[0].fx[0][0].values[1]).toBeNull();
  clipboard(parent, 'paste', '-6'); expect(project.patterns[0].notes[0][0].notes[1]).toBeNull();
});

it('adds and hides parameter lanes from the track menu, and keeps a lane that has values', () => {
  const { parent, project, feedback } = setup();
  const menu = (label: string) => { document.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)!.click(); };
  const open = () => parent.querySelector<HTMLButtonElement>('[aria-label="Lead columns"]')!.click();
  open(); menu('More parameter lanes');
  expect(project.patterns[0].fx[0]).toHaveLength(2);
  project.patterns[0].fx[0][1].values[4] = '6';
  menu('Fewer parameter lanes');
  expect(project.patterns[0].fx[0]).toHaveLength(2); expect(feedback).toHaveBeenCalledWith('Clear the last parameter lane before hiding it.');
  project.patterns[0].fx[0][1].values[4] = null;
  menu('Fewer parameter lanes');
  expect(project.patterns[0].fx[0]).toHaveLength(1);
  expect(document.querySelector<HTMLButtonElement>('[aria-label="Fewer parameter lanes"]')!.disabled).toBe(true);
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' })); // close the menu, which listens for it on the document
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
  document.querySelector<HTMLElement>('[aria-label="Colour #86ac1a"]')!.click();
  expect(project.tracks[1].colour).toBe('#86ac1a'); expect(edited).toHaveBeenCalled();
  expect(parent.querySelector<HTMLElement>('.track-header[data-track="1"]')!.style.getPropertyValue('--track-colour')).toBe('#86ac1a');
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
  click(1, 0); key('a'); expect(column.notes[1]).toBe('OFF'); expect(parent.querySelector('#cell-1-0')!.textContent).toBe('OFF');
  type(0, 'vol', '50'); expect(column.vol[0]).toBe('64'); expect(cell(0, 'vol').textContent).toBe('50');
  type(1, 'vol', '100'); expect(column.vol[1]).toBe('128'); type(2, 'vol', '101'); expect(column.vol[2]).toBeNull();
  type(2, 'pan', '-50'); expect(column.pan[2]).toBe('0'); expect(cell(2, 'pan').textContent).toBe('-50');
  type(3, 'pan', '+25'); expect(column.pan[3]).toBe('96'); expect(cell(3, 'pan').textContent).toBe('+25');
  type(4, 'pan', 'c'); expect(column.pan[4]).toBe('64'); expect(cell(4, 'pan').textContent).toBe('C'); type(5, 'pan', '51'); expect(column.pan[5]).toBeNull();
  type(5, 'delay', '50'); expect(column.delay[5]).toBe('128'); expect(cell(5, 'delay').textContent).toBe('50'); type(6, 'delay', '100'); expect(column.delay[6]).toBeNull();
  type(7, 'inst', '0'); expect(column.inst[7]).toBe(project.definitions.filter(d => d.kind === 'instr')[0].id); expect(cell(7, 'inst').textContent).toBe('00');
  type(10, 'inst', '7'); expect(column.inst[10]).toBeNull();
  // Typing a digit on a selected cell starts the same editor.
  cell(9, 'vol').click(); parent.querySelector<HTMLInputElement>('.lane-value')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  key('7'); const input = parent.querySelector<HTMLInputElement>('.lane-value')!; expect(input.value).toBe('7');
  input.value = '75'; input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); expect(column.vol[9]).toBe('96');
  grid.undo(); expect(column.vol[9]).toBeNull();
});

it('adds note columns to every pattern and shows or hides sub-columns from the track menu', () => {
  const { parent, project, grid } = setup();
  const second = { id: 'second', name: 'Second', rows: 16, fx: Array.from({ length: 4 }, () => [emptyFxLane(16)]), return_fx: [], master_fx: [emptyFxLane(16)], notes: Array.from({ length: 4 }, () => [blankColumn(16)]) };
  project.patterns.push(second); grid.render();
  parent.querySelector<HTMLButtonElement>('[aria-label="Lead columns"]')!.click();
  document.querySelector<HTMLButtonElement>('[aria-label="More note columns"]')!.click();
  expect(project.patterns.map(p => p.notes[0].length)).toEqual([2, 2]); expect(second.notes[0][1].notes).toHaveLength(16);
  expect(parent.querySelectorAll('.track-header[data-track="0"] .col-label')).toHaveLength(4);
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
  grid.writeParameter(0, { kind: 'instrument', instrument: 'tone', parameter: 'cutoff' }, 900);
  const lane = project.patterns[0].fx[0].find(l => l.target?.kind === 'instrument')!;
  expect(lane.values).toHaveLength(32); expect(lane.values.filter(v => v !== null)).toEqual(['900']);
});

it('transposes the selected notes by semitones and octaves, leaving OFF and notes that would leave the range', () => {
  const { project, key, click } = setup(); const notes = project.patterns[0].notes[0][0].notes;
  notes[0] = 'C-4'; notes[1] = 'OFF'; notes[2] = 'B-8'; notes[3] = 'C-0';
  click(0, 0); for (let i = 0; i < 3; i++) key('ArrowDown', { shiftKey: true });
  key('ArrowUp', { altKey: true }); expect(notes.slice(0, 4)).toEqual(['C#4', 'OFF', 'B-8', 'C#0']);
  key('ArrowUp', { altKey: true, shiftKey: true }); expect(notes.slice(0, 4)).toEqual(['C#5', 'OFF', 'B-8', 'C#1']);
  key('ArrowDown', { altKey: true, shiftKey: true }); key('ArrowDown', { altKey: true }); expect(notes.slice(0, 4)).toEqual(['C-4', 'OFF', 'A#7', 'C-0']); // B-8 could not go up, so it only came down
  key('z', { metaKey: true }); expect(notes[0]).toBe('C#4');
});

it('only edits with record mode on', () => {
  const { project, key, click, grid } = setup(); const notes = project.patterns[0].notes[0][0].notes; notes[0] = 'C-4';
  grid.recording = false; click(0, 0);
  for (const [name, options] of [['ArrowUp', { altKey: true }], ['Enter', { metaKey: true }], ['d', { metaKey: true }], ['Insert', {}], ['Backspace', {}]] as const) key(name, options);
  expect(notes.slice(0, 3)).toEqual(['C-4', null, null]);
});

it('interpolates the values between the first and last selected row, and rejects a selection with nothing to fill from', () => {
  const { project, key, grid, parent } = setup(); project.tracks[0].show = { vol: true }; grid.render();
  const vol = project.patterns[0].notes[0][0].vol; vol[0] = '0'; vol[4] = '128';
  parent.querySelector<HTMLElement>('#cell-0-0-0-vol')!.click(); for (let i = 0; i < 4; i++) key('ArrowDown', { shiftKey: true });
  key('i', { metaKey: true }); expect(vol.slice(0, 5)).toEqual(['0', '32', '64', '96', '128']);
  key('z', { metaKey: true }); expect(vol.slice(0, 5)).toEqual(['0', null, null, null, '128']);
  vol[4] = null; key('i', { metaKey: true }); expect(vol.slice(0, 5)).toEqual(['0', null, null, null, null]);
});

it('inserts and deletes a row, shifting the whole track and keeping it undoable', () => {
  const { project, key, click } = setup(); const column = project.patterns[0].notes[0][0], rows = project.patterns[0].rows, other = project.patterns[0].notes[1][0].notes;
  column.notes.fill(null); column.notes[1] = 'C-4'; column.notes[2] = 'D-4'; column.notes[rows - 1] = 'E-4'; other[1] = 'G-4';
  click(1, 0); key('Enter', { metaKey: true });
  expect(column.notes.slice(0, 4)).toEqual([null, null, 'C-4', 'D-4']); expect(column.notes[rows - 1]).toBeNull(); expect(other[1]).toBe('G-4');
  key('Backspace', { ctrlKey: true }); expect(column.notes.slice(0, 4)).toEqual([null, 'C-4', 'D-4', null]);
  key('Insert'); expect(column.notes.slice(0, 4)).toEqual([null, null, 'C-4', 'D-4']);
  key('z', { metaKey: true }); expect(column.notes.slice(0, 4)).toEqual([null, 'C-4', 'D-4', null]);
});

it('duplicates the selection below itself and carries on when repeated', () => {
  const { project, key, click } = setup(); const notes = project.patterns[0].notes[0][0].notes;
  notes.fill(null); notes[0] = 'C-4'; notes[1] = 'E-4'; click(0, 0); key('ArrowDown', { shiftKey: true });
  key('d', { metaKey: true }); expect(notes.slice(0, 4)).toEqual(['C-4', 'E-4', 'C-4', 'E-4']);
  key('d', { metaKey: true }); expect(notes.slice(0, 6)).toEqual(['C-4', 'E-4', 'C-4', 'E-4', 'C-4', 'E-4']);
  key('z', { metaKey: true }); expect(notes.slice(4, 6)).toEqual([null, null]);
});

it('plays a second octave on the Q row and moves by bars and to the ends of the pattern', () => {
  const { project, key, click } = setup(); const notes = project.patterns[0].notes[0][0].notes, last = project.patterns[0].rows - 1;
  click(0, 0); key('q'); expect(notes[0]).toBe('C-5');
  key('PageDown'); key('z'); expect(notes[16]).toBe('C-4');
  key('End'); key('u'); expect(notes[last]).toBe('B-5');
  key('Home'); key('i'); expect(notes[0]).toBe('C-6');
  key('End'); key('PageUp'); key('x'); expect(notes[last - 16]).toBe('D-4');
});

it('follows Renoise for Backspace, Delete and the column variants of insert and delete row', () => {
  const { project, key, click } = setup(); const first = project.patterns[0].notes[0][0].notes, second = project.patterns[0].notes[1][0].notes;
  first.fill(null); second.fill(null); first[1] = 'C-4'; first[2] = 'D-4'; second[1] = 'E-4'; second[2] = 'F-4';
  click(1, 0); key('Backspace'); expect(first.slice(0, 3)).toEqual([null, 'D-4', null]); // deletes the row and pulls the rest up
  key('z', { metaKey: true }); expect(first.slice(0, 3)).toEqual([null, 'C-4', 'D-4']);
  key('Delete'); expect(first.slice(0, 3)).toEqual([null, null, 'D-4']); // Delete only clears
  key('z', { metaKey: true }); expect(first.slice(0, 3)).toEqual([null, 'C-4', 'D-4']);
  key('y', { ctrlKey: true }); expect(first.slice(0, 3)).toEqual([null, null, 'D-4']); // Ctrl+Y redoes
  key('z', { metaKey: true }); expect(first.slice(0, 3)).toEqual([null, 'C-4', 'D-4']);
  // With the column variants only the selected column moves, not the rest of the track or the other tracks.
  const column = project.patterns[0].notes[0][0];
  key('Insert', { metaKey: true, shiftKey: true }); expect(first.slice(0, 4)).toEqual([null, null, 'C-4', 'D-4']); expect(second[1]).toBe('E-4');
  key('Delete', { ctrlKey: true, shiftKey: true }); expect(first.slice(0, 3)).toEqual([null, 'C-4', 'D-4']); expect(column.notes).toBe(first);
});

it('jumps to the quarter points of the pattern with F9 to F12', () => {
  const { project, key, click } = setup(); const notes = project.patterns[0].notes[0][0].notes; notes.fill(null); const rows = project.patterns[0].rows;
  click(5, 0);
  for (const [name, row] of [['F10', rows / 4], ['F11', rows / 2], ['F12', rows * 3 / 4], ['F9', 0]] as const) { key(name); key('z'); expect(notes[row]).toBe('C-4'); }
});

it('asks the sequence to move between steps, add or remove one, and change the pattern a step plays', () => {
  const { key, sequence } = setup();
  for (const [name, options, action] of [['ArrowUp', { metaKey: true }, 'previous'], ['ArrowDown', { ctrlKey: true }, 'next'], ['Home', { metaKey: true }, 'first'], ['End', { metaKey: true }, 'last'],
    ['ArrowLeft', { metaKey: true }, 'pattern-previous'], ['ArrowRight', { metaKey: true }, 'pattern-next'], ['Insert', { metaKey: true }, 'new'], ['Delete', { metaKey: true }, 'remove']] as const) {
    sequence.mockClear(); key(name, options); expect(sequence).toHaveBeenCalledWith(action);
  }
});

it('stamps the current instrument\'s number on each note entered in record mode, and one undo takes both back', () => {
  const { project, grid, key, audition } = setup(undefined, () => 'tone');
  project.definitions.push({ id: 'bell', name: 'Bell', kind: 'instr', source: 'instr bell(freq: hz) { osc(sine, freq) }' });
  grid.selectTrack(1); key('s');
  const column = project.patterns[0].notes[1][0];
  expect(column.notes[0]).toBe('C#4'); expect(column.inst[0]).toBe('tone'); expect(audition).toHaveBeenLastCalledWith('tone', 'C#4', 1);
  key('a'); expect(column.notes[1]).toBe('OFF'); expect(column.inst[1]).toBeNull(); // an OFF has no instrument
  grid.undo(); grid.undo();
  expect(column.notes[0]).toBeNull(); expect(column.inst[0]).toBeNull();
});

it('plays a note on the instrument set above it in its column when it has none, and shows the instrument column on every track', () => {
  const { parent, project, grid, key, audition } = setup(undefined, () => 'bell');
  const column = project.patterns[0].notes[0][0];
  grid.recording = false; grid.selectTrack(0); parent.querySelector<HTMLElement>('#cell-5-0')!.click();
  key('s'); expect(audition).toHaveBeenLastCalledWith('tone', 'C#4', 0); // row 0 sets tone, nothing below it does
  expect(column.inst[5]).toBeNull();
  expect(parent.querySelectorAll('[data-column]:not([id*="-0-0"])').length).toBeGreaterThan(0);
  expect(parent.querySelector('#cell-0-0-0-inst')).not.toBeNull(); expect(parent.querySelector('#cell-0-1-0-inst')).not.toBeNull();
  expect(parent.querySelector('#cell-0-0-0-vol')).toBeNull();
});

it('shades the beats and bars by rows per beat', () => {
  const { parent, project, grid } = setup();
  const shaded = (cls: string) => [...parent.querySelectorAll<HTMLElement>('.grid-row')].filter(r => r.classList.contains(cls)).map(r => Number(r.dataset.row)).slice(0, 4);
  project.rows_per_beat = 4; grid.render();
  expect(shaded('beat')).toEqual([0, 4, 8, 12]); expect(shaded('bar')).toEqual([0, 16]);
  project.rows_per_beat = 6; grid.render();
  expect(shaded('beat')).toEqual([0, 6, 12, 18]); expect(shaded('bar')).toEqual([0, 24]);
});

it('names a parameter lane briefly, grouped under what it belongs to, and keeps the full name as its title', () => {
  const { parent } = setup();
  const picker = parent.querySelector<HTMLSelectElement>('[aria-label="Lead parameter lane 1"]')!;
  expect([...picker.querySelectorAll('optgroup')].map(g => g.label)).toEqual(['Track']);
  expect([...picker.querySelectorAll('option')].map(o => o.textContent)).toEqual(['—', 'Vol', 'Pan']);
  expect(picker.title).toBe('Track volume');
});

it('lists only the instruments a track plays in the lane picker, and dims a lane for one it does not play', () => {
  const parent = document.createElement('div'); document.body.append(parent);
  const project = newProject();
  project.definitions.push({ id: 'bell', name: 'Bell', kind: 'instr', source: 'instr bell(freq: hz) { osc(sine, freq) }' });
  const parameters = { tone: [{ name: 'cutoff', min: 200, max: 8000, default: 2000, unit: 'hz' }], bell: [{ name: 'ring', min: 0, max: 1, default: 0.5, unit: null }] };
  const grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, vi.fn(), vi.fn(), vi.fn(), undefined, undefined, () => parameters);
  grid.render();
  const options = (track: string) => [...parent.querySelectorAll<HTMLOptionElement>(`[aria-label="${track} parameter lane 1"] option`)].map(o => o.textContent);
  expect(options('Lead')).toEqual(['—', 'Vol', 'Pan', 'cutoff']); // Lead's notes use the Warm saw; Bell is on no track
  expect(options('Bass')).toEqual(['—', 'Vol', 'Pan']);
  expect(parent.querySelector('.lane-heading.idle')).toBeNull();
  // a lane that already points at an instrument stays listed, and is dimmed while no note on the track plays it
  project.patterns[0].fx[1][0].target = { kind: 'instrument', instrument: 'bell', parameter: 'ring' }; grid.render();
  expect(options('Bass')).toEqual(['—', 'Vol', 'Pan', 'ring']);
  expect(parent.querySelector('.track-header[data-track="1"] .lane-heading.idle')).not.toBeNull();
  expect(parent.querySelector('.track-header[data-track="1"] .lane-target')!.getAttribute('title')).toContain('no effect');
  expect(parent.querySelector('#fx-0-1-0')!.classList.contains('idle')).toBe(true); expect(parent.querySelector('#fx-0-0-0')!.classList.contains('idle')).toBe(false);
  project.patterns[0].notes[1][0].inst[0] = 'bell'; grid.render();
  expect(parent.querySelector('.track-header[data-track="1"] .lane-heading.idle')).toBeNull();
});

it('chooses a lane\'s parameter from the keyboard: P opens a searchable list, Enter takes the first match', () => {
  const { parent, project, grid, key, feedback } = setup();
  const lane = project.patterns[0].fx[0][0];
  parent.querySelector<HTMLElement>('#fx-0-0-0')!.click();
  key('p');
  const search = document.querySelector<HTMLInputElement>('.browser input[type=search]')!;
  expect([...document.querySelectorAll('.browser-item')].map(b => b.textContent)).toEqual(['Vol', 'Pan', 'No parameter']);
  search.value = 'pan'; search.dispatchEvent(new Event('input'));
  search.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  expect(lane.target).toEqual({ kind: 'pan' }); expect(document.querySelector('.browser')).toBeNull(); expect(document.activeElement).toBe(parent);
  // a lane that holds values keeps its parameter, and says why
  lane.values[3] = '0.5'; grid.render(); parent.querySelector<HTMLElement>('#fx-0-0-0')!.click(); key('p');
  document.querySelector<HTMLButtonElement>('.browser-item')!.click();
  expect(lane.target).toEqual({ kind: 'pan' }); expect(feedback).toHaveBeenCalledWith('Clear this lane’s values before choosing a different parameter.');
  // Escape gives the keyboard back to the grid
  key('p'); document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
  expect(document.querySelector('.browser')).toBeNull(); expect(document.activeElement).toBe(parent);
});


function stripSetup() {
  const parent = document.createElement('div'); parent.tabIndex = 0; document.body.append(parent);
  const project = newProject(), edited = vi.fn(), feedback = vi.fn(), selected = vi.fn();
  project.definitions.push({ id: 'echo', name: 'Echo', kind: 'fx', source: 'fx echo { out = in }' });
  project.returns.push({ name: 'Verb', fx: [{ id: 'd1', definition: 'echo', bypass: false, values: {}, modulations: {} }], pan: 0, muted: false, volume_db: 0 });
  for (const pattern of project.patterns) pattern.return_fx.push([emptyFxLane(pattern.rows)]);
  const parameters = { echo: [{ name: 'mix', min: 0, max: 1, default: 0.3, unit: null }] };
  const grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, edited, vi.fn(), vi.fn(), selected, undefined, () => parameters, feedback);
  grid.recording = true; grid.render();
  return { parent, project, grid, edited, feedback, selected, key: (key: string) => parent.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true })) };
}

it('draws each return and the master after the tracks, with parameter lanes and no notes', () => {
  const { parent, project } = stripSetup();
  const headers = [...parent.querySelectorAll('.track-header')].map(h => h.querySelector('.track-name')!.textContent);
  expect(headers).toEqual(['Lead', 'Bass', 'Drums', 'Texture', 'Verb', 'Master']);
  for (const column of [4, 5]) {
    const header = parent.querySelector(`.track-header[data-track="${column}"]`)!;
    expect(header.querySelectorAll('.col-label')).toHaveLength(0); expect(header.querySelectorAll('.lane-target')).toHaveLength(1);
    expect(parent.querySelector(`#fx-0-${column}-0`)).not.toBeNull(); expect(parent.querySelector(`#cell-0-${column}`)).toBeNull();
  }
  expect([...parent.querySelectorAll('[aria-label="Mute Verb"], [aria-label="Mute Master"], [aria-label="Solo Verb"]')].map(b => b.getAttribute('aria-label'))).toEqual(['Mute Verb']);
  expect(project.returns[0].muted).toBe(false);
  // their headers size to their own column, so the menu button stays inside it
  expect(parent.querySelector('.track-header[data-track="5"]')!.classList.contains('strip')).toBe(true); expect(parent.querySelector('.track-header[data-track="0"]')!.classList.contains('strip')).toBe(false);
});

it('types values into a return\'s and the master\'s lanes, and offers them their own effects but no instruments', () => {
  const { parent, project, grid } = stripSetup();
  const picker = (name: string) => [...parent.querySelectorAll<HTMLOptionElement>(`[aria-label="${name} parameter lane 1"] option`)].map(o => o.textContent);
  expect(picker('Verb')).toEqual(['—', 'Vol', 'Pan', 'mix']); expect(picker('Master')).toEqual(['—', 'Vol', 'Pan']);
  parent.querySelector<HTMLElement>('#fx-2-5-0')!.click();
  expect(grid.activeTrack).toBe(5);
  parent.dispatchEvent(new KeyboardEvent('keydown', { key: '-', bubbles: true, cancelable: true }));
  const input = parent.querySelector<HTMLInputElement>('.lane-value')!; input.value = '-6'; input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  expect(project.patterns[0].master_fx[0].values[2]).toBe('-6');
  parent.querySelector<HTMLElement>('#fx-4-4-0')!.click(); grid.writeParameter(4, { kind: 'effect', device: 'd1', parameter: 'mix' }, 0.8);
  const lanes = project.patterns[0].return_fx[0]; expect(lanes.some(l => l.target?.kind === 'effect' && l.values.includes('0.8'))).toBe(true);
});

it('shows only the parameter lane controls in the menu of a return or the master', () => {
  const { parent, project } = stripSetup();
  parent.querySelector<HTMLButtonElement>('[aria-label="Verb columns"]')!.click();
  expect(document.querySelector('[aria-label="More note columns"]')).toBeNull(); expect(document.querySelector('[aria-label="Show Vol column"]')).toBeNull();
  document.querySelector<HTMLButtonElement>('[aria-label="More parameter lanes"]')!.click();
  expect(project.patterns[0].return_fx[0]).toHaveLength(2); expect(project.patterns[0].master_fx).toHaveLength(1);
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
});


it('shows a dB lane value without its unit, and says the unit in the cell\'s tooltip', () => {
  const { parent, project } = setup();
  project.patterns[0].fx[0][0].values[2] = '-6'; project.patterns[0].fx[0][0].target = { kind: 'volume' };
  const grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, vi.fn(), vi.fn(), vi.fn()); grid.render();
  const cell = parent.querySelector('#fx-2-0-0')!;
  expect(cell.firstChild!.textContent).toBe('-6'); expect(cell.getAttribute('title')).toContain('(db)');
});

it('moves down by the entry step after a value is entered in a parameter lane, as it does after a note', () => {
  const parent = document.createElement('div'); parent.tabIndex = 0; document.body.append(parent);
  const project = newProject(); let step = 3;
  const grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, vi.fn(), vi.fn(), vi.fn(), undefined, () => step);
  grid.recording = true; grid.render();
  const key = (k: string) => parent.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true }));
  parent.querySelector<HTMLElement>('#fx-2-0-0')!.click();
  key('5'); const input = parent.querySelector<HTMLInputElement>('.lane-value')!; input.value = '-6';
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  expect(project.patterns[0].fx[0][0].values[2]).toBe('-6');
  expect(parent.querySelector('#fx-5-0-0')!.classList.contains('cursor')).toBe(true); // 2 + 3
  // step 0 stays on the row, like a note
  step = 0; key('7'); const again = parent.querySelector<HTMLInputElement>('.lane-value')!; again.value = '-9'; again.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  expect(project.patterns[0].fx[0][0].values[5]).toBe('-9'); expect(parent.querySelector('#fx-5-0-0')!.classList.contains('cursor')).toBe(true);
});

it('moves down by the entry step after a value in a sub-column, and stays put on a value it refuses', () => {
  const parent = document.createElement('div'); parent.tabIndex = 0; document.body.append(parent);
  const project = newProject(); project.tracks[0].show = { vol: true };
  const feedback = vi.fn(), grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, vi.fn(), vi.fn(), vi.fn(), undefined, () => 4, undefined, feedback);
  grid.recording = true; grid.render();
  const key = (k: string) => parent.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true }));
  const type = (text: string) => { const input = parent.querySelector<HTMLInputElement>('.lane-value')!; input.value = text; input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); };
  parent.querySelector<HTMLElement>('#cell-1-0-0-vol')!.click(); key('8'); type('80');
  expect(project.patterns[0].notes[0][0].vol[1]).toBe('102'); expect(parent.querySelector('#cell-5-0-0-vol')!.classList.contains('cursor')).toBe(true);
  key('9'); type('150'); // out of range: refused, and the cursor stays
  expect(project.patterns[0].notes[0][0].vol[5]).toBeNull(); expect(feedback).toHaveBeenCalled();
  expect(parent.querySelector<HTMLInputElement>('.lane-value')).not.toBeNull();
});


it('lights a lane\'s envelope button while its envelope is in use', () => {
  const { parent, project, grid } = setup();
  const button = () => parent.querySelector<HTMLButtonElement>('[aria-label="Lead lane 1 automation"]')!;
  expect(button().classList.contains('on')).toBe(false);
  const lane = project.patterns[0].fx[0][0]; lane.envelope = { enabled: false, mode: 'linear', points: [{ row: 0, value: 0 }] }; grid.render();
  expect(button().classList.contains('on')).toBe(false); // an envelope that is switched off is not in use
  lane.envelope.enabled = true; grid.render();
  expect(button().classList.contains('on')).toBe(true); expect(button().title).toContain('in use');
});


it('enters OFF with A and a CUT with the minus key, neither with an instrument or a sound, and accepts a pasted CUT', () => {
  const { parent, project, grid, key, audition, click } = setup(undefined, () => 'tone');
  const column = project.patterns[0].notes[0][0];
  click(2, 0); audition.mockClear(); key('A'); key('-'); 
  expect(column.notes[2]).toBe('OFF'); expect(column.notes[3]).toBe('CUT'); expect(column.inst[2]).toBeNull(); expect(column.inst[3]).toBeNull();
  expect(audition).not.toHaveBeenCalled(); expect(parent.querySelector('#cell-3-0')!.classList.contains('cut')).toBe(true); expect(parent.querySelector('#cell-3-0')!.textContent).toBe('CUT');
  key('1'); key('`'); expect(column.notes[4]).toBe('D#4'); // the old keys do nothing now: row 4 is still the starter note
  click(6, 0); clipboard(parent, 'paste', 'CUT'); expect(column.notes[6]).toBe('CUT');
  grid.undo(); expect(column.notes[6]).toBeNull();
});
