// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { PatternGrid } from './grid';
import { newProject, emptyFxLane, blankColumn, type Project } from './project';
import { ProjectHistory } from './history';

Element.prototype.scrollIntoView = () => {};
// happy-dom does not expose the browser's Option convenience constructor.
vi.stubGlobal('Option', function (text: string, value: string) {
  const option = document.createElement('option'); option.textContent = text; option.value = value; return option;
});
afterEach(() => document.body.replaceChildren());

/** Puts `from` into `into` in place, so the arrays a test holds see what an undo restores. */
function restore(into: Record<string, unknown> | unknown[], from: Record<string, unknown> | unknown[]) {
  const nested = (a: unknown, b: unknown) => a !== null && b !== null && typeof a === 'object' && typeof b === 'object';
  if (Array.isArray(into) && Array.isArray(from)) into.length = from.length;
  else for (const key of Object.keys(into)) if (!(key in from)) delete (into as Record<string, unknown>)[key];
  for (const [key, value] of Object.entries(from)) {
    const target = into as Record<string, unknown>;
    if (nested(target[key], value)) restore(target[key] as Record<string, unknown>, value as Record<string, unknown>); else target[key] = value;
  }
}

/** Gives `grid` the project undo the app gives it: every edit is recorded, and undo puts the project back. */
function withHistory(grid: PatternGrid, project: Project, edited = vi.fn()) {
  const history = new ProjectHistory(JSON.stringify(project));
  edited.mockImplementation(() => history.record(JSON.stringify(project)));
  grid.projectUndo = redo => {
    const raw = redo ? history.redo() : history.undo();
    if (raw !== undefined) { restore(project as unknown as Record<string, unknown>, JSON.parse(raw)); grid.render(); }
  };
  /** Starts the history from the project as it is now, after a test has set it up by hand. */
  return () => history.reset(JSON.stringify(project));
}

function setup(sequence = vi.fn(), current: () => string | null = () => null, velocity: () => number | null = () => null) {
  const feedback = vi.fn();
  const parent = document.createElement('div'); parent.tabIndex = 0; document.body.append(parent);
  const project = newProject(), edited = vi.fn(), audition = vi.fn(), play = vi.fn();
  const grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, edited, audition, play, undefined, undefined, feedback, sequence, current, velocity);
  const baseline = withHistory(grid, project, edited);
  grid.recording = true;
  grid.render();
  const key = (key: string, options: KeyboardEventInit = {}) => parent.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...options }));
  const click = (row: number, track: number) => parent.querySelector<HTMLElement>(`#cell-${row}-${track}`)!.click();
  return { parent, project, grid, key, click, edited, audition, play, sequence, feedback, baseline };
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
  expect(audition).toHaveBeenCalledWith('tone', 'C-4', 0, { hold: 2096 });
  key('z', { metaKey: true }); expect(project.patterns[0].notes[0][0].notes[1]).toBeNull();
  key('z', { metaKey: true, shiftKey: true }); expect(project.patterns[0].notes[0][0].notes[1]).toBe('C-4');
  expect(edited).toHaveBeenCalledTimes(1);
});

it('copies a rectangular selection and treats a multi-cell paste as one undoable edit', () => {
  const { parent, project, key, click } = setup();
  click(0, 0); key('ArrowDown', { shiftKey: true }); key('ArrowDown', { shiftKey: true }); key('ArrowRight', { shiftKey: true }); key('ArrowRight', { shiftKey: true });
  expect(clipboard(parent, 'copy')).toBe('C-4\t00\t---\n---\t---\t---\n---\t---\t---');
  key('Delete'); expect(project.patterns[0].notes[0][0].notes[0]).toBeNull();
  key('z', { ctrlKey: true }); expect(project.patterns[0].notes[0][0].notes[0]).toBe('C-4');
  clipboard(parent, 'paste', 'F#3\t---\tS80\n---\t---\tU10');
  expect(project.patterns[0].notes[0][0].notes[0]).toBe('F#3'); expect(project.patterns[0].notes[0][0].fx[0][0]).toBe('S80');
  expect(project.patterns[0].notes[0][0].fx[0][1]).toBe('U10');
  key('z', { ctrlKey: true });
  expect(project.patterns[0].notes[0][0].notes[0]).toBe('C-4'); expect(project.patterns[0].notes[0][0].fx[0][0]).toBeNull(); expect(project.patterns[0].notes[0][0].fx[0][1]).toBeNull();
});

it('clips a pasted block at the pattern edge and ignores malformed or empty clipboard data', () => {
  const { parent, project, click, grid } = setup();
  click(31, 3); clipboard(parent, 'paste', 'D-3\t---\nF-3\t---');
  expect(project.patterns[0].notes[3][0].notes[31]).toBe('D-3');
  expect(project.patterns[0].notes[3][0].notes).toHaveLength(32);
  clipboard(parent, 'paste', 'not notes'); clipboard(parent, 'paste', '');
  expect(project.patterns[0].notes[3][0].notes[31]).toBe('D-3');
  grid.undo(); expect(project.patterns[0].notes[3][0].notes[31]).toBeNull();
});

it('keeps selector typing out of the note grid and space playback local to the grid', () => {
  const { parent, project, key, play } = setup();
  const select = document.createElement('select'); parent.append(select);
  select.dispatchEvent(new KeyboardEvent('keydown', { key: 's', bubbles: true }));
  expect(project.patterns[0].notes[0][0].notes[0]).toBe('C-4');
  key(' '); expect(play).toHaveBeenCalledTimes(1);
  const code = document.createElement('textarea'); document.body.append(code);
  code.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true }));
  expect(play).toHaveBeenCalledTimes(1);
});

