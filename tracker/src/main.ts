import { invoke, isTauri } from '@tauri-apps/api/core';
import { EditorView } from '@codemirror/view';
import { editorState, connectLanguageServer } from './editor';
import { newProject, emptyFxLane, readProject, storageKey, resizePattern, removeTrack, restoreTrack, type Definition, type Song, type DeletedTrack } from './project';
import { Arrangement } from './arrangement';
import { PatternGrid } from './grid';
import { renderMixer } from './mixer';
import { audioCommand, audioStatus, observeAudio } from './audio';
import './style.css';

const app = document.querySelector<HTMLDivElement>('#app')!;
app.innerHTML = `
  <header><div class="brand">niminal <span>tracker</span></div><span class="badge">ALPHA</span><div class="project-actions"><button id="toggle-mixer" aria-expanded="false">Track effects</button><button id="toggle-code" aria-expanded="false">Code editor</button><button id="focus-pattern">Focus pattern</button><button id="import">Open project</button><button id="export">Save project</button></div></header>
  <main>
    <aside id="sequencer"><div class="sequence-heading"><span class="section-title">SEQ</span><button id="sequence-names" aria-label="Toggle pattern names" title="Show / hide pattern names" aria-expanded="true">◂</button></div><div id="sequence"></div><div class="sequence-actions"><button id="new-pattern" data-structure title="New blank pattern" aria-label="New blank pattern">+</button><button id="variation" data-structure title="Duplicate as variation" aria-label="Duplicate as variation">⧉</button><button id="reuse" data-structure title="Reuse pattern" aria-label="Reuse pattern">↳</button><button id="move-up" data-structure title="Move sequence step up" aria-label="Move sequence step up">↑</button><button id="move-down" data-structure title="Move sequence step down" aria-label="Move sequence step down">↓</button><button id="remove-step" data-structure title="Remove sequence step" aria-label="Remove sequence step">−</button></div><details id="sound-library"><summary>SOUND LIBRARY</summary><div id="definitions"></div><div class="add-buttons"><button id="new-instrument">+ Instrument</button><button id="new-effect">+ Effect</button></div><p class="aside-help">Select a sound to edit its code.<br>Project copies are saved locally.</p></details></aside>
    <section class="workspace">
      <div class="engine-toolbar"><label>Engine port <input id="engine-port" type="number" min="1" max="65535" value="7401"></label><input id="engine-token" type="password" placeholder="Token (optional)" aria-label="Engine token"><button id="connect">Connect</button><span id="audio-status">Start niminal server, then connect</span></div>
      <div class="transport-toolbar"><button id="play">Loop pattern</button><button id="play-song" title="Play from the selected sequence step to the end">Play song</button><button id="stop">Stop</button><label><input id="follow" type="checkbox" checked> Follow playback</label><span id="play-position" class="muted"></span><label>BPM <input id="bpm" type="number" min="20" max="300" step="1"></label><label>Rows/beat <input id="rows-per-beat" type="number" min="1" max="16"></label></div>
      <div class="pattern-toolbar"><div><strong id="pattern-number">Pattern 00</strong><select id="pattern-picker" aria-label="Pattern to edit"></select><input id="pattern-name" aria-label="Pattern name"><span id="pattern-uses" class="muted"></span></div><label>Rows <input id="pattern-rows" type="number" min="1" max="256" data-structure></label><label>Octave <input id="octave" type="number" min="0" max="8" value="4"></label><label>Step <input id="entry-step" aria-label="Note entry step" title="Rows to advance after entering a note; 0 stays on this row" type="number" min="0" max="256" value="1"></label><button id="undo">Undo notes</button><button id="redo">Redo</button><button id="add-track" data-structure title="Add track" aria-label="Add track">+ Track</button><button id="delete-track" data-structure title="Delete selected track" aria-label="Delete selected track">− Track</button><button id="restore-track" data-structure title="Restore last deleted track" aria-label="Restore last deleted track">↶</button></div>
      <div id="grid" tabindex="0" role="grid" aria-label="Pattern editor"></div>
      <div class="grid-help">Z S X D C V G B H N J M: notes · Arrows: move · Delete: clear · Shift+arrows: select · Cmd/Ctrl+C/V: copy/paste · Cmd/Ctrl+Z: undo · Space: play/stop · F6: switch editor</div>
      <section id="mixer-panel" hidden><div class="panel-heading">TRACK DEVICES<button id="close-mixer">Minimise</button></div><div id="mixer" aria-label="Track mixer"></div></section>
      <section class="instrument-editor" id="code-panel" hidden><div class="editor-toolbar"><div><span id="definition-kind" class="badge"></span><input id="definition-name" aria-label="Definition name"></div><div><span id="apply-status" class="muted"></span><button id="apply">Apply</button><button id="audition">Audition</button><button id="expand">Expand editor</button><button id="close-code">Minimise</button></div></div><div id="code"></div><div class="editor-help">Hover for documentation or errors · Ctrl+Space: completion · Shift+Alt+F: format</div></section>
    </section>
  </main><footer><span id="status">Starting editor…</span><span id="save-status">Local autosave</span></footer>`;

