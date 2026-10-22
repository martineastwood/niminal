// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { EnvelopeEditor } from './envelope-editor';
import { newProject } from './project';
vi.stubGlobal('Option', function (text: string, value: string) { const option=document.createElement('option'); option.textContent=text; option.value=value; return option; });
afterEach(() => { document.body.replaceChildren(); vi.restoreAllMocks(); });
it('draws snapped points, edits exact values, and undoes a whole gesture', () => {
  const project=newProject(), parent=document.createElement('div'), changed=vi.fn(); document.body.append(parent);
  const editor=new EnvelopeEditor(parent,()=>project,()=>project.patterns[0],()=>({}),changed,vi.fn()); editor.render();
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