it('edits a pattern longer than the first', () => {
  const parent = document.createElement('div'); document.body.append(parent);
  const project = newProject();
  const second = { id: 'second', name: 'Second', rows: 64, fx: Array.from({ length: 4 }, () => [emptyFxLane()]), return_fx: [], master_fx: [emptyFxLane()], commands: [[], [], [], [], []], notes: Array.from({ length: 4 }, () => [blankColumn(64)]) };
  project.patterns.push(second);
  const grid = new PatternGrid(parent, () => project, () => second, () => 4, vi.fn(), vi.fn(), vi.fn());
  grid.recording = true; grid.render();
  parent.querySelector<HTMLElement>('#cell-63-0')!.click();
  parent.dispatchEvent(new KeyboardEvent('keydown', { key: 'x', bubbles: true, cancelable: true }));
  expect(second.notes[0][0].notes[63]).toBe('D-4');
});

it('selects a track from its header buttons without entering notes, and navigates beyond the starter tracks', () => {
  const parent = document.createElement('div'); document.body.append(parent);
  const project = newProject(), selected = vi.fn();
  for (let i = 4; i < 8; i++) {
    project.tracks.push({ name: `Track ${i + 1}`, fx: [], pan: 0, volume_db: 0, muted: false });
    project.patterns[0].notes.push([blankColumn(32)]); project.patterns[0].fx.push([emptyFxLane()]); project.patterns[0].commands.splice(project.tracks.length - 1, 0, []);
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

it('copies and pastes notes with commands as one undoable rectangle and rejects mismatched data', () => {
  const { parent, project, grid, key, click } = setup();
  project.patterns[0].notes[0][0].fx[0][0] = 'L80'; grid.render();
  click(0, 0); key('ArrowRight', { shiftKey: true }); key('ArrowRight', { shiftKey: true }); // the note, its instrument and its command column
  expect(clipboard(parent, 'copy')).toBe('C-4\t00\tL80');
  click(1, 0); clipboard(parent, 'paste', 'D-4\t---\t12FF');
  expect(project.patterns[0].notes[0][0].notes[1]).toBe('D-4'); expect(project.patterns[0].notes[0][0].fx[0][1]).toBe('12FF');
  key('z', { metaKey: true });
  expect(project.patterns[0].notes[0][0].notes[1]).toBeNull(); expect(project.patterns[0].notes[0][0].fx[0][1]).toBeNull();
  clipboard(parent, 'paste', '-6'); expect(project.patterns[0].notes[0][0].notes[1]).toBeNull();
  clipboard(parent, 'paste', 'D-4\t---\tQ00'); expect(project.patterns[0].notes[0][0].notes[1]).toBeNull(); // not a command
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
  grid.setRecording(true); expect(grid.recording).toBe(true); expect(changed).toHaveBeenCalledWith(true);
  key('s'); expect(project.patterns[0].notes[0][0].notes[0]).toBe('C#4');
  grid.setRecording(false); expect(grid.recording).toBe(false);
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
  const second = { id: 'second', name: 'Second', rows: 16, fx: Array.from({ length: 4 }, () => [emptyFxLane()]), return_fx: [], master_fx: [emptyFxLane()], commands: [[Array(16).fill(null)]], notes: Array.from({ length: 4 }, () => [blankColumn(16)]) };
  project.patterns.push(second); grid.render();
  parent.querySelector<HTMLButtonElement>('[aria-label="Lead columns"]')!.click();
  document.querySelector<HTMLButtonElement>('[aria-label="More note columns"]')!.click();
  expect(project.patterns.map(p => p.notes[0].length)).toEqual([2, 2]); expect(second.notes[0][1].notes).toHaveLength(16);
  expect(parent.querySelectorAll('.track-header[data-track="0"] .col-label')).toHaveLength(4 + 2); // each note column has Note, Ins and Fx
  const box = document.querySelector<HTMLInputElement>('[aria-label="Show Vel column"]')!; box.checked = true; box.dispatchEvent(new Event('change'));
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

it('transposes the selected notes by semitones and octaves, leaving OFF and notes that would leave the range', () => {
  const { project, key, click } = setup(); const notes = project.patterns[0].notes[0][0].notes;
  notes[0] = 'C-4'; notes[1] = 'OFF'; notes[2] = 'B-8'; notes[3] = 'C-0';
  click(0, 0); for (let i = 0; i < 3; i++) key('ArrowDown', { shiftKey: true });
  key('ArrowUp', { altKey: true, metaKey: true }); expect(notes.slice(0, 4)).toEqual(['C#4', 'OFF', 'B-8', 'C#0']);
  key('ArrowUp', { metaKey: true, altKey: true, shiftKey: true }); expect(notes.slice(0, 4)).toEqual(['C#5', 'OFF', 'B-8', 'C#1']);
  key('ArrowDown', { metaKey: true, altKey: true, shiftKey: true }); key('ArrowDown', { altKey: true, metaKey: true }); expect(notes.slice(0, 4)).toEqual(['C-4', 'OFF', 'A#7', 'C-0']); // B-8 could not go up, so it only came down
  key('z', { metaKey: true }); expect(notes[0]).toBe('C#4');
});

it('only edits with record mode on', () => {
  const { project, key, click, grid } = setup(); const notes = project.patterns[0].notes[0][0].notes; notes[0] = 'C-4';
  grid.recording = false; click(0, 0);
  for (const [name, options] of [['ArrowUp', { altKey: true, metaKey: true }], ['Enter', { metaKey: true }], ['d', { metaKey: true }], ['Insert', {}], ['Backspace', {}]] as const) key(name, options);
  expect(notes.slice(0, 3)).toEqual(['C-4', null, null]);
});

it('interpolates the values between the first and last selected row, and rejects a selection with nothing to fill from', () => {
  const { project, key, grid, parent, baseline } = setup(); project.tracks[0].show = { vol: true }; grid.render();
  const vol = project.patterns[0].notes[0][0].vol; vol[0] = '0'; vol[4] = '128'; baseline();
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
  const { project, key, click, baseline } = setup(); const first = project.patterns[0].notes[0][0].notes, second = project.patterns[0].notes[1][0].notes;
  first.fill(null); second.fill(null); first[1] = 'C-4'; first[2] = 'D-4'; second[1] = 'E-4'; second[2] = 'F-4'; baseline();
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
  const move = { altKey: true, shiftKey: true };
  for (const [name, options, action] of [['ArrowUp', move, 'previous'], ['ArrowDown', move, 'next'], ['Home', move, 'first'], ['End', move, 'last'],
    ['Insert', { metaKey: true }, 'new'], ['Delete', { metaKey: true }, 'remove']] as const) {
    sequence.mockClear(); key(name, options); expect(sequence).toHaveBeenCalledWith(action);
  }
});

it('stamps the current instrument\'s number on each note entered in record mode, and one undo takes both back', () => {
  const { project, grid, key, audition } = setup(undefined, () => 'tone');
  project.definitions.push({ id: 'bell', name: 'Bell', kind: 'instr', source: 'instr bell(freq: hz) { osc(sine, freq) }' });
  grid.selectTrack(1); key('s');
  const column = project.patterns[0].notes[1][0];
  expect(column.notes[0]).toBe('C#4'); expect(column.inst[0]).toBe('tone'); expect(audition).toHaveBeenLastCalledWith('tone', 'C#4', 1, { hold: 2097 });
  key('a'); expect(column.notes[1]).toBe('OFF'); expect(column.inst[1]).toBeNull(); // an OFF has no instrument
  grid.undo(); grid.undo();
  expect(column.notes[0]).toBeNull(); expect(column.inst[0]).toBeNull();
});

it('previews the selected instrument instead of the pattern instrument, and shows the instrument column on every track', () => {
  const { parent, project, grid, key, audition } = setup(undefined, () => 'bell');
  const column = project.patterns[0].notes[0][0];
  grid.recording = false; grid.selectTrack(0); parent.querySelector<HTMLElement>('#cell-5-0')!.click();
  key('s'); expect(audition).toHaveBeenLastCalledWith('bell', 'C#4', 0, { hold: 2097 });
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

it('moves down by the entry step after a value in a sub-column, and stays put on a value it refuses', () => {
  const parent = document.createElement('div'); parent.tabIndex = 0; document.body.append(parent);
  const project = newProject(); project.tracks[0].show = { vol: true };
  const feedback = vi.fn(), grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, vi.fn(), vi.fn(), vi.fn(), undefined, () => 4, feedback);
  grid.recording = true; grid.render();
  const key = (k: string) => parent.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true }));
  const type = (text: string) => { const input = parent.querySelector<HTMLInputElement>('.lane-value')!; input.value = text; input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); };
  parent.querySelector<HTMLElement>('#cell-1-0-0-vol')!.click(); key('8'); type('80');
  expect(project.patterns[0].notes[0][0].vol[1]).toBe('102'); expect(parent.querySelector('#cell-5-0-0-vol')!.classList.contains('cursor')).toBe(true);
  key('9'); type('150'); // out of range: refused, and the cursor stays
  expect(project.patterns[0].notes[0][0].vol[5]).toBeNull(); expect(feedback).toHaveBeenCalled();
  expect(parent.querySelector<HTMLInputElement>('.lane-value')).not.toBeNull();
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

it('jumps with Cmd/Ctrl+arrows as a spreadsheet does, and selects as it goes with Shift', () => {
  const sequence = vi.fn(), { project, key, click, parent } = setup(sequence);
  const notes = project.patterns[0].notes[0][0].notes; notes.fill(null);
  notes[2] = 'C-4'; notes[3] = 'D-4'; notes[4] = 'E-4'; notes[9] = 'G-4';
  const cursor = () => { const c = parent.querySelector<HTMLElement>('.cursor')!; return [Number(c.dataset.noteRow), Number(c.dataset.column)]; };
  click(0, 0);
  key('ArrowDown', { metaKey: true }); expect(cursor()).toEqual([2, 0]); // across the gap to the first note
  key('ArrowDown', { metaKey: true }); expect(cursor()).toEqual([4, 0]); // to the end of the run
  key('ArrowDown', { ctrlKey: true }); expect(cursor()).toEqual([9, 0]);
  key('ArrowDown', { metaKey: true }); expect(cursor()).toEqual([31, 0]); // nothing more: the last row
  key('ArrowUp', { metaKey: true, shiftKey: true }); expect(cursor()).toEqual([9, 0]);
  expect(parent.querySelectorAll('[data-column="0"].range-selected').length).toBe(31 - 9 + 1);
  key('Home', { metaKey: true }); expect(cursor()).toEqual([0, 0]);
  key('End', { metaKey: true }); expect(cursor()[0]).toBe(31);
  key('Home', { metaKey: true }); key('ArrowRight', { metaKey: true }); const edge = cursor()[1]; expect(edge).toBeGreaterThan(0); // the end of the first track
  key('ArrowRight', { metaKey: true }); expect(cursor()[1]).toBeGreaterThan(edge); // then the next track's start
  key('ArrowLeft', { metaKey: true }); expect(cursor()[1]).toBe(edge);
  click(5, 0); key(' ', { ctrlKey: true }); expect(parent.querySelectorAll('[data-column="0"].range-selected').length).toBe(32);
  expect(sequence).not.toHaveBeenCalled();
});

it('previews from instrument and command columns outside record mode without editing or advancing', () => {
  const { parent, project, grid, key, audition, edited } = setup(undefined, () => 'bell');
  grid.recording = false;
  const before = JSON.stringify(project);
  for (const cell of ['#cell-5-0-0-inst', '#cell-0-1']) {
    parent.querySelector<HTMLElement>(cell)!.click();
    const cursor = parent.getAttribute('aria-activedescendant');
    key('s');
    expect(audition).toHaveBeenLastCalledWith('bell', 'C#4', 0, { hold: 2097 });
    expect(parent.getAttribute('aria-activedescendant')).toBe(cursor);
  }
  expect(audition).toHaveBeenCalledTimes(2);
  expect(JSON.stringify(project)).toBe(before);
  expect(edited).not.toHaveBeenCalled();
});

it('gives notes entered in record mode the default velocity, showing the track\'s Vol column', () => {
  const { project, grid, key } = setup(undefined, () => 'tone', () => 50);
  const column = project.patterns[0].notes[1][0];
  grid.selectTrack(1); key('s');
  expect(project.tracks[1].show?.vol).toBe(true);
  expect(column.vol.filter(v => v === '64')).toHaveLength(1);
});

it('records onto the nearest row while the pattern plays, leaving the cursor on the play position', () => {
  const { project, grid, key } = setup();
  const column = project.patterns[0].notes[0][0];
  grid.selectTrack(0); grid.paint(3, 0.6, Date.now()); // 60% through row 3: the nearest is row 4
  key('s'); key('ArrowDown');
  expect(column.notes[4]).toBe('C#4'); expect(column.notes[3]).toBeNull();
  key('s'); expect(column.notes[4]).toBe('C#4'); // no step, and the cursor cannot be moved off the play row
  grid.paint(null);
});

it('selects every row and column of the cursor\'s track for copying', () => {
  const { parent, project, key, click } = setup();
  project.patterns[0].notes[1][0].notes[5] = 'D-3'; project.patterns[0].notes[1][0].inst[5] = 'tone';
  const columns = (track: number) => 2 + project.patterns[0].commands[track].length;
  click(7, 0); // somewhere in the Lead track
  key('A', { metaKey: true, shiftKey: true });
  const lead = clipboard(parent, 'copy').split('\n');
  expect(lead).toHaveLength(32); expect(lead[0].split('\t')).toHaveLength(columns(0)); expect(lead.join()).not.toContain('D-3');
  click(7, 1); // the Bass track
  key('A', { metaKey: true, shiftKey: true });
  const bass = clipboard(parent, 'copy').split('\n');
  expect(bass).toHaveLength(32); expect(bass[5].split('\t')[0]).toBe('D-3');
  key('a', { metaKey: true });
  expect(clipboard(parent, 'copy').split('\n')[0].split('\t').length).toBeGreaterThan(columns(0) * 3);
});

it('enters absolute MIDI pitch, hardware velocity and instrument as one undoable step', () => {
  const { project, grid, click, audition } = setup(undefined, () => 'tone', () => 1);
  const column = project.patterns[0].notes[1][0];
  click(5, 1);
  expect(grid.midiNoteOn(61, 64)).toEqual({ note: 'C#4', track: 1, instrument: 'tone', velocity: 64 });
  expect(column.notes[5]).toBe('C#4'); expect(column.inst[5]).toBe('tone'); expect(column.vol[5]).toBe('65');
  expect(project.tracks[1].show?.vol).toBe(true); expect(audition).not.toHaveBeenCalled(); // native held preview is separate
  grid.midiNoteOn(72, 127); expect(column.notes[6]).toBe('C-5'); expect(column.vol[6]).toBe('128');
  grid.undo(); expect(column.notes[6]).toBeNull(); expect(column.vol[6]).toBeNull();
  grid.redo(); expect(column.notes[6]).toBe('C-5'); expect(column.inst[6]).toBe('tone');
});

it('previews MIDI without editing when record is off, including from an instrument column', () => {
  const { project, grid, parent, edited } = setup(undefined, () => 'tone');
  grid.setRecording(false); parent.querySelector<HTMLElement>('#cell-4-1-0-inst')!.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 0 }));
  const before = JSON.stringify(project);
  expect(grid.midiNoteOn(60, 100)).toEqual({ note: 'C-4', track: 1, instrument: 'tone', velocity: 100 });
  expect(JSON.stringify(project)).toBe(before); expect(edited).not.toHaveBeenCalled();
  for (const [note, velocity] of [[0, 100], [120, 100], [60, 0], [60, 128], [60.5, 1]]) expect(grid.midiNoteOn(note, velocity)).toBeNull();
});

it('records MIDI from a sub-column into the correct note column when velocity reveals Vol', () => {
  const { project, grid, parent } = setup(undefined, () => 'tone');
  project.patterns[0].notes[0].push(blankColumn(32)); grid.render();
  parent.querySelector<HTMLElement>('#cell-3-0-1-inst')!.click();
  grid.midiNoteOn(67, 127);
  expect(project.patterns[0].notes[0][1].notes[3]).toBe('G-4');
  expect(project.patterns[0].notes[0][1].inst[3]).toBe('tone');
  expect(project.patterns[0].notes[0][1].vol[3]).toBe('128');
  expect(project.patterns[0].notes[0][0].notes[3]).toBeNull();
});

it('records MIDI on the nearest live row without stepping away from playback', () => {
  const { project, grid } = setup(undefined, () => 'tone');
  // Rendering between the two events must not advance this test's playback clock.
  const clock = vi.spyOn(Date, 'now').mockReturnValue(1000);
  try {
    grid.selectTrack(1); grid.paint(3, 0.6, Date.now());
    grid.midiNoteOn(62, 90); grid.midiNoteOn(64, 90);
    expect(project.patterns[0].notes[1][0].notes[4]).toBe('E-4');
    expect(project.patterns[0].notes[1][0].notes[5]).toBeNull();
    grid.paint(null);
  } finally { clock.mockRestore(); }
});

it('reverses and randomizes the selection, one undo step each, and keeps rests and OFF where they are', () => {
  const { project, grid, key, click, feedback, baseline } = setup();
  const notes = project.patterns[0].notes[0][0].notes;
  notes.splice(0, 8, 'C-4', null, 'E-4', 'OFF', 'G-4', null, 'C-5', null); baseline();
  click(0, 0); for (let i = 0; i < 7; i++) key('ArrowDown', { shiftKey: true });
  grid.reverseSelection();
  expect(notes.slice(0, 8)).toEqual([null, 'C-5', null, 'G-4', 'OFF', 'E-4', null, 'C-4']);
  key('z', { ctrlKey: true }); expect(notes.slice(0, 8)).toEqual(['C-4', null, 'E-4', 'OFF', 'G-4', null, 'C-5', null]);
  grid.random = () => 0.999;
  grid.randomizeSelection();
  expect(notes.slice(0, 8)).toEqual(['C-5', null, 'C-5', 'OFF', 'C-5', null, 'C-5', null]);
  grid.random = () => 0; key('z', { ctrlKey: true }); grid.randomizeSelection();
  expect(notes.slice(0, 8)).toEqual(['C-4', null, 'C-4', 'OFF', 'C-4', null, 'C-4', null]);
  // a column with a single value has nothing to vary
  key('z', { ctrlKey: true }); notes.splice(0, 8, 'C-4', null, null, null, null, null, null, null);
  grid.randomizeSelection(); expect(feedback).toHaveBeenCalled();
  grid.recording = false; feedback.mockClear(); grid.reverseSelection(); expect(feedback).toHaveBeenCalledWith(expect.stringContaining('record mode'));
});

it('randomizes the numbers of a volume column between its lowest and highest', () => {
  const { project, grid, key, click } = setup();
  project.tracks[0].show = { vol: true }; grid.render();
  const vol = project.patterns[0].notes[0][0].vol; vol.splice(0, 3, '20', '60', '100');
  click(0, 0); key('ArrowRight', { shiftKey: true }); key('ArrowRight', { shiftKey: true }); for (let i = 0; i < 2; i++) key('ArrowDown', { shiftKey: true });
  grid.random = () => 0.5; grid.randomizeSelection();
  expect(vol.slice(0, 3)).toEqual(['60', '60', '60']);
});

it('cuts a whole track to the clipboard and clears it', () => {
  const { project, grid, click } = setup();
  const written: string[] = [], real = navigator;
  vi.stubGlobal('navigator', { clipboard: { writeText: (text: string) => { written.push(text); return Promise.resolve(); } } });
  project.patterns[0].notes[1][0].notes[2] = 'D-3'; click(5, 1);
  grid.cutTrack();
  expect(written[0].split('\n')).toHaveLength(32); expect(written[0].split('\n')[2].split('\t')[0]).toBe('D-3');
  expect(project.patterns[0].notes[1][0].notes[2]).toBeNull();
  vi.stubGlobal('navigator', real);
});

function withCommandColumns(count = 2) {
  const made = setup();
  for (const p of made.project.patterns) for (const track of p.notes) for (const column of track) column.fx = Array.from({ length: count }, () => Array(p.rows).fill(null));
  made.grid.render();
  return made;
}

it('enters command cells and plays the note they act on with them', () => {
  const { parent, project, audition, feedback } = withCommandColumns();
  const column = project.patterns[0].notes[0][0], cell = (row: number, idx = 0) => parent.querySelector<HTMLElement>(`#cell-${row}-0-0-fx${idx ? idx + 1 : ''}`)!;
  const type = (row: number, idx: number, text: string) => {
    cell(row, idx).click(); const input = parent.querySelector<HTMLInputElement>('.lane-value')!; input.value = text;
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  };
  type(0, 0, 's80'); expect(column.fx[0][0]).toBe('S80'); expect(cell(0).textContent).toBe('S80');
  expect(cell(0).title).toBe('S80 starts the sample 50% of the way in'); expect(feedback).toHaveBeenLastCalledWith('S80 starts the sample 50% of the way in');
  expect(audition).toHaveBeenLastCalledWith('tone', 'C-4', 0, { commands: ['S80'] });
  type(3, 0, 'r03'); expect(column.fx[0][3]).toBe('R03'); expect(audition).toHaveBeenLastCalledWith('tone', 'C-4', 0, { commands: ['R03'] }); // the note held above it
  audition.mockClear(); type(4, 0, 'x99'); expect(column.fx[0][4]).toBeNull(); expect(feedback).toHaveBeenLastCalledWith('Commands are S, B, G, U, D, R, C, Y, I and O for a note, and L, P or four hex digits for the track'); expect(audition).not.toHaveBeenCalled();
  type(4, 0, 'b02'); expect(column.fx[0][4]).toBeNull(); expect(feedback).toHaveBeenLastCalledWith('B is B00 (backwards) or B01 (forwards)');
  type(5, 0, 'g10'); expect(column.fx[0][5]).toBe('G10'); expect(audition).not.toHaveBeenCalled();
  type(5, 1, 'r03'); expect(column.fx[1][5]).toBeNull(); // a glide and a retrigger compete for the row, even in another command column
  type(6, 1, 'r03'); expect(column.fx[1][6]).toBe('R03');
  type(3, 1, 'u10'); expect(column.fx[1][3]).toBe('U10'); // a slide and a retrigger do not
  audition.mockClear(); column.notes[7] = 'OFF'; type(7, 0, 'r03'); expect(column.fx[0][7]).toBe('R03'); expect(audition).not.toHaveBeenCalled(); // nothing is held after an OFF
});

it('starts a command from a letter or digit typed on its cell', () => {
  const { parent, key, click, project } = withCommandColumns(1);
  click(2, 0); parent.querySelector<HTMLElement>('#cell-2-0-0-fx')!.click();
  key('g');
  const input = parent.querySelector<HTMLInputElement>('.lane-value')!;
  expect(input.value).toBe('G'); expect(input.maxLength).toBe(4); // room for an effect command such as 12FF
  input.value = 'G10'; input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  expect(project.patterns[0].notes[0][0].fx[0][2]).toBe('G10');
});

it('gives each track one command column, and adds and removes them for a whole track', () => {
  const { parent, project, grid, feedback } = setup();
  const second = { ...project.patterns[0], id: 'second', notes: project.patterns[0].notes.map(track => track.map(column => ({ ...column, fx: [] as (string | null)[][] }))) };
  project.patterns.push(second); grid.render();
  expect(parent.querySelectorAll('.track-header[data-track="1"] .col-label.fx-label')).toHaveLength(1); // a track starts with one
  expect(parent.style.getPropertyValue('--track-columns')).toContain('30px 106px'); // a note (44), its instrument (26), the command (34) and 2
  const menu = (label: string) => { document.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)!.click(); };
  const open = () => parent.querySelector<HTMLButtonElement>('[aria-label="Lead columns"]')!.click();
  const fx = () => project.patterns.map(p => p.notes[0][0].fx.length);
  open(); menu('More command columns'); expect(fx()).toEqual([2, 1]); // the second pattern had none, and gains one
  expect(second.notes[0][0].fx[0]).toHaveLength(32);
  project.patterns[0].notes[0][0].fx[1][3] = 'S80';
  menu('Fewer command columns'); expect(fx()).toEqual([2, 1]); expect(feedback).toHaveBeenCalledWith('Clear the last command column before hiding it.');
  project.patterns[0].notes[0][0].fx[1][3] = null;
  menu('Fewer command columns'); expect(fx()).toEqual([1, 0]);
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
});