const element = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
function setText(id: string, value: string) {
  const node = element(id);
  if (node.textContent !== value) node.textContent = value;
}
const status = (text: string) => { setText('status', text); };
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
let deviceTrack = 0;
let deletedTrack: DeletedTrack | null = null;
let wasPlaying = false;
let connectionPending: 'connect' | 'disconnect' | null = null;

function save() {
  try { localStorage.setItem(storageKey, JSON.stringify(project)); element('save-status').textContent = 'Saved locally'; }
  catch { element('save-status').textContent = 'Autosave failed · use Save project'; }
}

function selectDefinition(definition: Definition, reveal = true) {
  if (reveal) showPanel('code', true);
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
function song(): Song { return { bpm: project.bpm, rows_per_beat: project.rows_per_beat, tracks: project.tracks, patterns: project.patterns, sequence: project.sequence }; }
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
  grid.render(); mixerView();
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
    for (const slot of track.effects) await ensureSound(slot.definition);
  }
}
function mixerView() {
  renderMixer(element('mixer'), project, deviceTrack, track => grid.selectTrack(track), () => { editedPattern(); grid.render(); }, id => {
    const definition = project.definitions.find(d => d.id === id);
    if (definition) selectDefinition(definition);
  }, audioStatus.effect_parameters, (target, value) => grid.writeParameter(deviceTrack, target, value), audioStatus.playing, () => {
    const definition = addDefinition('fx');
    project.tracks[deviceTrack].effects.push({ definition: definition.id, values: {}, modulations: {} });
    editedPattern(); mixerView();
  });
}
async function stopPlayback() {
  transportRevision++; starting = false;
  await audioCommand({ op: 'stop' });
}
async function togglePlay(mode: 'pattern' | 'song' = 'pattern') {
  if (audioStatus.playing || starting) { await stopPlayback(); return; }
  const revision = ++transportRevision;
  starting = true;
  try {
    for (const track of project.tracks) {
      if (track.instrument) await ensureSound(track.instrument);
      for (const slot of track.effects) await ensureSound(slot.definition);
      if (revision !== transportRevision) return;
    }
    await audioCommand({ op: 'play', song: song(), mode, pattern_id: arrangement.pattern.id, start_position: arrangement.position });
  } finally { if (revision === transportRevision) starting = false; }
}
function editedPattern() {
  save();
  clearTimeout(updateTimer);
  updateTimer = window.setTimeout(() => {
    if (audioStatus.connected) void action(async () => {
      await ensureAssignments();
      await audioCommand({ op: 'update', song: song() });
    });
  }, 50);
}
async function audition(id: string, note: string, track: number) {
  await ensureSound(id); await ensureAssignments();
  await audioCommand({ op: 'update', song: song() });
  await audioCommand({ op: 'audition', instrument: id, note, track });
}
const arrangement = new Arrangement(element('sequence'), () => project, manual => {
  if (manual) element<HTMLInputElement>('follow').checked = false;
  grid.render(); deviceTrack = grid.activeTrack; mixerView(); patternControls();
}, editedPattern);
const grid = new PatternGrid(element('grid'), () => project, () => arrangement.pattern, () => Number(element<HTMLInputElement>('octave').value), () => { editedPattern(); mixerView(); },
  id => { if (id) { const definition = project.definitions.find(d => d.id === id); if (definition) selectDefinition(definition); } },
  (id, note, track) => { if (audioStatus.connected && !audioStatus.playing) void action(() => audition(id, note, track)); },
  () => { void action(() => togglePlay()); },
  track => { if (deviceTrack !== track) { deviceTrack = track; mixerView(); } },
  (track, device, sameTrack) => {
    if (device === 'effects') { showPanel('mixer', !sameTrack || element('mixer-panel').hidden); mixerView(); }
    else {
      const definition = project.definitions.find(d => d.id === project.tracks[track].instrument);
      if (definition) {
        if (sameTrack && selected.id === definition.id && !element('code-panel').hidden) { showPanel('code', false); element('grid').focus(); }
        else { selectDefinition(definition); editor.focus(); }
      }
      else { addDefinition('instr'); project.tracks[track].instrument = selected.id; grid.render(); editedPattern(); }
    }
  }, () => Number(element<HTMLInputElement>('entry-step').value), () => audioStatus.effect_parameters, status);
