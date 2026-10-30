// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { EnvelopeEditor } from './envelope-editor';
import { newProject } from './project';
vi.stubGlobal('Option', function (text: string, value: string) { const option=document.createElement('option'); option.textContent=text; option.value=value; return option; });
afterEach(() => { document.body.replaceChildren(); vi.restoreAllMocks(); });
it('draws snapped points, edits exact values, and undoes a whole gesture', () => {
  const project=newProject(), parent=document.createElement('div'), changed=vi.fn(); document.body.append(parent);
  const editor=new EnvelopeEditor(parent,()=>project,()=>project.patterns[0],()=>({}),changed); editor.render();
  const graph=parent.querySelector('svg')!;
  vi.spyOn(graph,'getBoundingClientRect').mockReturnValue({left:0,top:0,width:1000,height:200} as DOMRect);
  graph.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,button:0,pointerId:1,clientX:76,clientY:174}));
  graph.dispatchEvent(new PointerEvent('pointermove',{bubbles:true,pointerId:1,clientX:529,clientY:24}));
  graph.dispatchEvent(new PointerEvent('pointerup',{bubbles:true,pointerId:1}));
  const lane=project.patterns[0].fx[0][0];
  expect(lane.envelope?.points).toEqual([{row:16,value:6}]); expect(lane.envelope?.enabled).toBe(true); expect(changed).toHaveBeenCalledOnce();
  const value=parent.querySelector<HTMLInputElement>('[data-value]')!; value.value='-6 dB'; value.dispatchEvent(new Event('change')); expect(lane.envelope?.points[0].value).toBe(-6);
  parent.querySelector<HTMLButtonElement>('[data-undo]')!.click(); expect(lane.envelope?.points[0].value).toBe(6);
  parent.querySelector<HTMLButtonElement>('[data-undo]')!.click(); expect(lane.envelope).toBeUndefined();
  parent.querySelector<HTMLButtonElement>('[data-redo]')!.click(); expect(lane.envelope?.points[0].row).toBe(16);
});

it('starts automation on any parameter from the lane list, using an empty lane before making one', () => {
  const project = newProject(), parent = document.createElement('div'), changed = vi.fn(); document.body.append(parent);
  project.patterns[0].fx[0][0].target = null;
  const editor = new EnvelopeEditor(parent, () => project, () => project.patterns[0], () => ({}), changed); editor.render();
  const lanes = () => project.patterns[0].fx[0], pick = (value: string) => { const select = parent.querySelector<HTMLSelectElement>('[data-lane]')!; select.value = value; select.dispatchEvent(new Event('change')); };
  const offered = () => [...parent.querySelectorAll<HTMLOptionElement>('[data-lane] optgroup option')].map(o => o.textContent);
  expect(offered()).toEqual(['Track → Vol', 'Track → Pan']);
  pick(`add:${JSON.stringify({ kind: 'pan' })}`);
  expect(lanes()).toHaveLength(1); expect(lanes()[0].target).toEqual({ kind: 'pan' }); // the empty first lane was used
  expect(offered()).toEqual(['Track → Vol']);
  pick(`add:${JSON.stringify({ kind: 'volume' })}`);
  expect(lanes().map(lane => lane.target)).toEqual([{ kind: 'pan' }, { kind: 'volume' }]); expect(changed).toHaveBeenCalledTimes(2);
});

it('chooses a lane\'s parameter, adds and removes lanes, and refuses what would lose an envelope', () => {
  const project = newProject(), parent = document.createElement('div'), changed = vi.fn(); document.body.append(parent);
  project.definitions.push({ id: 'bell', name: 'Bell', kind: 'instr', source: 'instr bell(freq: hz) { osc(sine, freq) }' });
  const parameters = { tone: [{ name: 'cutoff', min: 200, max: 8000, default: 2000, unit: 'hz' }], bell: [{ name: 'ring', min: 0, max: 1, default: 0.5, unit: null }] };
  const editor = new EnvelopeEditor(parent, () => project, () => project.patterns[0], () => parameters, changed); editor.render();
  const lanes = () => project.patterns[0].fx[0];
  const options = () => [...parent.querySelectorAll<HTMLOptionElement>('[data-target] option')].map(o => o.textContent);
  expect(options()).toEqual(['Choose a parameter…', 'Vol', 'Pan', 'cutoff']); // Lead plays the Warm saw, which has a cutoff; Bell is on no track
  const target = parent.querySelector<HTMLSelectElement>('[data-target]')!;
  target.value = JSON.stringify({ kind: 'pan' }); target.dispatchEvent(new Event('change'));
  expect(lanes()[0].target).toEqual({ kind: 'pan' }); expect(changed).toHaveBeenCalledOnce();
  // another lane, and the same parameter is not offered twice
  parent.querySelector<HTMLButtonElement>('[data-add-lane]')!.click();
  expect(lanes()).toHaveLength(2); expect(lanes()[1].target).toBeNull();
  parent.querySelector<HTMLSelectElement>('[data-target]')!.value = JSON.stringify({ kind: 'pan' }); parent.querySelector<HTMLSelectElement>('[data-target]')!.dispatchEvent(new Event('change'));
  expect(lanes()[1].target).toBeNull(); expect(parent.querySelector('[data-help]')!.textContent).toContain('already has a lane');
  parent.querySelector<HTMLSelectElement>('[data-target]')!.value = JSON.stringify({ kind: 'volume' }); parent.querySelector<HTMLSelectElement>('[data-target]')!.dispatchEvent(new Event('change'));
  expect(lanes()[1].target).toEqual({ kind: 'volume' });
  // a lane with an envelope keeps its parameter and cannot be removed until cleared
  lanes()[1].envelope = { enabled: true, mode: 'linear', points: [{ row: 2, value: -6 }] };
  parent.querySelector<HTMLSelectElement>('[data-target]')!.value = ''; parent.querySelector<HTMLSelectElement>('[data-target]')!.dispatchEvent(new Event('change'));
  expect(lanes()[1].target).toEqual({ kind: 'volume' }); expect(parent.querySelector('[data-help]')!.textContent).toContain('Clear this lane');
  parent.querySelector<HTMLButtonElement>('[data-remove-lane]')!.click(); expect(lanes()).toHaveLength(2);
  delete lanes()[1].envelope; editor.render();
  parent.querySelector<HTMLButtonElement>('[data-remove-lane]')!.click(); expect(lanes()).toHaveLength(1);
  expect(parent.querySelector<HTMLButtonElement>('[data-remove-lane]')!.disabled).toBe(true); // the last one stays
  for (let i = 0; i < 15; i++) parent.querySelector<HTMLButtonElement>('[data-add-lane]')!.click();
  expect(lanes()).toHaveLength(16); expect(parent.querySelector<HTMLButtonElement>('[data-add-lane]')!.disabled).toBe(true);
});