it('a click moves the cursor in either mode, and drag selects', () => {
  const { grid, parent } = setup();
  grid.setRecording(false);
  const at = (row: number) => parent.querySelector<HTMLElement>(`[data-note-row="${row}"][data-column="0"]`)!;
  at(3).dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 0 })); at(3).dispatchEvent(new MouseEvent('pointerup', { bubbles: true }));
  expect(parent.querySelector('[data-note-row="3"][data-column="0"]')!.classList.contains('cursor')).toBe(true);
});

it('copies a selection and pastes it at the cursor outside record mode', () => {
  const { grid, parent, project, click } = setup();
  click(0, 0); grid.setRecording(false);
  const copied = clipboard(parent, 'copy'); expect(copied.split('\t')[0]).toBe('C-4');
  parent.querySelector<HTMLElement>('#cell-5-0')!.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 0 }));
  clipboard(parent, 'paste', copied);
  expect(project.patterns[0].notes[0][0].notes[5]).toBe('C-4');
});

it('draws a line under a note until the next note, OFF or CUT', () => {
  const { project, grid, parent } = setup();
  const notes = project.patterns[0].notes[0][0].notes; notes.fill(null);
  notes[2] = 'C-4'; notes[5] = 'OFF'; notes[7] = 'E-4'; notes[9] = 'CUT'; grid.render();
  const held = () => [...parent.querySelectorAll('[data-column="0"].held')].map(cell => Number((cell as HTMLElement).dataset.noteRow));
  expect(held()).toEqual([3, 4, 8]);
});

