import { invoke, isTauri } from '@tauri-apps/api/core';
import { EditorView } from '@codemirror/view';
import { editorState, connectLanguageServer } from './editor';
import { newProject, readProject, storageKey, type Definition, type Pattern } from './project';
import { PatternGrid } from './grid';
import { renderMixer } from './mixer';
import { audioCommand, audioStatus, observeAudio } from './audio';
import './style.css';

const app = document.querySelector<HTMLDivElement>('#app')!;
app.innerHTML = `
  <header><div class="brand">niminal <span>tracker</span></div><span class="badge">ALPHA</span><div class="project-actions"><button id="import">Open project</button><button id="export">Save project</button></div></header>
  <main>
    <aside><div class="section-title">SOUND LIBRARY</div><div id="definitions"></div><div class="add-buttons"><button id="new-instrument">+ Instrument</button><button id="new-effect">+ Effect</button></div><p class="aside-help">Select a sound to edit its code.<br>Project copies are saved locally.</p></aside>
    <section class="workspace">
      <div class="engine-toolbar"><label>Engine port <input id="engine-port" type="number" min="1" max="65535" value="7401"></label><input id="engine-token" type="password" placeholder="Token (optional)" aria-label="Engine token"><button id="connect">Connect</button><span id="audio-status">Start niminal server, then connect</span></div>
      <div class="pattern-toolbar"><div><strong>Pattern 00</strong><span class="muted">32 rows · 4 tracks</span></div><label>Octave <input id="octave" type="number" min="0" max="8" value="4"></label><label>BPM <input id="bpm" type="number" min="20" max="300" step="1"></label><label>Rows/beat <input id="rows-per-beat" type="number" min="1" max="16"></label><button id="undo">Undo notes</button><button id="redo">Redo</button><button id="play">Play loop</button><button id="stop">Stop</button></div>
      <div id="grid" tabindex="0" role="grid" aria-label="Pattern editor"></div>
      <div class="grid-help">Z S X D C V G B H N J M: notes · Arrows: move · Delete: clear · Shift+arrows: select · Cmd/Ctrl+C/V: copy/paste · Cmd/Ctrl+Z: undo · Space: play/stop · F6: switch editor</div>
      <div id="mixer" aria-label="Track mixer"></div>
      <section class="instrument-editor"><div class="editor-toolbar"><div><span id="definition-kind" class="badge"></span><input id="definition-name" aria-label="Definition name"></div><div><span id="apply-status" class="muted"></span><button id="apply">Apply</button><button id="audition">Audition</button><button id="expand">Expand editor</button></div></div><div id="code"></div><div class="editor-help">Hover for documentation or errors · Ctrl+Space: completion · Shift+Alt+F: format</div></section>
    </section>
  </main><footer><span id="status">Starting editor…</span><span id="save-status">Local autosave</span></footer>`;

const element = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const status = (text: string) => { element('status').textContent = text; };
let project = newProject();
try { const raw = localStorage.getItem(storageKey); if (raw) project = readProject(raw); }
catch (error) { status(`Could not restore project: ${error}`); }
let selected = project.definitions[0];
let editor: EditorView;
const states = new Map<string, ReturnType<typeof editorState>>();
const appliedSources = new Map<string, string>();
let updateTimer: number | undefined;
let transportRevision = 0;
let starting = false;
let audioError: string | null = null;

function save() {
  try { localStorage.setItem(storageKey, JSON.stringify(project)); element('save-status').textContent = 'Saved locally'; }
  catch { element('save-status').textContent = 'Autosave failed · use Save project'; }
}

