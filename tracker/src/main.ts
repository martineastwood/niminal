import { invoke, isTauri } from '@tauri-apps/api/core';
import { EditorView } from '@codemirror/view';
import { editorState, connectLanguageServer } from './editor';
import { newProject, emptyFxLane, blankColumn, type Device, readProject, storageKey, maxReturns, resizePattern, removeTrack, restoreTrack, type Definition, type Song, type DeletedTrack } from './project';
import { Arrangement } from './arrangement';
import { PatternGrid } from './grid';
import { EnvelopeEditor } from './envelope-editor';
import { renderMixer, renderInstrument } from './mixer';
import { addDevice, removeDevice, moveDevice, newEffect, duplicateEffect, effectUsers, removeEffect, stripOf } from './effects';
import { MixPanel } from './mix';
import { drawScope } from './scope';
import { addInstrument, duplicateInstrument, instrumentUsers, removeInstrument } from './instruments';
import { audioCommand, audioStatus, observeAudio } from './audio';
import './style.css';

const app = document.querySelector<HTMLDivElement>('#app')!;
app.innerHTML = `
  <div id="toolbar">
    <div class="group"><button id="play" title="Loop pattern (Space)">▶ Pattern</button><button id="play-song" title="Play from the selected sequence step to the end, or repeat the loop once it is reached">▶ Song</button><button id="stop" title="Stop">■</button><button id="record" aria-pressed="false" title="Record mode: typed notes and values are entered (Esc)">● Rec</button><label class="check"><input id="follow" type="checkbox" checked> Follow</label><span id="play-position" class="muted"></span></div>
    <div class="group"><label>BPM <input id="bpm" type="number" min="20" max="300" step="1"></label><label>LPB <input id="rows-per-beat" type="number" min="1" max="16" title="Rows per beat"></label><label>Oct <input id="octave" type="number" min="0" max="8" value="4"></label><label>Step <input id="entry-step" aria-label="Note entry step" title="Rows to advance after entering a note; 0 stays on this row" type="number" min="0" max="256" value="1"></label></div>
    <div class="group"><strong id="pattern-number">00</strong><select id="pattern-picker" aria-label="Pattern to edit"></select><input id="pattern-name" aria-label="Pattern name"><label>Rows <input id="pattern-rows" type="number" min="1" max="256" data-structure></label><span id="pattern-uses" class="muted"></span></div>
    <div class="group"><button id="undo" title="Undo (Cmd/Ctrl+Z)" aria-label="Undo">↶</button><button id="redo" title="Redo (Cmd/Ctrl+Shift+Z)" aria-label="Redo">↷</button><button id="add-track" data-structure title="Add track" aria-label="Add track">+ Track</button><button id="delete-track" data-structure title="Delete selected track" aria-label="Delete selected track">− Track</button><button id="restore-track" data-structure title="Restore last deleted track" aria-label="Restore last deleted track">↺</button></div>
    <div class="group push"><button id="open-mix" aria-pressed="false" title="Mixer: levels, pan, mute, solo and a scope for each channel">Mixer</button><button id="import">Open</button><button id="export">Save</button></div>
  </div>
  <div id="scopebar"><canvas id="master-scope" aria-label="Master scope"></canvas></div>
  <main>
    <aside id="sequencer">
      <div class="side-heading"><span class="section-title">SEQUENCE</span><span class="side-tools"><button id="new-pattern" data-structure title="New blank pattern" aria-label="New blank pattern">+</button><button id="variation" data-structure title="Duplicate as variation" aria-label="Duplicate as variation">⧉</button><button id="reuse" data-structure title="Reuse pattern" aria-label="Reuse pattern">↳</button><button id="move-up" data-structure title="Move sequence step up" aria-label="Move sequence step up">↑</button><button id="move-down" data-structure title="Move sequence step down" aria-label="Move sequence step down">↓</button><button id="remove-step" data-structure title="Remove sequence step" aria-label="Remove sequence step">−</button><button id="loop" title="Loop the selected step; shift-click another step to loop a range. Works while playing." aria-label="Loop sequence step" aria-pressed="false">⟲</button><button id="sequence-names" aria-label="Toggle pattern names" title="Show / hide pattern names" aria-expanded="true">◂</button></span></div>
      <div id="sequence"></div>
      <div id="sound-library" class="library"><div class="side-heading"><span class="section-title">INSTRUMENTS</span><span class="side-tools"><button id="new-instrument" title="New synth" aria-label="New instrument">+</button><button id="import-sample" title="New sampler from WAV files" aria-label="Import sample">♪</button><button id="duplicate-instrument" title="Duplicate instrument" aria-label="Duplicate instrument">⧉</button><button id="delete-instrument" title="Delete instrument" aria-label="Delete instrument">−</button></span></div><div id="definitions"></div></div>
      <div id="effect-library" class="library"><div class="side-heading"><span class="section-title">EFFECTS</span><span class="side-tools"><button id="new-effect" title="New effect" aria-label="New effect">+</button><button id="duplicate-effect" title="Duplicate effect" aria-label="Duplicate effect">⧉</button><button id="delete-effect" title="Delete effect" aria-label="Delete effect">−</button></span></div><div id="effects"></div></div>
    </aside>
    <section class="workspace">
      <div id="grid-wrap"><div id="grid" tabindex="0" role="grid" aria-label="Pattern editor"></div></div>
      <div id="bottom-dock" hidden>
        <div class="dock-resize" id="dock-resize"></div>
        <div class="dock-bar"><button class="tab" data-tab="code">Code</button><button class="tab" data-tab="automation">Automation</button><button class="tab" data-tab="mix">Mixer</button><span id="code-tools"><span id="definition-kind" class="badge"></span><input id="definition-name" aria-label="Definition name"><button id="apply">Apply</button><span id="apply-status" class="muted"></span></span><span class="spacer"></span><button id="expand" title="Expand editor" aria-label="Expand editor">⤢</button><button id="close-dock" title="Close" aria-label="Close panel">×</button></div>
        <section id="automation-panel" hidden><div id="automation"></div></section>
        <section id="mix-panel" hidden><div id="mix"></div></section>
        <section class="instrument-editor" id="code-panel" hidden><div class="code-workspace"><div id="mixer" aria-label="Track FX controls" hidden></div><div id="code"></div></div></section>
      </div>
    </section>
  </main>
  <footer><span id="status">Starting editor…</span><span class="spacer"></span><span id="audio-status"></span><label>Port <input id="engine-port" type="number" min="1" max="65535" value="7401"></label><input id="engine-token" type="password" placeholder="Token" aria-label="Engine token"><button id="connect">Connect</button><span id="save-status">Local autosave</span></footer>`;

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
/** Where the engine finds this project's samples; resolved before the first apply. */
let projectDir: string | null = null;
let projectDirReady: Promise<void> = Promise.resolve();
/** Documents live in the project folder, so the language server finds `sample(\"samples/...\")` beside them. */
function documentUri(id: string) {
  return projectDir ? `file://${projectDir.split('/').map(encodeURIComponent).join('/')}/${id}.nml` : `file:///niminal-tracker/${id}.nml`;
}
function documentState(definition: Definition) {
  return editorState(documentUri(definition.id), definition.source, source => { definition.source = source; save(); renderApplyStatus(); });
}
function refreshProjectDir() {
  projectDirReady = isTauri() ? invoke<string>('project_dir', { id: project.id }).then(dir => {
    projectDir = dir;
    // Documents opened before the folder was known have the wrong address; reopen them.
    states.clear(); if (editor) editor.setState(documentState(selected));
  }, error => status(`Project folder unavailable: ${error}`)) : Promise.resolve();
}
/** The dock's code tab shows the selected track's effect chain rather than an instrument. */
let fxMode = false;
/** An effect opened from the library to edit its code, apart from any track's chain. */
let libraryFx: string | null = null;
const chosenDevice = new Map<number, string>();
let deletedTrack: DeletedTrack | null = null;
let wasPlaying = false;
let envelopeEditor: EnvelopeEditor | undefined;
let connectionPending: 'connect' | 'disconnect' | null = null;