it('stops at the edges of the pattern instead of wrapping', () => {
  const { grid, parent, key } = setup();
  key('ArrowUp'); key('ArrowLeft');
  expect(parent.querySelector('[data-note-row="0"][data-column="0"]')!.classList.contains('cursor')).toBe(true);
  grid.render();
});

it('opens a menu of selection actions on right click, which edit only in record mode', () => {
  const { project, parent, feedback, grid } = setup();
  const notes = project.patterns[0].notes[0][0].notes; notes.fill(null); notes[2] = 'C-4'; grid.render();
  const cell = parent.querySelector<HTMLElement>('[data-note-row="2"][data-column="0"]')!;
  cell.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  const item = (text: string) => [...document.querySelectorAll<HTMLButtonElement>('.menu button')].find(b => b.textContent!.startsWith(text))!;
  item('Transpose up an octave').click(); expect(notes[2]).toBe('C-5');
  parent.querySelector<HTMLElement>('[data-note-row="2"][data-column="0"]')!.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  grid.setRecording(false); item('Clear').click();
  expect(notes[2]).toBe('C-5'); expect(feedback).toHaveBeenCalledWith('Turn on record mode (Esc) to edit.');
});

it('opens a menu on a track header, with what the owner adds', () => {
  const { project, parent, grid } = setup();
  const duplicate = vi.fn(); grid.trackMenu = () => [{ text: 'Duplicate', run: duplicate }];
  parent.querySelector<HTMLElement>('.track-header[data-track="1"]')!.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  const item = (text: string) => [...document.querySelectorAll<HTMLButtonElement>('.menu button')].find(b => b.textContent === text)!;
  expect(['Rename', 'Colour…', 'Mute', 'Solo', 'Columns…', 'Duplicate'].every(text => item(text))).toBe(true);
  item('Mute').click(); expect(project.tracks[1].muted).toBe(true);
  parent.querySelector<HTMLElement>('.track-header[data-track="1"]')!.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
  item('Duplicate').click(); expect(duplicate).toHaveBeenCalled();
});

