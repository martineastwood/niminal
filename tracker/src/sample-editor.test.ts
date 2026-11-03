// @vitest-environment happy-dom
import { expect, it, vi } from 'vitest';
import { SampleEditor } from './sample-editor';
import { samplerSource } from './instruments';
import type { Definition } from './project';

it('keeps selection in source frames while zooming, and prepares new audio without losing the original', async () => {
  const definition: Definition = { id: 'a', name: 'Hit', kind: 'instr', source: samplerSource('hit', 'samples/hit.wav') };
  const actions = { analyze: vi.fn(async () => ({ frames: 48000, rate: 48000, channels: 2, peak: 0.5, silence_start: 0.1, silence_end: 0.9, crossings: [0.25, 0.5], loop_start: 0.25, loop_end: 0.75 })), prepare: vi.fn(async () => 'samples/edit.wav'), edit: vi.fn((_d: Definition, _s: string) => {}), detect: vi.fn(async () => [0, 0.25]), preview: vi.fn() };
  const editor = new SampleEditor(actions, vi.fn()); await editor.refresh(definition);
  editor.select(0.25, 0.75);
  const click = (label: string) => [...editor.root.querySelectorAll('button')].find(b => b.textContent === label)!.click();
  click('Zoom selection'); expect([editor.viewStart, editor.viewEnd]).toEqual([0.25, 0.75]);
  expect(editor.root.querySelector<HTMLInputElement>('[aria-label="Selection start frame"]')!.value).toBe('12000');
  click('Hear selection'); expect(actions.preview).toHaveBeenCalledWith('samples/hit.wav', 0.25, 0.75, false);
  click('Keep selection'); await vi.waitFor(() => expect(actions.edit).toHaveBeenCalled());
  expect(actions.prepare).toHaveBeenCalledWith('samples/hit.wav', expect.objectContaining({ start: 0.25, end: 0.75 }));
  expect(actions.edit.mock.calls[0][1]).toContain('// Original sample: samples/hit.wav');
  expect(actions.edit.mock.calls[0][1]).toContain('sample("samples/edit.wav")');
});
it('discards late analysis and preparation results after switching instruments', async () => {
  const first: Definition = { id: 'a', name: 'A', kind: 'instr', source: samplerSource('a', 'samples/a.wav') };
  const second: Definition = { ...first, id: 'b', source: samplerSource('b', 'samples/b.wav') };
  let finish!: (file: string) => void;
  const edit = vi.fn();
  const editor = new SampleEditor({ prepare: () => new Promise(resolve => { finish = resolve; }), edit, detect: async () => [] }, vi.fn());
  await editor.refresh(first);
  [...editor.root.querySelectorAll('button')].find(b => b.textContent === 'Reverse')!.click();
  await editor.refresh(second); finish('samples/edit.wav'); await Promise.resolve();
  expect(edit).not.toHaveBeenCalled();
});
