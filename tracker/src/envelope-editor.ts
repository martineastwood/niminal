import type { EffectParameters, Envelope, FxLane, Pattern, Project } from './project';
import { envelopeValue, formatValue, laneParameter, parseValue, targetKey } from './automation';

const svgNS = 'http://www.w3.org/2000/svg';
const blank = (): Envelope => ({ enabled: false, mode: 'linear', points: [] });
type Snapshot = Envelope | undefined;

export class EnvelopeEditor {
  private track = 0;
  private lane = 0;
  private selected = -1;
  private tool = 'points';
  private snap = 1;
  private history = new Map<string, { past: Snapshot[]; future: Snapshot[] }>();
  private graph: SVGSVGElement | null = null;
  private dragging = false;
  private playRow: number | null = null;
  private dragKey = '';
  private cancelDrag: (() => void) | null = null;

  constructor(private parent: HTMLElement, private project: () => Project, private pattern: () => Pattern,
    private parameters: () => EffectParameters, private changed: () => void) {}

  reset() { this.cancelDrag?.(); this.history.clear(); this.track = this.lane = 0; this.selected = -1; }
  select(track: number, lane = this.lane) { if (this.dragging) this.cancelDrag?.(); this.track = track; this.lane = Math.min(lane, this.pattern().fx[track].length - 1); this.selected = -1; this.render(); }
  private get column(): FxLane { return this.pattern().fx[this.track][this.lane]; }
  private get info() { return laneParameter(this.column.target, this.track, this.project(), this.parameters()); }
  private get envelope() { return this.column.envelope ?? blank(); }
  private get key() { return `${this.pattern().id}:${this.track}:${this.lane}:${targetKey(this.column.target)}`; }
  private get edits() { if (!this.history.has(this.key)) this.history.set(this.key, { past: [], future: [] }); return this.history.get(this.key)!; }
  private snapshot(): Snapshot { return this.column.envelope ? structuredClone(this.column.envelope) : undefined; }
  private commit(before: Snapshot) {
    if (JSON.stringify(before) !== JSON.stringify(this.column.envelope)) { this.edits.past.push(before); if (this.edits.past.length > 100) this.edits.past.shift(); this.edits.future = []; this.changed(); }
    this.render();
  }
  private mutate(change: (envelope: Envelope) => void) { const before = this.snapshot(); this.column.envelope ??= blank(); change(this.column.envelope); this.commit(before); }
  private undo(redo = false) {
    const from = redo ? this.edits.future : this.edits.past, to = redo ? this.edits.past : this.edits.future;
    if (!from.length) return; to.push(this.snapshot()); const restored = from.pop();
    if (restored) this.column.envelope = restored; else delete this.column.envelope;
    this.selected = -1; this.changed(); this.render();
  }
  private removePoint() { if (this.selected < 0) return; this.mutate(e => e.points.splice(this.selected, 1)); this.selected = -1; this.render(); }