describe('dragging a selection', () => {
  const cell = (parent: HTMLElement, row: number, column = 0) => parent.querySelector<HTMLElement>(`[data-note-row="${row}"][data-column="${column}"]`)!;
  const drag = (parent: HTMLElement, from: HTMLElement, to: HTMLElement, options: MouseEventInit = {}) => {
    from.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 0 }));
    const original = document.elementFromPoint; document.elementFromPoint = () => to;
    parent.dispatchEvent(new MouseEvent('pointermove', { bubbles: true }));
    document.elementFromPoint = original;
    parent.dispatchEvent(new MouseEvent('pointerup', { bubbles: true, ...options }));
  };
  function selected() {
    const t = setup(); const notes = t.project.patterns[0].notes[0][0].notes; notes.fill(null);
    notes[0] = 'C-4'; notes[1] = 'E-4'; t.baseline(); t.grid.render();
    t.click(0, 0); t.key('ArrowDown', { shiftKey: true });
    return { ...t, notes };
  }
  it('moves the cells and leaves blanks behind, in one undo step', () => {
    const { parent, notes, grid } = selected();
    drag(parent, cell(parent, 0), cell(parent, 4));
    expect(notes[0]).toBeNull(); expect(notes[1]).toBeNull(); expect(notes[4]).toBe('C-4'); expect(notes[5]).toBe('E-4');
    grid.undo(); expect(notes[0]).toBe('C-4'); expect(notes[4]).toBeNull();
  });
  it('copies with Alt', () => {
    const { parent, notes } = selected();
    drag(parent, cell(parent, 1), cell(parent, 5), { altKey: true });
    expect(notes[0]).toBe('C-4'); expect(notes[4]).toBe('C-4'); expect(notes[5]).toBe('E-4');
  });
  it('handles a move that overlaps itself', () => {
    const { parent, notes } = selected();
    drag(parent, cell(parent, 0), cell(parent, 1));
    expect(notes[0]).toBeNull(); expect(notes[1]).toBe('C-4'); expect(notes[2]).toBe('E-4');
  });
  it('stops at the pattern bounds, and a plain click inside the selection only moves the cursor', () => {
    const { parent, notes } = selected();
    drag(parent, cell(parent, 0), cell(parent, 0));
    expect(notes[0]).toBe('C-4'); expect(parent.querySelector('[data-note-row="0"][data-column="0"]')!.classList.contains('cursor')).toBe(true);
    drag(parent, cell(parent, 0), cell(parent, 31));
    expect(notes[0]).toBe('C-4'); expect(notes[30]).toBeNull();
  });
});

