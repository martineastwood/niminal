// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { PatternGrid } from './grid';
import { newProject } from './project';

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
  expect(project.patterns[0].notes[0][1]).toBe('C-4');
  expect(audition).toHaveBeenCalledWith('tone', 'C-4', 0);
  key('z', { metaKey: true }); expect(project.patterns[0].notes[0][1]).toBeNull();
  key('z', { metaKey: true, shiftKey: true }); expect(project.patterns[0].notes[0][1]).toBe('C-4');
  expect(edited).toHaveBeenCalledTimes(3);
});

it('copies a rectangular selection and treats a multi-cell paste as one undoable edit', () => {
  const { parent, project, key, click } = setup();
  click(0, 0); key('ArrowDown', { shiftKey: true }); key('ArrowDown', { shiftKey: true }); key('ArrowRight', { shiftKey: true });
  expect(clipboard(parent, 'copy')).toBe('C-4\t---\n---\t---\n---\t---');
  key('Delete'); expect(project.patterns[0].notes[0][0]).toBeNull();
  key('z', { ctrlKey: true }); expect(project.patterns[0].notes[0][0]).toBe('C-4');
  clipboard(parent, 'paste', 'F#3\tA-3\n---\tC-4');
  expect(project.patterns[0].notes[0][0]).toBe('F#3'); expect(project.patterns[0].notes[1][0]).toBe('A-3');
  expect(project.patterns[0].notes[1][1]).toBe('C-4');
  key('z', { ctrlKey: true });
  expect(project.patterns[0].notes[0][0]).toBe('C-4'); expect(project.patterns[0].notes[1][0]).toBeNull(); expect(project.patterns[0].notes[1][1]).toBeNull();
});

it('clips a pasted block at the pattern edge and ignores malformed or empty clipboard data', () => {
  const { parent, project, click, grid } = setup();
  click(31, 3); clipboard(parent, 'paste', 'D-3\tE-3\nF-3\tG-3');
  expect(project.patterns[0].notes[3][31]).toBe('D-3');
  expect(project.patterns[0].notes[3]).toHaveLength(32);
  clipboard(parent, 'paste', 'not notes'); clipboard(parent, 'paste', '');
  expect(project.patterns[0].notes[3][31]).toBe('D-3');
  grid.undo(); expect(project.patterns[0].notes[3][31]).toBeNull();
});

it('keeps selector typing out of the note grid and space playback local to the grid', () => {
  const { parent, project, key, play } = setup();
  const select = parent.querySelector('select')!;
  select.dispatchEvent(new KeyboardEvent('keydown', { key: 's', bubbles: true }));
  expect(project.patterns[0].notes[0][0]).toBe('C-4');
  key(' '); expect(play).toHaveBeenCalledTimes(1);
  const code = document.createElement('textarea'); document.body.append(code);
  code.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true }));
  expect(play).toHaveBeenCalledTimes(1);
});

it('keeps note undo history independent for each pattern and supports longer patterns', () => {
  const parent = document.createElement('div'); document.body.append(parent);
  const project = newProject();
  const second = { id: 'second', name: 'Second', rows: 64, notes: Array.from({ length: 4 }, () => Array(64).fill(null)) };
  project.patterns.push(second);
  let pattern = project.patterns[0];
  const grid = new PatternGrid(parent, () => project, () => pattern, () => 4, vi.fn(), vi.fn(), vi.fn(), vi.fn());
  const key = (key: string) => parent.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
  grid.render(); key('s'); // change first pattern's C to C#
  pattern = second; grid.render();
  parent.querySelector<HTMLElement>('#cell-63-0')!.click(); key('x');
  expect(second.notes[0][63]).toBe('D-4');
  pattern = project.patterns[0]; grid.render(); grid.undo();
  expect(project.patterns[0].notes[0][0]).toBe('C-4');
  expect(second.notes[0][63]).toBe('D-4');
  pattern = second; grid.render(); grid.undo(); expect(second.notes[0][63]).toBeNull();
});

it('opens track devices without entering notes and navigates beyond the starter tracks', () => {
  const parent = document.createElement('div'); document.body.append(parent);
  const project = newProject(), selected = vi.fn(), open = vi.fn();
  for (let i = 4; i < 8; i++) {
    project.tracks.push({ name: `Track ${i + 1}`, instrument: 'tone', effect: null, volume_db: 0, muted: false });
    project.patterns[0].notes.push(Array(32).fill(null));
  }
  const grid = new PatternGrid(parent, () => project, () => project.patterns[0], () => 4, vi.fn(), vi.fn(), vi.fn(), vi.fn(), selected, open);
  grid.render();
  parent.querySelector<HTMLButtonElement>('[aria-label="Show Track 8 effects"]')!.click();
  expect(parent.querySelector('#cell-0-7')?.getAttribute('aria-selected')).toBe('true');
  expect(grid.activeTrack).toBe(7); expect(open).toHaveBeenCalledWith(7, 'effects');
  parent.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', bubbles: true, cancelable: true }));
  expect(project.patterns[0].notes[7][0]).toBe('C-4');
  parent.querySelector<HTMLButtonElement>('[aria-label="Mute Track 8"]')!.click();
  expect(project.tracks[7].muted).toBe(true);
  parent.querySelector<HTMLButtonElement>('[aria-label="Edit Track 8 instrument"]')!.click();
  expect(open).toHaveBeenLastCalledWith(7, 'instrument');
  parent.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true, cancelable: true }));
  expect(grid.activeTrack).toBe(0); expect(selected).toHaveBeenLastCalledWith(0);
});