element('undo').onclick = () => { grid.undo(); element('grid').focus(); };
element('redo').onclick = () => { grid.redo(); element('grid').focus(); };
function renderApplyStatus() {
  const effect = selected.kind === 'fx';
  const ready = (effect ? audioStatus.effects[selected.id] : audioStatus.instruments[selected.id]) !== undefined;
  element<HTMLButtonElement>('apply').disabled = !audioStatus.connected;
  element<HTMLButtonElement>('audition').disabled = !audioStatus.connected || (effect && !project.tracks.some(t => t.effects.some(slot => slot.definition === selected.id) && t.instrument));
  setText('apply-status', !ready ? 'Not applied' : appliedSources.get(selected.id) === selected.source ? 'Applied' : 'Unapplied edits · playing last applied version');
}
function renderConnectButton() {
  setText('connect', connectionPending === 'connect' ? 'Connecting…' : connectionPending === 'disconnect' ? 'Disconnecting…' : audioStatus.connected ? 'Disconnect' : 'Connect');
  element<HTMLButtonElement>('connect').disabled = connectionPending !== null;
  element<HTMLInputElement>('engine-port').disabled = connectionPending !== null || audioStatus.connected;
  element<HTMLInputElement>('engine-token').disabled = connectionPending !== null || audioStatus.connected;
}
element('connect').onclick = () => {
  if (connectionPending) return;
  connectionPending = audioStatus.connected ? 'disconnect' : 'connect';
  renderConnectButton();
  void action(async () => {
    transportRevision++; starting = false;
    appliedSources.clear();
    if (audioStatus.connected) { await audioCommand({ op: 'disconnect' }); return; }
    const port = Number(element<HTMLInputElement>('engine-port').value);
    if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('Enter an engine port between 1 and 65535');
    await audioCommand({ op: 'connect', port, token: element<HTMLInputElement>('engine-token').value || null });
  }).finally(() => { connectionPending = null; renderConnectButton(); });
};
element('play').onclick = () => { void action(() => togglePlay('pattern')); };
element('play-song').onclick = () => { void action(() => togglePlay('song')); };
element('stop').onclick = () => { void action(stopPlayback); };
element('apply').onclick = () => { const definition = selected; void action(() => applyDefinition(definition)); };
element('audition').onclick = () => { const definition = selected; void action(async () => {
  let track = grid.activeTrack;
  let id = definition.id;
  if (definition.kind === 'fx') {
    await ensureSound(id);
    if (!project.tracks[track].effects.some(slot => slot.definition === id) || !project.tracks[track].instrument) track = project.tracks.findIndex(t => t.effects.some(slot => slot.definition === id) && t.instrument);
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
element<HTMLInputElement>('entry-step').onchange = event => {
  const input = event.target as HTMLInputElement; input.value = String(Math.max(0, Math.min(256, Math.trunc(Number(input.value) || 0))));
};
element('sequence-names').onclick = () => {
  const collapsed = element('sequencer').classList.toggle('names-hidden');
  element('sequence-names').setAttribute('aria-expanded', String(!collapsed));
  setText('sequence-names', collapsed ? '▸' : '◂');
};
element<HTMLInputElement>('octave').onchange = event => {
  const input = event.target as HTMLInputElement;
  input.value = String(Math.min(8, Math.max(0, Math.trunc(Number(input.value) || 0))));
};
element<HTMLInputElement>('definition-name').oninput = event => { selected.name = (event.target as HTMLInputElement).value; renderDefinitions(); grid.render(); mixerView(); save(); };
function addDefinition(kind: 'instr' | 'fx') {
  const id = `${kind}_${crypto.randomUUID().replaceAll('-', '_')}`;
  const name = `${kind === 'instr' ? 'voice' : 'effect'}${project.definitions.length + 1}`;
  const definition: Definition = { id, kind, name, source: kind === 'instr' ? `/// Describe your instrument here.\ninstr ${name}(freq: hz) {\n  osc(sine, freq) * env[0.15 | 200ms 0]\n}\n` : `/// Describe your effect here.\nfx ${name}(cutoff: hz = 2400hz) {\n  out = it.lpf(cutoff: cutoff)\n}\n` };
  project.definitions.push(definition); selectDefinition(definition); editor.focus(); grid.render(); mixerView(); save();
  return definition;
}
element('new-instrument').onclick = () => addDefinition('instr');
element('new-effect').onclick = () => addDefinition('fx');
element('expand').onclick = () => {
  showPanel('code', true);
  const expanded = element('app').classList.toggle('editor-expanded');
  element('expand').textContent = expanded ? 'Show pattern' : 'Expand editor'; editor.requestMeasure();
};
document.addEventListener('keydown', event => {
  if (event.key === 'F6') { event.preventDefault(); if (editor.hasFocus) element('grid').focus(); else { showPanel('code', true); editor.focus(); } }
});
async function loadProject(raw: string) {
  const loaded = readProject(raw);
  transportRevision++; starting = false;
  if (audioStatus.connected) await audioCommand({ op: 'disconnect' });
  appliedSources.clear();
  editor.destroy(); states.clear();
  project = loaded; deletedTrack = null; deviceTrack = 0; selected = project.definitions[0];
  const definition = selected;
  editor = new EditorView({ state: editorState(`file:///niminal-tracker/${definition.id}.nml`, definition.source, source => { definition.source = source; save(); renderApplyStatus(); }), parent: element('code') });
  selectDefinition(selected, false); arrangement.reset(); grid.resetHistory(); grid.selectTrack(0); mixerView(); patternControls(); timingInputs(); save();
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
function showPanel(panel: 'code' | 'mixer', visible: boolean) {
  element(`${panel}-panel`).hidden = !visible;
  element(`toggle-${panel}`).setAttribute('aria-expanded', String(visible));
  if (panel === 'code') {
    if (!visible) { element('app').classList.remove('editor-expanded'); setText('expand', 'Expand editor'); }
    if (editor) editor.requestMeasure();
  }
}
for (const panel of ['code', 'mixer'] as const) {
  element(`toggle-${panel}`).onclick = () => showPanel(panel, element(`${panel}-panel`).hidden);
  element(`close-${panel}`).onclick = () => { showPanel(panel, false); element('grid').focus(); };
}
element('focus-pattern').onclick = () => { showPanel('code', false); showPanel('mixer', false); element<HTMLDetailsElement>('sound-library').open = false; element('grid').focus(); };
function patternControls() {
  const current = arrangement.pattern;
  setText('pattern-number', `Pattern ${arrangement.number}`);
  element<HTMLInputElement>('pattern-name').value = current.name;
  element<HTMLInputElement>('pattern-rows').value = String(current.rows);
  const uses = project.sequence.filter(id => id === current.id).length;
  setText('pattern-uses', uses > 1 ? `Shared by ${uses} sequence steps` : uses === 1 ? 'One sequence step' : 'Not sequenced');
  const picker = element<HTMLSelectElement>('pattern-picker'); picker.replaceChildren();
  for (const [index, pattern] of project.patterns.entries()) picker.add(new Option(`${String(index).padStart(2, '0')} · ${pattern.name}`, pattern.id));
  picker.value = current.id;
}
element<HTMLSelectElement>('pattern-picker').onchange = event => arrangement.choose((event.target as HTMLSelectElement).value);
element<HTMLInputElement>('pattern-name').oninput = event => {
  arrangement.pattern.name = (event.target as HTMLInputElement).value;
  arrangement.render(); save();
};
element<HTMLInputElement>('pattern-name').onchange = patternControls;
element<HTMLInputElement>('pattern-rows').onchange = event => {
  if (audioStatus.playing) { patternControls(); return; }
  resizePattern(arrangement.pattern, Number((event.target as HTMLInputElement).value));
  grid.render(); patternControls(); editedPattern();
};
for (const [id, operation] of [['new-pattern', 'new'], ['variation', 'variation'], ['reuse', 'reuse']] as const) {
  element(id).onclick = () => { if (!audioStatus.playing) arrangement.insert(operation); };
}
element('move-up').onclick = () => { if (!audioStatus.playing) arrangement.move(-1); };
element('move-down').onclick = () => { if (!audioStatus.playing) arrangement.move(1); };
element('add-track').onclick = () => {
  if (audioStatus.playing || project.tracks.length >= 16) return;
  project.tracks.push({ name: `Track ${project.tracks.length + 1}`, instrument: null, effects: [], pan: 0, muted: false, volume_db: 0 });
  for (const pattern of project.patterns) { pattern.notes.push(Array(pattern.notes[0].length).fill(null)); pattern.fx.push([emptyFxLane(pattern.notes[0].length)]); }
  grid.selectTrack(project.tracks.length - 1); editedPattern();
};
element('delete-track').onclick = () => { void action(async () => {
  if (audioStatus.playing || project.tracks.length <= 1) return;
  if (audioStatus.connected) await stopPlayback();
  const index = grid.activeTrack;
  deletedTrack = removeTrack(project, index);
  grid.resetHistory(); grid.selectTrack(Math.min(index, project.tracks.length - 1)); mixerView(); editedPattern();
}); };
element('restore-track').onclick = () => {
  if (audioStatus.playing || !deletedTrack || project.tracks.length >= 16) return;
  const index = deletedTrack.index;
  if (!restoreTrack(project, deletedTrack)) return;
  deletedTrack = null; grid.resetHistory(); grid.selectTrack(index); mixerView(); editedPattern();
};
element('remove-step').onclick = () => { if (!audioStatus.playing) arrangement.remove(); };
selectDefinition(selected, false); arrangement.reset(); grid.render(); mixerView(); patternControls(); timingInputs();
const audioTimer = observeAudio(state => {
  if (state.playing !== wasPlaying) { wasPlaying = state.playing; mixerView(); }
  // Replacing a button's text node during a mouse press can cancel its click
  // in WebKit. Only change labels when the displayed state actually changes.
  renderConnectButton();
  element<HTMLButtonElement>('play').disabled = !state.connected || state.playing;
  element<HTMLButtonElement>('stop').disabled = !state.connected;
  element<HTMLButtonElement>('play-song').disabled = !state.connected || state.playing;
  for (const input of document.querySelectorAll<HTMLInputElement | HTMLButtonElement>('[data-structure]')) {
    if (input.closest('#mixer')) continue;
    input.disabled = state.playing || input.id === 'add-track' && project.tracks.length >= 16 || input.id === 'delete-track' && project.tracks.length <= 1 || input.id === 'restore-track' && (!deletedTrack || project.tracks.length >= 16);
  }
  arrangement.paint(state.sequence_position, state.playing);
  if (state.playing && element<HTMLInputElement>('follow').checked && state.pattern_id && (arrangement.pattern.id !== state.pattern_id || state.sequence_position !== null && arrangement.position !== state.sequence_position)) {
    if (state.sequence_position !== null) arrangement.select(state.sequence_position, false);
    else { arrangement.choose(state.pattern_id); element<HTMLInputElement>('follow').checked = true; }
  }
  setText('play-position', state.playing && state.pattern_id ? `${state.sequence_position === null ? 'Loop' : `Step ${state.sequence_position + 1}`} · row ${state.row ?? '…'}` : '');
  element<HTMLInputElement>('bpm').disabled = state.playing;
  element<HTMLInputElement>('rows-per-beat').disabled = state.playing;
  setText('audio-status', audioError ?? state.error ?? (state.connected ? `${state.playing ? 'Playing' : 'Connected'} · ${state.sample_rate / 1000} kHz${state.skipped_rows ? ` · ${state.skipped_rows} skipped rows` : ''}${state.late_events ? ` · ${state.late_events} late events` : ''}` : 'Start niminal server, then connect'));
  grid.paint(state.pattern_id === arrangement.pattern.id ? state.row : null); renderApplyStatus();
});
void connectLanguageServer(status);
window.addEventListener('beforeunload', () => { clearInterval(audioTimer); editor.destroy(); });