function stripSetup() {
  const parent = document.createElement('div'); parent.tabIndex = 0; document.body.append(parent);
  const project = newProject(), edited = vi.fn(), feedback = vi.fn();
  project.returns.push({ name: 'Verb', fx: [], pan: 0, muted: false, volume_db: 0 });
  for (const pattern of project.patterns) { pattern.return_fx.push([emptyFxLane()]); pattern.commands.splice(0, 0, [Array(pattern.rows).fill(null)]); }
  const grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, edited, vi.fn(), vi.fn(), undefined, undefined, feedback);
  grid.recording = true; grid.render();
  return { parent, project, grid, edited, feedback, key: (key: string) => parent.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true })) };
}

it('draws each return and the master after the tracks, each with a command column and no notes', () => {
  const { parent } = stripSetup();
  const headers = [...parent.querySelectorAll('.track-header')].map(h => h.querySelector('.track-name')!.textContent);
  expect(headers).toEqual(['Lead', 'Bass', 'Drums', 'Texture', 'Verb', 'Master']);
  for (const column of [4, 5]) {
    const header = parent.querySelector(`.track-header[data-track="${column}"]`)!;
    expect(header.querySelectorAll('.col-label')).toHaveLength(1); expect(header.querySelector('.col-label')!.textContent).toBe('Cmd');
    expect(parent.querySelector(`#cell-0-${column}-0-cmd`)).not.toBeNull(); expect(parent.querySelector(`#cell-0-${column}`)).toBeNull();
  }
  expect(parent.querySelectorAll('.track-header[data-track="0"] .col-label.fx-label')).toHaveLength(1); // and a track starts with a command column of its own
  expect([...parent.querySelectorAll('[aria-label="Mute Verb"], [aria-label="Mute Master"], [aria-label="Solo Verb"]')].map(b => b.getAttribute('aria-label'))).toEqual(['Mute Verb']);
  // their headers size to their own column, so the menu button stays inside it
  expect(parent.querySelector('.track-header[data-track="5"]')!.classList.contains('strip')).toBe(true); expect(parent.querySelector('.track-header[data-track="0"]')!.classList.contains('strip')).toBe(false);
});