function selectDefinition(definition: Definition) {
  if (editor) states.set(selected.id, editor.state);
  selected = definition;
  const state = states.get(definition.id) ?? editorState(`file:///niminal-tracker/${definition.id}.nml`, definition.source, source => { definition.source = source; save(); renderApplyStatus(); });
  if (editor) editor.setState(state);
  else editor = new EditorView({ state, parent: element('code') });
  element<HTMLInputElement>('definition-name').value = definition.name;
  element('definition-kind').textContent = definition.kind === 'instr' ? 'INSTRUMENT' : 'EFFECT';
  renderDefinitions(); renderApplyStatus();
}
function renderDefinitions() {
  const list = element('definitions');
  list.replaceChildren();
  for (const [index, definition] of project.definitions.entries()) {
    const button = document.createElement('button');
    button.className = `definition ${definition.id === selected.id ? 'selected' : ''}`;
    button.textContent = `${String(index).padStart(2, '0')}  ${definition.name}`;
    const kind = document.createElement('small'); kind.textContent = definition.kind;
    button.append(kind);
    button.onclick = () => { selectDefinition(definition); editor.focus(); };
    list.append(button);
  }
}
function pattern(): Pattern { return { bpm: project.bpm, rows_per_beat: project.rows_per_beat, tracks: project.tracks }; }
async function action(work: () => Promise<unknown>) {
  audioError = null;
  try { await work(); }
  catch (error) { audioError = String(error); element('audio-status').textContent = audioError; }
  finally { renderApplyStatus(); }
}
async function applyDefinition(definition: Definition) {
  const source = definition.source;
  await audioCommand({ op: 'apply', id: definition.id, source, kind: definition.kind });
  appliedSources.set(definition.id, source);
  renderApplyStatus();
}
async function ensureSound(id: string) {
  const definition = project.definitions.find(d => d.id === id);
  if (!definition) throw new Error('Sound definition is missing');
  const ready = definition.kind === 'instr' ? audioStatus.instruments[id] : audioStatus.effects[id];
  if (!ready) await applyDefinition(definition);
}
async function ensureAssignments() {
  for (const track of project.tracks) {
    if (track.instrument) await ensureSound(track.instrument);
    if (track.effect) await ensureSound(track.effect);
  }
}
function mixerView() {
  renderMixer(element('mixer'), project, editedPattern, id => {
    const definition = project.definitions.find(d => d.id === id);
    if (definition) selectDefinition(definition);
  });
}
async function stopPlayback() {
  transportRevision++; starting = false;
  await audioCommand({ op: 'stop' });
}
async function togglePlay() {
  if (audioStatus.playing || starting) { await stopPlayback(); return; }
  const revision = ++transportRevision;
  starting = true;
  try {
    for (const track of project.tracks) {
      if (track.instrument) await ensureSound(track.instrument);
      if (track.effect) await ensureSound(track.effect);
      if (revision !== transportRevision) return;
    }
    await audioCommand({ op: 'play', pattern: pattern() });
  } finally { if (revision === transportRevision) starting = false; }
}
function editedPattern() {
  save();
  clearTimeout(updateTimer);
  updateTimer = window.setTimeout(() => {
    if (audioStatus.connected) void action(async () => {
      await ensureAssignments();
      await audioCommand({ op: 'update', pattern: pattern() });
    });
  }, 50);
}
async function audition(id: string, note: string, track: number) {
  await ensureSound(id); await ensureAssignments();
  await audioCommand({ op: 'update', pattern: pattern() });
  await audioCommand({ op: 'audition', instrument: id, note, track });
}
const grid = new PatternGrid(element('grid'), () => project, () => Number(element<HTMLInputElement>('octave').value), editedPattern,
  id => { if (id) { const definition = project.definitions.find(d => d.id === id); if (definition) selectDefinition(definition); } },
  (id, note, track) => { if (audioStatus.connected && !audioStatus.playing) void action(() => audition(id, note, track)); },
  () => { void action(togglePlay); });