  render() {
    if (this.dragging) { if (this.key !== this.dragKey) this.cancelDrag?.(); else return; }
    this.track = Math.min(this.track, this.project().tracks.length - 1); this.lane = Math.min(this.lane, this.pattern().fx[this.track].length - 1);
    const info = this.info, envelope = this.envelope;
    this.parent.innerHTML = `<div class="automation-toolbar"><strong data-track-name></strong><select data-lane aria-label="Automation parameter lane"></select><label><input data-enabled type="checkbox"> Use envelope</label><label>Shape <select data-mode aria-label="Envelope shape"><option value="linear">Linear</option><option value="hold">Hold</option></select></label><label>Tool <select data-tool aria-label="Envelope tool"><option value="points">Points</option><option value="draw">Draw</option></select></label><label>Snap <select data-snap aria-label="Envelope snap"><option value="1">Row</option><option value="beat">Beat</option><option value="0.0625">1/16 row</option></select></label><button data-undo>Undo</button><button data-redo>Redo</button><button data-clear>Clear</button></div><div class="automation-graph" tabindex="0" role="group" aria-label="Automation envelope editor"></div><div class="automation-point"><label>Row <input data-row type="number" step="0.0625" min="0" aria-label="Automation point row"></label><label>Value <input data-value aria-label="Automation point value"></label><button data-delete>Delete</button><span data-help></span></div>`;
    const find = <T extends HTMLElement>(selector: string) => this.parent.querySelector<T>(selector)!;
    find('[data-track-name]').textContent = `${String(this.track + 1).padStart(2, '0')} ${this.project().tracks[this.track].name}`;
    const lanes = find<HTMLSelectElement>('[data-lane]'); this.pattern().fx[this.track].forEach((lane, i) => lanes.add(new Option(laneParameter(lane.target, this.track, this.project(), this.parameters())?.label ?? 'Choose a lane parameter above', String(i)))); lanes.value = String(this.lane); lanes.onchange = () => this.select(this.track, Number(lanes.value));
    const enabled = find<HTMLInputElement>('[data-enabled]'); enabled.checked = envelope.enabled; enabled.disabled = !info; enabled.onchange = () => this.mutate(e => { e.enabled = enabled.checked; });
    const mode = find<HTMLSelectElement>('[data-mode]'); mode.value = envelope.mode; mode.onchange = () => this.mutate(e => { e.mode = mode.value as Envelope['mode']; });
    const tool = find<HTMLSelectElement>('[data-tool]'); tool.value = this.tool; tool.onchange = () => { this.tool = tool.value; };
    const snap = find<HTMLSelectElement>('[data-snap]'); snap.value = this.snap === this.project().rows_per_beat && this.snap !== 1 ? 'beat' : String(this.snap); snap.onchange = () => { this.snap = snap.value === 'beat' ? this.project().rows_per_beat : Number(snap.value); };
    find<HTMLButtonElement>('[data-undo]').disabled = !this.edits.past.length; find('[data-undo]').onclick = () => this.undo();
    find<HTMLButtonElement>('[data-redo]').disabled = !this.edits.future.length; find('[data-redo]').onclick = () => this.undo(true);
    find('[data-clear]').onclick = () => this.mutate(e => { e.points = []; e.enabled = false; this.selected = -1; });
    const point = envelope.points[this.selected]; const row = find<HTMLInputElement>('[data-row]'), value = find<HTMLInputElement>('[data-value]');
    row.max = String(this.pattern().rows); row.disabled = value.disabled = !point || !info;
    if (point && info) { row.value = String(point.row); value.value = formatValue(point.value, info); }
    row.onchange = () => { const n = row.valueAsNumber; if (!Number.isFinite(n) || n < 0 || n > this.pattern().rows || envelope.points.some((p,i) => i !== this.selected && p.row === n)) { this.render(); return; } this.mutate(e => { const p = e.points[this.selected]; p.row = n; e.points.sort((a,b) => a.row-b.row); this.selected = e.points.indexOf(p); }); };
    value.onchange = () => { const parsed = info && parseValue(value.value, info); if (parsed === null || !info) { this.render(); return; } this.mutate(e => { e.points[this.selected].value = Number(parsed); }); };
    find<HTMLButtonElement>('[data-delete]').disabled = !point; find('[data-delete]').onclick = () => this.removePoint();
    find('[data-help]').textContent = info ? '' : 'Choose a parameter in a pattern lane.';
    const area = find('[aria-label="Automation envelope editor"]');
    area.onkeydown = event => { if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'z') { event.preventDefault(); this.undo(event.shiftKey); } else if (event.key === 'Delete' || event.key === 'Backspace') { event.preventDefault(); this.removePoint(); } };
    this.graph = document.createElementNS(svgNS, 'svg'); this.graph.setAttribute('viewBox', '0 0 1000 200'); this.graph.setAttribute('preserveAspectRatio', 'none'); area.append(this.graph);
    this.graph.onpointerdown = event => this.begin(event);
    this.graph.oncontextmenu = event => { event.preventDefault(); const i = this.nearest(event); if (i >= 0) { this.selected = i; this.removePoint(); } };
    this.draw();
  }
  private xy(row: number, value: number) { const p = this.info!; return [76 + row / this.pattern().rows * 906, 174 - (value-p.min)/(p.max-p.min || 1)*150]; }
  private coordinate(event: PointerEvent | MouseEvent) {
    const rect = this.graph!.getBoundingClientRect(), p = this.info!;
    const x = (event.clientX-rect.left)/rect.width*1000, y = (event.clientY-rect.top)/rect.height*200;
    return { row: Math.max(0,Math.min(this.pattern().rows,Math.round((x-76)/906*this.pattern().rows/this.snap)*this.snap)), value: Math.max(p.min,Math.min(p.max,p.min+(174-y)/150*(p.max-p.min))) };
  }
  private nearest(event: PointerEvent | MouseEvent) {
    if (!this.info) return -1;
    const rect = this.graph!.getBoundingClientRect();
    return this.envelope.points.findIndex(p => { const [x,y] = this.xy(p.row,p.value); return Math.hypot(rect.left+x/1000*rect.width-event.clientX, rect.top+y/200*rect.height-event.clientY) < 10; });
  }
  private begin(event: PointerEvent) {
    if (event.button !== 0 || !this.info || this.info.max <= this.info.min) return;
    event.preventDefault(); const graph = this.graph!, column = this.column, before = this.snapshot();
    graph.parentElement?.focus(); let index = this.nearest(event), last = this.coordinate(event);
    column.envelope ??= blank(); column.envelope.enabled = true; this.dragging = true; this.dragKey = this.key;
    const put = (point: {row:number;value:number}) => { const points = column.envelope!.points; const existing = points.findIndex(p => p.row === point.row); if (existing >= 0) points[existing] = point; else points.push(point); points.sort((a,b) => a.row-b.row); return points.indexOf(point); };
    if (index < 0 || this.tool === 'draw') index = put(last); this.selected = index; graph.setPointerCapture?.(event.pointerId); this.draw();
    graph.onpointermove = move => {
      const point = this.coordinate(move), points = column.envelope!.points;
      if (this.tool === 'draw') { const lo = Math.min(last.row,point.row), hi = Math.max(last.row,point.row); for (let row = lo; row <= hi + 0.00001; row += this.snap) { const fraction = hi === lo ? 1 : (row-last.row)/(point.row-last.row); put({ row, value: last.value+(point.value-last.value)*fraction }); } this.selected = put(point); }
      else { points.splice(this.selected,1); this.selected = put(point); }
      last = point; this.draw();
    };
    const finish = (commit: boolean) => { graph.onpointermove = graph.onpointerup = graph.onpointercancel = null; this.dragging = false; this.cancelDrag = null; if (commit) this.commit(before); else { if (before) column.envelope = before; else delete column.envelope; this.render(); } };
    this.cancelDrag = () => finish(false);
    graph.onpointerup = () => { finish(true); this.parent.querySelector<HTMLElement>('.automation-graph')?.focus(); }; graph.onpointercancel = () => finish(false);
  }
  private draw() {
    if (!this.graph) return; this.graph.replaceChildren(); const info = this.info;
    const add = (name: string, attrs: Record<string,string|number>, text?: string) => { const n = document.createElementNS(svgNS,name); for (const [k,v] of Object.entries(attrs)) n.setAttribute(k,String(v)); if (text) n.textContent = text; this.graph!.append(n); return n; };
    add('rect',{x:76,y:24,width:906,height:150,fill:'#1b2027'});
    for (let row=0;row<=this.pattern().rows;row+=this.project().rows_per_beat) { const x=76+row/this.pattern().rows*906; add('line',{x1:x,x2:x,y1:24,y2:174,stroke:'#3b424c'}); if (row % Math.max(this.project().rows_per_beat,Math.ceil(this.pattern().rows/16/this.project().rows_per_beat)*this.project().rows_per_beat)===0) add('text',{x,y:194,fill:'#9aa7b7','font-size':10,'text-anchor':'middle'},String(row)); }
    if (info) {
      for (let i=0;i<=4;i++) { const v=info.min+(info.max-info.min)*i/4, [,y]=this.xy(0,v); add('line',{x1:76,x2:982,y1:y,y2:y,stroke:'#303741'}); add('text',{x:68,y:y+3,fill:'#a9b4c3','font-size':10,'text-anchor':'end'},formatValue(v,info)); }
      const envelope = this.envelope; let path='';
      for (let step=0;step<=512;step++) { const row=this.pattern().rows*step/512, value=envelopeValue(envelope,row,info.unit==='db'); if(value===null) continue; const [x,y]=this.xy(row,value); path+=`${path?'L':'M'}${x},${y} `; }
      add('path',{d:path,fill:'none',stroke:envelope.enabled?'#e5b47b':'#738293','stroke-width':2,'vector-effect':'non-scaling-stroke'});
      envelope.points.forEach((p,i)=> { if(p.row>this.pattern().rows) return; const [x,y]=this.xy(p.row,p.value); const circle=add('circle',{cx:x,cy:y,r:i===this.selected?5:3.5,fill:i===this.selected?'#fff2c9':'#e5b47b'}); const title=document.createElementNS(svgNS,'title'); title.textContent=`Row ${p.row}: ${formatValue(p.value,info)}`; circle.append(title); });
    }
    add('line',{'data-playhead':'',x1:76,x2:76,y1:24,y2:174,stroke:'#91c4dd','stroke-width':1}); this.paint(this.playRow);
  }
  paint(row: number | null) { this.playRow=row; const line=this.graph?.querySelector<SVGLineElement>('[data-playhead]'); if(!line)return; line.style.display=row===null?'none':''; if(row!==null) { const x=76+row/this.pattern().rows*906; line.setAttribute('x1',String(x)); line.setAttribute('x2',String(x)); } }
}