it('types commands into a track, a return and the master, and refuses ones the engine cannot play', () => {
  const { parent, project, feedback, key } = stripSetup();
  const type = (id: string, text: string) => {
    parent.querySelector<HTMLElement>(id)!.click(); const input = parent.querySelector<HTMLInputElement>('.lane-value')!; input.value = text;
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  };
  type('#cell-2-0-0-fx', 'l80'); expect(project.patterns[0].notes[0][0].fx[0][2]).toBe('L80'); // a track's own command column
  expect(parent.getAttribute('aria-activedescendant')).toBe('cell-3-0-0-fx'); // it moves down as a note does
  type('#cell-1-4-0-cmd', 'p00'); expect(project.patterns[0].commands[0][0][1]).toBe('P00'); // the return's
  expect(parent.querySelector('#cell-1-4-0-cmd')!.getAttribute('title')).toBe('P00 pans the track 100% left');
  type('#cell-0-5-0-cmd', '12ff'); expect(project.patterns[0].commands[1][0][0]).toBe('12FF'); // the master's
  type('#cell-4-0-0-fx', 'q10'); expect(project.patterns[0].notes[0][0].fx[0][4]).toBeNull(); expect(feedback).toHaveBeenLastCalledWith(expect.stringContaining('Commands are'));
  type('#cell-4-4-0-cmd', 's80'); expect(project.patterns[0].commands[0][0][4]).toBeNull(); expect(feedback).toHaveBeenLastCalledWith(expect.stringContaining('not a track command')); // a strip has no notes to start
  // the status line says what a command does as it is typed, before it is saved
  parent.querySelector<HTMLElement>('#cell-7-0-0-fx')!.click(); const input = parent.querySelector<HTMLInputElement>('.lane-value')!; input.value = '12FF'; input.dispatchEvent(new Event('input'));
  expect(feedback).toHaveBeenLastCalledWith(expect.stringContaining('sets parameter 2 of effect 1'));
  // typing a letter on the cell starts the edit, with room for four characters
  parent.querySelector<HTMLElement>('#cell-6-0-0-fx')!.click(); key('p');
  const typed = parent.querySelector<HTMLInputElement>('.lane-value')!; expect(typed.value).toBe('P'); expect(typed.maxLength).toBe(4);
});