element('undo').onclick = () => { grid.undo(); element('grid').focus(); };
element('redo').onclick = () => { grid.redo(); element('grid').focus(); };
function renderApplyStatus() {
  const effect = selected.kind === 'fx';
  const ready = (effect ? audioStatus.effects[selected.id] : audioStatus.instruments[selected.id]) !== undefined;
  element<HTMLButtonElement>('apply').disabled = !audioStatus.connected;
  element<HTMLButtonElement>('audition').disabled = !audioStatus.connected || (effect && !project.tracks.some(t => t.effect === selected.id && t.instrument));
  element('apply-status').textContent = !ready ? 'Not applied' : appliedSources.get(selected.id) === selected.source ? 'Applied' : 'Unapplied edits · playing last applied version';
}
element('connect').onclick = () => { void action(async () => {
  transportRevision++; starting = false;
  appliedSources.clear();
  if (audioStatus.connected) { await audioCommand({ op: 'disconnect' }); return; }
  const port = Number(element<HTMLInputElement>('engine-port').value);
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('Enter an engine port between 1 and 65535');
  await audioCommand({ op: 'connect', port, token: element<HTMLInputElement>('engine-token').value || null });
}); };
element('play').onclick = () => { void action(togglePlay); };
element('stop').onclick = () => { void action(stopPlayback); };
element('apply').onclick = () => { const definition = selected; void action(() => applyDefinition(definition)); };
element('audition').onclick = () => { const definition = selected; void action(async () => {
  let track = grid.activeTrack;
  let id = definition.id;
  if (definition.kind === 'fx') {
    await ensureSound(id);
    if (project.tracks[track].effect !== id || !project.tracks[track].instrument) track = project.tracks.findIndex(t => t.effect === id && t.instrument);
    if (track < 0) throw new Error('Assign this effect and an instrument to a track first');
    id = project.tracks[track].instrument!;
  }
  await audition(id, `C-${element<HTMLInputElement>('octave').value}`, track);
}); };
function timingInputs() {
  element<HTMLInputElement>('bpm').value = String(project.bpm);
  element<HTMLInputElement>('rows-per-beat').value = String(project.rows_per_beat);
}
for (const [id, field, min, max] of [['bpm', 'bpm', 20, 300], ['rows-per-beat', 'rows_per_beat', 1, 16]] as const) {
  element<HTMLInputElement>(id).onchange = event => {
    const input = event.target as HTMLInputElement;
    if (audioStatus.playing) { timingInputs(); return; }
    project[field] = Math.min(max, Math.max(min, Math.trunc(Number(input.value) || min)));
    timingInputs(); save();
  };
}
element<HTMLInputElement>('octave').onchange = event => {
  const input = event.target as HTMLInputElement;
  input.value = String(Math.min(8, Math.max(0, Math.trunc(Number(input.value) || 0))));
};
element<HTMLInputElement>('definition-name').oninput = event => { selected.name = (event.target as HTMLInputElement).value; renderDefinitions(); grid.render(); mixerView(); save(); };
function addDefinition(kind: 'instr' | 'fx') {
  const id = `${kind}_${crypto.randomUUID().replaceAll('-', '_')}`;
  const name = `${kind === 'instr' ? 'voice' : 'effect'}${project.definitions.length + 1}`;
  const definition: Definition = { id, kind, name, source: kind === 'instr' ? `/// Describe your instrument here.\ninstr ${name}(freq: hz) {\n  osc(sine, freq) * env[0.15 | 200ms 0]\n}\n` : `/// Describe your effect here.\nfx ${name} {\n  out = it.lpf(cutoff: 2400hz)\n}\n` };
  project.definitions.push(definition); selectDefinition(definition); editor.focus(); grid.render(); mixerView(); save();
}
element('new-instrument').onclick = () => addDefinition('instr');
element('new-effect').onclick = () => addDefinition('fx');
element('expand').onclick = () => {
  const expanded = element('app').classList.toggle('editor-expanded');
  element('expand').textContent = expanded ? 'Show pattern' : 'Expand editor'; editor.requestMeasure();
};
document.addEventListener('keydown', event => {
  if (event.key === 'F6') { event.preventDefault(); if (editor.hasFocus) element('grid').focus(); else editor.focus(); }
});
async function loadProject(raw: string) {
  const loaded = readProject(raw);
  transportRevision++; starting = false;
  if (audioStatus.connected) await audioCommand({ op: 'disconnect' });
  appliedSources.clear();
  editor.destroy(); states.clear();
  project = loaded; selected = project.definitions[0];
  const definition = selected;
  editor = new EditorView({ state: editorState(`file:///niminal-tracker/${definition.id}.nml`, definition.source, source => { definition.source = source; save(); renderApplyStatus(); }), parent: element('code') });
  selectDefinition(selected); grid.resetHistory(); grid.render(); mixerView(); timingInputs(); save();
}
element('export').onclick = async () => {
  const source = JSON.stringify(project, null, 2);
  try {
    if (isTauri()) {
      if (await invoke<boolean>('save_project', { source })) element('save-status').textContent = 'Project file saved';
    } else {
      const url = URL.createObjectURL(new Blob([source], { type: 'application/json' }));
      const link = document.createElement('a'); link.href = url; link.download = 'untitled.nimtracker.json'; link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    }
  } catch (error) { status(`Could not save project: ${error}`); }
};
element('import').onclick = async () => {
  if (isTauri()) {
    try { const raw = await invoke<string | null>('open_project'); if (raw !== null) await loadProject(raw); }
    catch (error) { status(`Could not open project: ${error}`); }
    return;
  }
  const input = document.createElement('input'); input.type = 'file'; input.accept = '.json';
  input.onchange = async () => {
    const file = input.files?.[0]; if (!file) return;
    try { await loadProject(await file.text()); }
    catch (error) { status(`Could not open project: ${error}`); }
  };
  input.click();
};
selectDefinition(selected); grid.render(); mixerView(); timingInputs();
const audioTimer = observeAudio(state => {
  element('connect').textContent = state.connected ? 'Disconnect' : 'Connect';
  element<HTMLButtonElement>('play').disabled = !state.connected || state.playing;
  element<HTMLButtonElement>('stop').disabled = !state.connected;
  element<HTMLInputElement>('bpm').disabled = state.playing;
  element<HTMLInputElement>('rows-per-beat').disabled = state.playing;
  element('audio-status').textContent = audioError ?? state.error ?? (state.connected ? `${state.playing ? 'Playing' : 'Connected'} · ${state.sample_rate / 1000} kHz${state.skipped_rows ? ` · ${state.skipped_rows} skipped rows` : ''}${state.late_events ? ` · ${state.late_events} late events` : ''}` : 'Start niminal server, then connect');
  grid.paint(state.row); renderApplyStatus();
});
void connectLanguageServer(status);
window.addEventListener('beforeunload', () => { clearInterval(audioTimer); editor.destroy(); });