function save() {
  try { localStorage.setItem(storageKey, JSON.stringify(project)); element('save-status').textContent = 'Saved locally'; }
  catch { element('save-status').textContent = 'Autosave failed · use Save project'; }
}

function selectDefinition(definition: Definition, reveal = true, library = false) {
  if (editor) states.set(selected.id, editor.state);
  selected = definition;
  if (reveal) showPanel('code', true);
  const state = states.get(definition.id) ?? documentState(definition);
  if (editor) editor.setState(state);
  else editor = new EditorView({ state, parent: element('code') });
  element<HTMLInputElement>('definition-name').value = definition.name;
  element('definition-kind').textContent = definition.kind === 'instr' ? 'INSTRUMENT' : 'EFFECT';
  fxMode = definition.kind === 'fx'; libraryFx = library ? definition.id : null;
  element('mixer').hidden = false; element('code').hidden = false;
  renderDefinitions(); renderApplyStatus(); mixerView();
}
function renderDefinitions() {
  const list = element('definitions');
  list.replaceChildren();
  for (const [index, definition] of project.definitions.filter(d => d.kind === 'instr').entries()) {
    const button = document.createElement('button');
    button.className = `definition ${definition.id === selected.id ? 'selected' : ''}`;
    const users = instrumentUsers(project, definition.id).length;
    button.textContent = `${String(index).padStart(2, '0')}  ${definition.name}`;
    if (users) { const count = document.createElement('small'); count.textContent = `×${users}`; button.append(count); }
    button.onclick = () => { selectDefinition(definition); editor.focus(); };
    list.append(button);
  }
  const effects = element('effects');
  effects.replaceChildren();
  for (const definition of project.definitions.filter(d => d.kind === 'fx')) {
    const button = document.createElement('button');
    button.className = `definition ${definition.id === selected.id ? 'selected' : ''}`;
    const users = effectUsers(project, definition.id).length;
    button.textContent = definition.name;
    if (users) { const count = document.createElement('small'); count.textContent = `×${users}`; button.append(count); }
    button.onclick = () => { selectDefinition(definition, true, true); editor.focus(); };
    effects.append(button);
  }
}
/** Effect and instrument parameters share one table: definition ids are unique across both. */
function allParameters() { return { ...audioStatus.instrument_parameters, ...audioStatus.effect_parameters }; }
function song(): Song {
  const instrument_values = Object.fromEntries(project.definitions.filter(d => d.kind === 'instr' && d.values).map(d => [d.id, d.values!]));
  return { bpm: project.bpm, rows_per_beat: project.rows_per_beat, tracks: project.tracks, returns: project.returns, master: project.master, patterns: project.patterns, sequence: project.sequence, loop_range: project.loop_range, instrument_values };
}
async function action(work: () => Promise<unknown>) {
  audioError = null;
  try { await work(); }
  catch (error) { audioError = String(error); element('audio-status').textContent = audioError; }
  finally { renderApplyStatus(); }
}
async function applyDefinition(definition: Definition) {
  const source = definition.source;
  await projectDirReady;
  await audioCommand({ op: 'apply', id: definition.id, source, kind: definition.kind, dir: projectDir });
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
/** Every instrument the song can play: each track's own, and any chosen in an instrument column. */
function usedInstruments() {
  const ids = new Set<string>();
  for (const track of project.tracks) if (track.instrument) ids.add(track.instrument);
  for (const pattern of project.patterns) for (const column of pattern.notes.flat()) for (const id of column.inst) if (id) ids.add(id);
  return ids;
}
async function ensureAssignments() {
  for (const id of usedInstruments()) await ensureSound(id);
  for (const track of [...project.tracks, ...project.returns, project.master]) {
    for (const device of track.fx) await ensureSound(device.definition);
  }
}
function currentDevice(): Device | undefined {
  const chain = stripOf(project, deviceTrack).fx;
  return chain.find(d => d.id === chosenDevice.get(deviceTrack)) ?? chain[0];
}
/** The effect definition the code editor is showing, or null when the track has no effects. */
function shownEffect() { return fxMode && !element('code').hidden ? selected.id : null; }
function mixerView() {
  refreshMix();
  if (fxMode && libraryFx !== null) { element('mixer').hidden = true; return; }
  element('mixer').hidden = false;
  if (fxMode && !element('code-panel').hidden && shownEffect() !== (currentDevice()?.definition ?? null)) { showFx(); return; }
  if (deviceTrack >= 0) envelopeEditor?.select(deviceTrack);
  if (!fxMode) { renderInstrument(element('mixer'), project, selected, allParameters(), editedPattern, (target, value) => { if (deviceTrack >= 0) grid.writeParameter(deviceTrack, target, value); }, deviceTrack); return; }
  const device = currentDevice();
  renderMixer(element('mixer'), project, deviceTrack, () => { editedPattern(); grid.render(); }, allParameters(), (target, value) => { if (deviceTrack >= 0) grid.writeParameter(deviceTrack, target, value); }, {
    selected: device?.id ?? null,
    select: id => { chosenDevice.set(deviceTrack, id); showFx(); editor.focus(); },
    add: preset => {
      const added = addDevice(project, deviceTrack, preset); if (!added) return;
      chosenDevice.set(deviceTrack, added.device.id); showFx(); editor.focus(); editedPattern(); grid.render();
      if (audioStatus.connected) void action(() => ensureSound(added.definition.id));
    },
    remove: id => {
      removeDevice(project, deviceTrack, id);
      chosenDevice.delete(deviceTrack); showFx(); editedPattern(); grid.render();
    },
    move: (id, delta) => { if (moveDevice(project, deviceTrack, id, delta)) { editedPattern(); grid.render(); mixerView(); } },
    changed: () => { editedPattern(); grid.render(); mixerView(); },
  });
}
/** Show the selected track's chain: the controls beside the code of its selected effect. */
function showFx() {
  libraryFx = null;
  const device = currentDevice();
  const definition = device && project.definitions.find(d => d.id === device.definition);
  if (definition) selectDefinition(definition);
  else { fxMode = true; showPanel('code', true); element('mixer').hidden = false; element('code').hidden = true; element('code-tools').hidden = true; }
  mixerView();
}
function openTrackFx() { showFx(); if (!element('code').hidden) editor.focus(); }
async function stopPlayback() {
  transportRevision++; starting = false;
  await audioCommand({ op: 'stop' });
}
async function togglePlay(mode: 'pattern' | 'song' = 'pattern') {
  if (audioStatus.playing || starting) { await stopPlayback(); return; }
  const revision = ++transportRevision;
  starting = true;
  try {
    for (const id of usedInstruments()) await ensureSound(id);
    for (const track of [...project.tracks, ...project.returns, project.master]) {
      for (const device of track.fx) await ensureSound(device.definition);
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
  track => { if (deviceTrack !== track) { const editingFx = fxMode && !element('code-panel').hidden; deviceTrack = track; if (editingFx) showFx(); else mixerView(); } },
  (track, device, sameTrack) => {
    if (device === 'effects') {
      if (sameTrack && fxMode && !element('code-panel').hidden) { showPanel('code', false); element('grid').focus(); }
      else openTrackFx();
    }
    else {
      const definition = project.definitions.find(d => d.id === project.tracks[track].instrument);
      if (definition) {
        if (sameTrack && selected.id === definition.id && !element('code-panel').hidden) { showPanel('code', false); element('grid').focus(); }
        else { selectDefinition(definition); editor.focus(); }
      }
      else { newInstrument(); project.tracks[track].instrument = selected.id; grid.render(); editedPattern(); }
    }
  }, () => Number(element<HTMLInputElement>('entry-step').value), () => allParameters(), status, (track, lane) => {
    grid.selectTrack(track); showPanel('automation', true); envelopeEditor?.select(track, lane);
  });
envelopeEditor = new EnvelopeEditor(element('automation'), () => project, () => arrangement.pattern, () => allParameters(),
  () => { editedPattern(); grid.render(); });
/** Opens the effect chain of the master (-1) or a return, in the code tab beside its controls. */
function openStripFx(strip: number) { deviceTrack = strip; fxMode = true; showFx(); if (!element('code').hidden) editor.focus(); }
const mix = new MixPanel(element('mix'), () => project, {
  selectTrack: track => grid.selectTrack(track),
  openFx: track => { if (track < 0) openStripFx(track); else { grid.selectTrack(track); openTrackFx(); } },
  edited: () => { editedPattern(); grid.render(); },
  addReturn: () => {
    if (project.returns.length >= maxReturns) return;
    project.returns.push({ name: `Return ${String.fromCharCode(65 + project.returns.length)}`, instrument: null, fx: [], pan: 0, muted: false, volume_db: 0 });
    editedPattern(); mixerView();
  },
  removeReturn: index => {
    project.returns.splice(index, 1);
    for (const track of [...project.tracks, ...project.returns]) {
      // A strip that fed the removed return goes back to the master; later returns shift down.
      if (track.output?.kind === 'return') track.output = track.output.index === index ? { kind: 'master' } : track.output.index > index ? { kind: 'return', index: track.output.index - 1 } : track.output;
      for (const device of track.fx) if (device.target !== undefined && device.target > index) device.target--;
    }
    if (deviceTrack < -1) { deviceTrack = grid.activeTrack; if (fxMode) showFx(); }
    editedPattern(); mixerView();
  },
});
function refreshMix() {
  if (element('mix-panel').hidden) return;
  mix.render(); mix.highlight(deviceTrack);
}
let scopes: Record<string, number[]> = {};
async function pollScopes() {
  if (isTauri() && audioStatus.connected) { try { scopes = await invoke<Record<string, number[]>>('audio_scopes'); } catch { scopes = {}; } }
  else scopes = {};
  drawScope(element<HTMLCanvasElement>('master-scope'), scopes.s_m);
  if (!element('mix-panel').hidden) mix.paint(scopes);
}
const scopeTimer = window.setInterval(() => { void pollScopes(); }, 50);
function newLibraryEffect(preset = 'blank') {
  const definition = newEffect(project, preset)!;
  selectDefinition(definition, true, true); editor.focus(); save();
}
element('new-effect').onclick = () => newLibraryEffect();
element('duplicate-effect').onclick = () => {
  if (selected.kind !== 'fx') { status('Select an effect to duplicate'); return; }
  const copy = duplicateEffect(project, selected); selectDefinition(copy, true, true); editor.focus(); save();
};
element('delete-effect').onclick = () => {
  if (selected.kind !== 'fx') { status('Select an effect to delete'); return; }
  const removed = selected, problem = removeEffect(project, removed.id);
  if (problem) { status(problem); return; }
  states.delete(removed.id);
  selectDefinition(project.definitions.find(d => d.kind === 'instr')!, false); save();
};
grid.modeChanged = on => { element('grid-wrap').classList.toggle('recording', on); element('record').setAttribute('aria-pressed', String(on)); };
element('record').onclick = () => { grid.setRecording(!grid.recording); element('grid').focus(); };
element('undo').onclick = () => { grid.undo(); element('grid').focus(); };
element('redo').onclick = () => { grid.redo(); element('grid').focus(); };
function renderApplyStatus() {
  const effect = selected.kind === 'fx';
  const ready = (effect ? audioStatus.effects[selected.id] : audioStatus.instruments[selected.id]) !== undefined;
  element<HTMLButtonElement>('apply').disabled = !audioStatus.connected;
  element('apply').classList.toggle('dirty', audioStatus.connected && (!ready || appliedSources.get(selected.id) !== selected.source));
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
element('loop').onclick = () => { arrangement.toggleLoop(); patternControls(); };
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
function newInstrument(sample?: string) {
  const definition = addInstrument(project, sample);
  selectDefinition(definition); editor.focus(); grid.render(); mixerView(); save();
  return definition;
}
element('new-instrument').onclick = () => newInstrument();
element('import-sample').onclick = async () => {
  if (!isTauri()) { status('Importing samples needs the desktop app'); return; }
  try {
    const files = await invoke<string[]>('import_samples', { id: project.id });
    for (const file of files) newInstrument(file);
    if (files.length) status(`Added ${files.length} sampler${files.length > 1 ? 's' : ''}`);
  } catch (error) { status(`Could not import samples: ${error}`); }
};
element('duplicate-instrument').onclick = () => {
  if (selected.kind !== 'instr') { status('Select an instrument to duplicate'); return; }
  const copy = duplicateInstrument(project, selected);
  selectDefinition(copy); editor.focus(); save();
};
element('delete-instrument').onclick = () => {
  if (selected.kind !== 'instr') { status('Select an instrument to delete'); return; }
  const removed = selected, problem = removeInstrument(project, removed.id);
  if (problem) { status(problem); return; }
  states.delete(removed.id);
  selectDefinition(project.definitions.find(d => d.kind === 'instr')!, false); grid.render(); save();
};
element('expand').onclick = () => {
  showPanel('code', true);
  const expanded = element('app').classList.toggle('editor-expanded');
  setText('expand', expanded ? '⤡' : '⤢'); editor.requestMeasure();
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
  envelopeEditor?.reset();
  project = loaded; deletedTrack = null; deviceTrack = 0; chosenDevice.clear(); fxMode = false; refreshProjectDir(); selected = project.definitions[0];
  const definition = selected;
  editor = new EditorView({ state: documentState(definition), parent: element('code') });
  selectDefinition(selected, false); arrangement.reset(); grid.resetHistory(); grid.selectTrack(0); mixerView(); patternControls(); timingInputs(); save();
}
element('export').onclick = async () => {
  const source = JSON.stringify(project, null, 2);
  try {
    if (isTauri()) {
      if (await invoke<boolean>('save_project', { source, id: project.id })) element('save-status').textContent = 'Project saved';
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
type Panel = 'code' | 'automation' | 'mix';
const panels: Panel[] = ['code', 'automation', 'mix'];
function showPanel(panel: Panel, visible: boolean) {
  for (const candidate of panels) {
    const section = element(`${candidate}-panel`);
    if (visible || candidate === panel) section.hidden = !visible || candidate !== panel;
    document.querySelector(`[data-tab=${candidate}]`)!.setAttribute('aria-selected', String(!section.hidden));
  }
  element('code-tools').hidden = element('code-panel').hidden;
  element('bottom-dock').hidden = panels.every(candidate => element(`${candidate}-panel`).hidden);
  element('open-mix').setAttribute('aria-pressed', String(!element('mix-panel').hidden));
  if (!element('mix-panel').hidden) refreshMix();
  if (element('code-panel').hidden) {
    element('app').classList.remove('editor-expanded'); setText('expand', '⤢');
  }
  if (editor) editor.requestMeasure();
}
for (const panel of panels) {
  document.querySelector<HTMLElement>(`[data-tab=${panel}]`)!.onclick = () => {
    showPanel(panel, true);
    if (panel === 'automation') envelopeEditor?.select(Math.max(0, deviceTrack));
    if (panel === 'code' && fxMode) showFx();
  };
}
element('open-mix').onclick = () => showPanel('mix', element('mix-panel').hidden);
element('close-dock').onclick = () => { showPanel('code', false); element('grid').focus(); };
element('dock-resize').onpointerdown = event => {
  const dock = element('bottom-dock'), startY = event.clientY, startHeight = dock.offsetHeight;
  const target = event.currentTarget as HTMLElement; target.setPointerCapture(event.pointerId);
  target.onpointermove = move => { dock.style.height = `${Math.max(120, startHeight + startY - move.clientY)}px`; editor.requestMeasure(); };
  target.onpointerup = () => { target.onpointermove = null; target.onpointerup = null; };
};
function patternControls() {
  const current = arrangement.pattern;
  element('loop').setAttribute('aria-pressed', String(!!project.loop_range));
  setText('pattern-number', arrangement.number);
  element<HTMLInputElement>('pattern-name').value = current.name;
  element<HTMLInputElement>('pattern-rows').value = String(current.rows);
  const uses = project.sequence.filter(id => id === current.id).length;
  setText('pattern-uses', uses > 1 ? `×${uses}` : uses === 0 ? 'unused' : '');
  const picker = element<HTMLSelectElement>('pattern-picker'); picker.replaceChildren();
  for (const [index, pattern] of project.patterns.entries()) picker.add(new Option(String(index).padStart(2, '0'), pattern.id));
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
  project.tracks.push({ name: `Track ${project.tracks.length + 1}`, instrument: null, fx: [], pan: 0, muted: false, volume_db: 0 });
  for (const pattern of project.patterns) { const rows = pattern.notes[0][0].notes.length; pattern.notes.push([blankColumn(rows)]); pattern.fx.push([emptyFxLane(rows)]); }
  grid.selectTrack(project.tracks.length - 1); editedPattern();
};
element('delete-track').onclick = () => { void action(async () => {
  if (audioStatus.playing || project.tracks.length <= 1) return;
  if (audioStatus.connected) await stopPlayback();
  const index = grid.activeTrack;
  envelopeEditor?.reset();
  deletedTrack = removeTrack(project, index);
  grid.resetHistory(); grid.selectTrack(Math.min(index, project.tracks.length - 1)); mixerView(); editedPattern();
}); };
element('restore-track').onclick = () => {
  if (audioStatus.playing || !deletedTrack || project.tracks.length >= 16) return;
  const index = deletedTrack.index;
  envelopeEditor?.reset();
  if (!restoreTrack(project, deletedTrack)) return;
  deletedTrack = null; grid.resetHistory(); grid.selectTrack(index); mixerView(); editedPattern();
};
element('remove-step').onclick = () => { if (!audioStatus.playing) arrangement.remove(); };
refreshProjectDir();
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
  envelopeEditor?.paint(state.pattern_id === arrangement.pattern.id ? state.row : null);
  grid.paint(state.pattern_id === arrangement.pattern.id ? state.row : null, state.row_phase, state.row_at_ms); renderApplyStatus();
});
void connectLanguageServer(status);
window.addEventListener('beforeunload', () => { clearInterval(audioTimer); clearInterval(scopeTimer); editor.destroy(); });