it('adds and removes a return\'s command columns, and it keeps one', () => {
  const { parent, project, feedback } = stripSetup();
  const menu = (label: string) => { document.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)!.click(); };
  const count = () => project.patterns.map(p => p.commands[0].length);
  parent.querySelector<HTMLButtonElement>('[aria-label="Verb columns"]')!.click();
  expect(document.querySelector('[aria-label="More note columns"]')).toBeNull(); expect(document.querySelector<HTMLButtonElement>('[aria-label="Fewer command columns"]')!.disabled).toBe(true); // the one it keeps
  menu('More command columns'); expect(count()).toEqual([2]);
  project.patterns[0].commands[0][1][3] = 'L80';
  menu('Fewer command columns'); expect(count()).toEqual([2]); expect(feedback).toHaveBeenCalledWith('Clear the last command column before hiding it.');
  project.patterns[0].commands[0][1][3] = null;
  menu('Fewer command columns'); expect(count()).toEqual([1]);
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
});

it('records a moved control as an envelope point at the cursor row, in a lane of its own', () => {
  const { project, grid, edited, click } = setup();
  click(3, 0);
  grid.writeParameter(0, { kind: 'pan' }, 0.25);
  const lanes = project.patterns[0].fx[0];
  expect(lanes).toHaveLength(2); expect(lanes[1].target).toEqual({ kind: 'pan' }); // the first lane is the track's volume
  expect(lanes[1].envelope).toEqual({ enabled: true, mode: 'hold', points: [{ row: 3, value: 0.25 }] });
  expect(edited).toHaveBeenCalled();
  click(1, 0); grid.writeParameter(0, { kind: 'pan' }, -0.5);
  expect(lanes).toHaveLength(2); expect(lanes[1].envelope!.points).toEqual([{ row: 1, value: -0.5 }, { row: 3, value: 0.25 }]);
  click(1, 0); grid.writeParameter(0, { kind: 'volume' }, -6); expect(lanes).toHaveLength(2); expect(lanes[0].envelope!.points).toEqual([{ row: 1, value: -6 }]);
  click(1, 0); grid.writeParameter(0, { kind: 'pan' }, 0); expect(lanes[1].envelope!.points[0]).toEqual({ row: 1, value: 0 }); // the same row is replaced
});

it('keeps the keyboard in the grid when a header button redraws it', () => {
  const { parent } = setup();
  const mute = parent.querySelector<HTMLButtonElement>('[aria-label="Mute Lead"]')!; mute.focus();
  expect(document.activeElement).toBe(mute);
  mute.click();
  expect(document.activeElement).toBe(parent); // the button was replaced; the cursor did not go with it
});

it('holds a note for as long as its key is down: not again when the key repeats, and until it comes up or the window loses focus', () => {
  const { grid, parent, audition } = setup(undefined, () => 'tone');
  const release = vi.fn(); grid.releaseHeld = release;
  const down = (key: string, code: string, repeat = false) => parent.dispatchEvent(new KeyboardEvent('keydown', { key, code, repeat, bubbles: true, cancelable: true }));
  const up = (code: string) => document.dispatchEvent(new KeyboardEvent('keyup', { code, bubbles: true }));
  down('z', 'KeyZ'); expect(audition).toHaveBeenCalledOnce(); expect(audition).toHaveBeenLastCalledWith('tone', 'C-4', 0, { hold: 2096 });
  down('z', 'KeyZ', true); down('z', 'KeyZ', true); expect(audition).toHaveBeenCalledOnce(); expect(release).not.toHaveBeenCalled();
  down('x', 'KeyX'); expect(audition).toHaveBeenLastCalledWith('tone', 'D-4', 0, { hold: 2098 }); // a chord: each key is held on its own
  up('KeyZ'); expect(release).toHaveBeenCalledOnce(); expect(release).toHaveBeenLastCalledWith(2096);
  up('KeyZ'); expect(release).toHaveBeenCalledOnce(); // a key that is not held releases nothing
  window.dispatchEvent(new Event('blur')); expect(release).toHaveBeenLastCalledWith(2098); expect(release).toHaveBeenCalledTimes(2);
});

it('holds a note started from outside the grid too, and an entered note in record mode', () => {
  const { grid, audition, key } = setup(undefined, () => 'tone');
  const release = vi.fn(); grid.releaseHeld = release;
  document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', code: 'KeyZ', bubbles: true, cancelable: true }));
  grid.auditionKey(new KeyboardEvent('keydown', { key: 'x', code: 'KeyX', cancelable: true }));
  expect(audition).toHaveBeenLastCalledWith('tone', 'D-4', 0, { hold: 2098 });
  document.dispatchEvent(new KeyboardEvent('keyup', { code: 'KeyX' })); expect(release).toHaveBeenLastCalledWith(2098);
  grid.setRecording(true); audition.mockClear(); key('s', { code: 'KeyS' }); expect(audition).toHaveBeenLastCalledWith('tone', 'C#4', 0, { hold: 2097 });
  document.dispatchEvent(new KeyboardEvent('keyup', { code: 'KeyS' })); expect(release).toHaveBeenLastCalledWith(2097);
});
