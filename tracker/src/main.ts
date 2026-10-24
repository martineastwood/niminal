import { invoke, isTauri } from '@tauri-apps/api/core';
import { EditorView } from '@codemirror/view';
import { undo as editorUndo, redo as editorRedo } from '@codemirror/commands';
import { listen } from '@tauri-apps/api/event';
import { editorState, connectLanguageServer } from './editor';
import { MatrixPanel } from './matrix';
import type { BrowserItem } from './browser';
import { SoundBrowser, type SoundKind, type ManageOp } from './soundbrowser';
import { instrumentPresets, effectPresets, type Preset } from './library';
import { emptyProject, columnCount, emptyFxLane, blankColumn, type Device, readProject, storageKey, maxReturns, resizePattern, removeTrack, restoreTrack, trackSource, type Definition, type Song, type DeletedTrack } from './project';
import { Arrangement } from './arrangement';
import { PatternGrid } from './grid';
import { EnvelopeEditor } from './envelope-editor';
import { renderMixer, renderInstrument } from './mixer';
import { addDevice, removeDevice, moveDevice, newEffect, duplicateEffect, effectUsers, removeEffect, stripOf, columnToStrip, stripToColumn } from './effects';
import { MixPanel } from './mix';
import { drawScope } from './scope';
import { renameDefinition, freeCodeName, slug } from './naming';
import { presetSource, addInstrument, addInstrumentFromPreset, duplicateInstrument, instrumentUsers, removeInstrument } from './instruments';
import { audioCommand, audioStatus, observeAudio } from './audio';
import './style.css';

const app = document.querySelector<HTMLDivElement>('#app')!;
app.innerHTML = `
  <div id="toolbar">
    <div class="group"><button id="play" title="Loop pattern (Space)">▶ Pattern</button><button id="play-song" title="Play from the selected sequence step to the end, or repeat the loop once it is reached">▶ Song</button><button id="stop" title="Stop">■</button><button id="record" aria-pressed="false" title="Record mode: typed notes and values are entered (Esc)">● Rec</button><label class="check"><input id="follow" type="checkbox" checked> Follow</label><span id="play-position" class="muted"></span></div>
    <div class="group"><label>BPM <input id="bpm" type="number" min="20" max="300" step="1"></label><label>LPB <input id="rows-per-beat" type="number" min="1" max="16" title="Rows per beat"></label><label>Oct <input id="octave" type="number" min="0" max="8" value="4"></label><label>Step <input id="entry-step" aria-label="Note entry step" title="Rows to advance after entering a note; 0 stays on this row" type="number" min="0" max="256" value="1"></label></div>
    <div class="group"><strong id="pattern-number">00</strong><select id="pattern-picker" aria-label="Pattern to edit"></select><input id="pattern-name" aria-label="Pattern name"><label>Rows <input id="pattern-rows" type="number" min="1" max="256" data-structure></label><span id="pattern-uses" class="muted"></span></div>
    <div class="group"><button id="undo" title="Undo (Cmd/Ctrl+Z)" aria-label="Undo">↶</button><button id="redo" title="Redo (Cmd/Ctrl+Shift+Z)" aria-label="Redo">↷</button><button id="add-track" data-structure title="Add track" aria-label="Add track">+ Track</button><button id="delete-track" data-structure title="Delete selected track" aria-label="Delete selected track">− Track</button><button id="restore-track" data-structure title="Restore last deleted track" aria-label="Restore last deleted track">↺</button></div>
    <div class="group push"><button id="open-matrix" aria-pressed="false" title="Pattern matrix: mute a track, or have it play another pattern, in each sequence step">Matrix</button><button id="open-mix" aria-pressed="false" title="Mixer: levels, pan, mute, solo and a scope for each channel">Mixer</button><button id="import">Open</button><button id="export">Save</button></div>
  </div>
  <div id="scopebar"><canvas id="master-scope" aria-label="Master scope"></canvas></div>
  <main>
    <aside id="sequencer">
      <div class="side-heading"><span class="section-title">SEQUENCE</span><span class="side-tools"><button id="new-pattern" data-structure title="New blank pattern" aria-label="New blank pattern">+</button><button id="variation" data-structure title="Duplicate as variation" aria-label="Duplicate as variation">⧉</button><button id="reuse" data-structure title="Reuse pattern" aria-label="Reuse pattern">↳</button><button id="move-up" data-structure title="Move sequence step up" aria-label="Move sequence step up">↑</button><button id="move-down" data-structure title="Move sequence step down" aria-label="Move sequence step down">↓</button><button id="remove-step" data-structure title="Remove sequence step" aria-label="Remove sequence step">−</button><button id="section" title="Start a section at the selected step, or remove the one that starts there" aria-label="Start or remove a section" aria-pressed="false">§</button><button id="loop" title="Loop the selected step; shift-click another step to loop a range. Works while playing." aria-label="Loop sequence step" aria-pressed="false">⟲</button><button id="sequence-names" aria-label="Toggle pattern names" title="Show / hide pattern names" aria-expanded="true">◂</button></span></div>
      <div id="sequence"></div>
    </aside>
    <section class="workspace">
      <div id="grid-wrap"><div id="grid" tabindex="0" role="grid" aria-label="Pattern editor"></div></div>
      <div id="bottom-dock" hidden>
        <div class="dock-resize" id="dock-resize"></div>
        <div class="dock-bar"><button class="tab" data-tab="code">Code</button><button class="tab" data-tab="automation">Automation</button><button class="tab" data-tab="mix">Mixer</button><button class="tab" data-tab="matrix">Matrix</button><span id="code-tools"><span class="segmented"><button id="show-instrument" title="This track's instrument">Instrument</button><button id="show-fx" title="This track's effects">FX</button></span><span id="definition-kind" class="badge"></span><input id="definition-name" aria-label="Definition name"><button id="apply" title="Apply the code and play a note through it (Cmd/Ctrl+Enter in the editor)">Apply ⌘↩</button><span id="apply-status" class="muted"></span></span><span class="spacer"></span><button id="expand" title="Expand editor" aria-label="Expand editor">⤢</button><button id="close-dock" title="Close" aria-label="Close panel">×</button></div>
        <section id="automation-panel" hidden><div id="automation"></div></section>
        <section id="mix-panel" hidden><div id="mix"></div></section>
        <section id="matrix-panel" hidden><div id="matrix"></div></section>
        <section class="instrument-editor" id="code-panel" hidden><div class="code-workspace"><div id="mixer" aria-label="Track FX controls" hidden></div><div id="code"></div></div></section>
      </div>
    </section>
    <section id="sound-browser" aria-label="Sound browser"></section>
  </main>
  <footer><span id="status">Starting editor…</span><span class="spacer"></span><span id="audio-status"></span><label>Port <input id="engine-port" type="number" min="1" max="65535" value="7401"></label><input id="engine-token" type="password" placeholder="Token" aria-label="Engine token"><button id="connect">Connect</button><span id="save-status">Local autosave</span></footer>`;

const element = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
function setText(id: string, value: string) {
  const node = element(id);
  if (node.textContent !== value) node.textContent = value;
}
const status = (text: string) => { setText('status', text); };
let project = emptyProject();
try { const raw = localStorage.getItem(storageKey); if (raw) project = readProject(raw); }
catch (error) { status(`Could not restore project: ${error}`); }
let selected = project.definitions.find(d => d.kind === 'instr') ?? project.definitions[0];
/** The instrument notes are given as they are entered: the last one chosen in the browser or shown in the code pane. */
let currentInstrument = selected.id;
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
  return editorState(documentUri(definition.id), definition.source, source => { definition.source = source; save(); renderApplyStatus(); }, () => { void action(() => applyAndAudition(definition)); });
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
  if (definition.kind === 'instr') currentInstrument = definition.id;
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
/** The browser's Project folder lists the definitions, so it follows them. */
function renderDefinitions() { soundBrowser.refresh(); }
/** Effect and instrument parameters share one table: definition ids are unique across both. */
function allParameters() { return { ...audioStatus.instrument_parameters, ...audioStatus.effect_parameters }; }
function song(): Song {
  const instrument_values = Object.fromEntries(project.definitions.filter(d => d.kind === 'instr' && d.values).map(d => [d.id, d.values!]));
  return { bpm: project.bpm, rows_per_beat: project.rows_per_beat, tracks: project.tracks, returns: project.returns, master: project.master, patterns: project.patterns, sequence: project.sequence, loop_range: project.loop_range, matrix: project.matrix, instrument_values };
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
/** Applies a definition, then plays a note through it: an instrument on the selected track, or the track's instrument through an effect. */
async function applyAndAudition(definition: Definition) {
  await applyDefinition(definition);
  const track = Math.max(0, deviceTrack);
  const instrument = definition.kind === 'instr' ? definition.id : currentInstrument;
  if (definition.kind === 'fx' && !stripOf(project, deviceTrack).fx.some(d => d.definition === definition.id)) { status('Add this effect to the track to hear it'); return; }
  await audition(instrument, 'C-3', track);
}
async function ensureSound(id: string) {
  const definition = project.definitions.find(d => d.id === id);
  if (!definition) throw new Error('Sound definition is missing');
  const ready = definition.kind === 'instr' ? audioStatus.instruments[id] : audioStatus.effects[id];
  if (!ready) await applyDefinition(definition);
}
/** Every instrument the song can play: those set in an instrument column. */
function usedInstruments() {
  const ids = new Set<string>();
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
  refreshMix(); refreshMatrix(); browserTarget(); soundBrowser.refresh();
  element('show-fx').setAttribute('aria-pressed', String(fxMode)); element('show-instrument').setAttribute('aria-pressed', String(!fxMode));
  if (fxMode && libraryFx !== null) { element('mixer').hidden = true; return; }
  element('mixer').hidden = false;
  if (fxMode && !element('code-panel').hidden && shownEffect() !== (currentDevice()?.definition ?? null)) { showFx(); return; }
  envelopeEditor?.select(stripToColumn(project, deviceTrack));
  if (!fxMode) { renderInstrument(element('mixer'), selected, allParameters(), editedPattern, (target, value) => grid.writeParameter(stripToColumn(project, deviceTrack), target, value), deviceTrack); return; }
  const device = currentDevice();
  renderMixer(element('mixer'), project, deviceTrack, () => { editedPattern(); grid.render(); }, allParameters(), (target, value) => grid.writeParameter(stripToColumn(project, deviceTrack), target, value), {
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
async function audition(id: string, note: string, track: number, ms = 300) {
  await ensureSound(id); await ensureAssignments();
  await audioCommand({ op: 'update', song: song() });
  await audioCommand({ op: 'audition', instrument: id, note, track, ms });
}
/** Slots in the engine for what the sound browser is playing; they are not part of the project. */
const previewInstrument = 'preview_instr', previewEffect = 'preview_fx';
let previewingEffect = false;
/** Plays a ready-made instrument on the selected track, without adding it to the project. */
async function previewPreset(preset: Preset) {
  const track = noteTrack();
  await projectDirReady;
  await ensureAssignments();
  await audioCommand({ op: 'apply', id: previewInstrument, source: presetSource(preset, 'browser_preview'), kind: 'instr', dir: projectDir });
  await audioCommand({ op: 'update', song: song() });
  await audioCommand({ op: 'audition', instrument: previewInstrument, note: 'C-3', track, ms: 700 });
}
/** Hears a ready-made effect on the selected track: added to its chain for as long as the browser is previewing, with a note played through it unless the song is playing. */
async function previewEffectPreset(preset: { source: string }) {
  const track = noteTrack(), instrument = currentInstrument;
  await projectDirReady;
  await ensureAssignments();
  await audioCommand({ op: 'apply', id: previewEffect, source: preset.source.replace(/^fx \w+/m, 'fx browser_preview_fx'), kind: 'fx', dir: projectDir });
  const preview = song();
  preview.tracks = preview.tracks.map((t, index) => index === track ? { ...t, fx: [...t.fx, { id: 'd_preview', definition: previewEffect, bypass: false, values: {}, modulations: {} }] } : t);
  previewingEffect = true;
  await audioCommand({ op: 'update', song: preview });
  if (!audioStatus.playing) await audioCommand({ op: 'audition', instrument, note: 'C-3', track, ms: 1200 });
}
/** The browser is done previewing: take the effect it added back out of the chain. */
async function endPreview() {
  if (!previewingEffect) return;
  previewingEffect = false;
  await audioCommand({ op: 'update', song: song() });
}
/** The strip of the grid column the cursor is in. */
const activeStrip = () => columnToStrip(project, Math.min(grid.activeTrack, columnCount(project) - 1));
/** The track notes are auditioned on: the cursor's, or the last track when it is in a return or the master. */
const noteTrack = () => Math.min(grid.activeTrack, project.tracks.length - 1);
const arrangement = new Arrangement(element('sequence'), () => project, manual => {
  if (manual) element<HTMLInputElement>('follow').checked = false;
  grid.render(); deviceTrack = activeStrip(); mixerView(); patternControls();
}, editedPattern);
const grid = new PatternGrid(element('grid'), () => project, () => arrangement.pattern, () => Number(element<HTMLInputElement>('octave').value), () => { editedPattern(); mixerView(); },
  (id, note, track) => { if (audioStatus.connected && !audioStatus.playing) void action(() => audition(id, note, track)); },
  () => { void action(() => togglePlay()); },
  column => { const strip = columnToStrip(project, column); if (deviceTrack !== strip) { const editingFx = fxMode && !element('code-panel').hidden; deviceTrack = strip; if (editingFx) showFx(); else mixerView(); } },
  () => Number(element<HTMLInputElement>('entry-step').value), () => allParameters(), status, (track, lane) => {
    grid.selectTrack(track); showPanel('automation', true); envelopeEditor?.select(track, lane);
  }, action => {
    const last = project.sequence.length - 1;
    if (action === 'previous') arrangement.select(Math.max(0, arrangement.position - 1));
    else if (action === 'next') arrangement.select(Math.min(last, arrangement.position + 1));
    else if (action === 'first') arrangement.select(0);
    else if (action === 'last') arrangement.select(last);
    else if (audioStatus.playing) return; // the sequence cannot change while it plays
    else if (action === 'new') arrangement.insert('new');
    else if (action === 'remove') arrangement.remove();
    else arrangement.shiftPattern(action === 'pattern-next' ? 1 : -1);
  }, () => currentInstrument);
const presetItem = (p: Preset): BrowserItem => ({ value: p.id, name: p.name, folder: p.folder, description: p.description });
/** Loads a ready-made instrument into an existing slot: its name and code are replaced, and the slot keeps its number. */
function loadPresetIntoSlot(definition: Definition, preset: Preset) {
  definition.name = preset.name;
  definition.source = presetSource(preset, freeCodeName(project, slug(preset.name), definition));
  delete definition.values;
  states.delete(definition.id);
  if (selected === definition) editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: definition.source } });
  else selectDefinition(definition, false);
  renderDefinitions(); grid.render(); mixerView(); save();
  if (audioStatus.connected) void action(() => applyAndAudition(definition));
}
/** Makes an instrument from a preset and chooses it, so the notes entered next use it. */
function useInstrumentPreset(preset: Preset) {
  const definition = addInstrumentFromPreset(project, preset);
  selectDefinition(definition); grid.render(); mixerView(); save();
}
/** Adds an effect to the end of the selected strip's chain and shows its code: a preset's id, or `fx:<id>` for one already in the project. */
function addEffectToStrip(choice: string) {
  const added = addDevice(project, deviceTrack, choice);
  if (!added) { status('This strip has no room for another effect'); return; }
  chosenDevice.set(deviceTrack, added.device.id); showFx(); editedPattern(); grid.render();
  if (audioStatus.connected) void action(() => ensureSound(added.definition.id));
}
/** What the browser lists: this project's own sounds first (instruments numbered as the Ins column counts them), then the ready-made ones. */
function browserItems(kind: SoundKind): BrowserItem[] {
  const own = project.definitions.filter(d => d.kind === kind).map((definition, index) => {
    const users = kind === 'instr' ? instrumentUsers(project, definition.id).length : effectUsers(project, definition.id).length;
    return { value: `project:${definition.id}`, name: kind === 'instr' ? `${String(index).padStart(2, '0')}  ${definition.name}` : definition.name, folder: 'Project', description: 'A sound of this project', note: users ? `×${users}` : undefined };
  });
  return [...own, ...(kind === 'instr' ? instrumentPresets : effectPresets).map(presetItem)];
}
/** The project's own definition behind an item of the Project folder. */
const ownDefinition = (item: BrowserItem) => item.value.startsWith('project:') ? project.definitions.find(d => d.id === item.value.slice(8)) : undefined;
const soundBrowser = new SoundBrowser(element('sound-browser'), browserItems, {
  audition: (kind, item) => {
    const own = ownDefinition(item), preset = own ? undefined : (kind === 'instr' ? instrumentPresets : effectPresets).find(p => p.id === item.value)!;
    // landing on an instrument of the project chooses it: its code shows, and the notes entered next use it
    if (own && kind === 'instr') selectDefinition(own, false);
    if (!audioStatus.connected) return;
    if (kind === 'instr') void action(() => own ? audition(own.id, 'C-3', noteTrack(), 700) : previewPreset(preset!));
    else void action(() => previewEffectPreset(own ?? preset!));
  },
  pick: (kind, item, replace) => {
    const own = ownDefinition(item);
    void action(endPreview).then(() => {
      if (kind === 'fx') { addEffectToStrip(own ? `fx:${own.id}` : item.value); return; }
      // an instrument becomes the current one, and the cursor goes back to the grid to enter notes with it
      const preset = own ? undefined : instrumentPresets.find(p => p.id === item.value)!, slot = project.definitions.find(d => d.id === currentInstrument);
      if (own) selectDefinition(own);
      else if (replace && slot) loadPresetIntoSlot(slot, preset!);
      else useInstrumentPreset(preset!);
      element('grid').focus();
    });
  },
  manage: manageSound,
  leave: () => { void action(endPreview); },
}, () => element('grid').focus());
/** Shows or hides the browser; showing it puts the cursor in its search box. */
function toggleBrowser(kind?: SoundKind) {
  const section = element('sound-browser');
  if (!section.hidden && section.contains(document.activeElement) && !kind) { section.hidden = true; element('grid').focus(); return; }
  section.hidden = false;
  if (kind) soundBrowser.show(kind); else soundBrowser.focus();
}
function browserTarget() {
  const instruments = project.definitions.filter(d => d.kind === 'instr'), at = instruments.findIndex(d => d.id === currentInstrument), strip = stripOf(project, deviceTrack);
  soundBrowser.setTarget(`Notes use ${at < 0 ? 'no instrument' : `${String(at).padStart(2, '0')} ${instruments[at].name}`} · effects go on ${strip.name}`);
}
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
    project.returns.push({ name: `Return ${String.fromCharCode(65 + project.returns.length)}`, fx: [], pan: 0, muted: false, volume_db: 0 });
    for (const pattern of project.patterns) pattern.return_fx.push([emptyFxLane(pattern.rows)]);
    editedPattern(); grid.render(); mixerView();
  },
  removeReturn: index => {
    project.returns.splice(index, 1);
    for (const pattern of project.patterns) pattern.return_fx.splice(index, 1);
    for (const track of [...project.tracks, ...project.returns]) {
      // A strip that fed the removed return goes back to the master; later returns shift down.
      if (track.output?.kind === 'return') track.output = track.output.index === index ? { kind: 'master' } : track.output.index > index ? { kind: 'return', index: track.output.index - 1 } : track.output;
      for (const device of track.fx) if (device.target !== undefined && device.target > index) device.target--;
    }
    if (deviceTrack < -1) { deviceTrack = activeStrip(); if (fxMode) showFx(); }
    editedPattern(); grid.render(); mixerView();
  },
});
const matrix = new MatrixPanel(element('matrix'), () => project, { select: step => arrangement.select(step), edited: () => { editedPattern(); grid.render(); } });
function refreshMatrix() {
  if (element('matrix-panel').hidden) return;
  matrix.render(arrangement.position);
}
/** What the pattern editor shows for a track whose notes in the selected step come from somewhere else. */
grid.slot = track => {
  const source = trackSource(project, arrangement.position, track);
  if (!source) return null;
  return source.kind === 'mute' ? { kind: 'mute', text: 'Muted in this step: its notes here are not played (see Matrix)' } : { kind: 'alias', text: `Plays pattern ${project.patterns.findIndex(p => p.id === source.id)} in this step: edits here are not heard (see Matrix)` };
};
function refreshMix() {
  if (element('mix-panel').hidden) return;
  mix.render(); mix.highlight(deviceTrack);
}
let scopes: Record<string, number[]> = {};
let watchingStrips: boolean | null = null;
/** The engine only reports the strip scopes while the mixer is on screen. */
function watchStrips() {
  const strips = !element('mix-panel').hidden;
  if (strips === watchingStrips) return;
  watchingStrips = strips;
  void audioCommand({ op: 'scopes', strips }).catch(() => { watchingStrips = null; });
}
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
/** Connects to the engine, or disconnects from it. The first connection starts the engine if none is running. */
function toggleConnection() {
  if (connectionPending) return;
  connectionPending = audioStatus.connected ? 'disconnect' : 'connect';
  renderConnectButton();
  if (connectionPending === 'connect') setText('audio-status', 'Starting the engine…');
  void action(async () => {
    transportRevision++; starting = false;
    appliedSources.clear();
    if (audioStatus.connected) { await audioCommand({ op: 'disconnect' }); return; }
    const port = Number(element<HTMLInputElement>('engine-port').value);
    if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('Enter an engine port between 1 and 65535');
    await audioCommand({ op: 'connect', port, token: element<HTMLInputElement>('engine-token').value || null });
  }).finally(() => { connectionPending = null; renderConnectButton(); });
}
element('connect').onclick = toggleConnection;
element('play').onclick = () => { void action(() => togglePlay('pattern')); };
element('play-song').onclick = () => { void action(() => togglePlay('song')); };
element('stop').onclick = () => { void action(stopPlayback); };
element('show-instrument').onclick = () => { selectDefinition(project.definitions.find(d => d.id === currentInstrument) ?? project.definitions.find(d => d.kind === 'instr')!); editor.focus(); };
element('show-fx').onclick = () => { if (deviceTrack < 0 && !stripOf(project, deviceTrack)) deviceTrack = 0; openTrackFx(); };
element('apply').onclick = () => { const definition = selected; void action(() => applyAndAudition(definition)); };
function timingInputs() {
  element<HTMLInputElement>('bpm').value = String(project.bpm);
  element<HTMLInputElement>('rows-per-beat').value = String(project.rows_per_beat);
}
for (const [id, field, min, max] of [['bpm', 'bpm', 20, 300], ['rows-per-beat', 'rows_per_beat', 1, 16]] as const) {
  element<HTMLInputElement>(id).onchange = event => {
    const input = event.target as HTMLInputElement;
    if (audioStatus.playing) { timingInputs(); return; }
    project[field] = Math.min(max, Math.max(min, Math.trunc(Number(input.value) || min)));
    if (field === 'rows_per_beat') grid.render(); // the beat and bar shading follows it
    timingInputs(); save();
  };
}
element<HTMLInputElement>('entry-step').onchange = event => {
  const input = event.target as HTMLInputElement; input.value = String(Math.max(0, Math.min(256, Math.trunc(Number(input.value) || 0))));
};
element('section').onclick = () => { arrangement.toggleSection(); patternControls(); };
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
/** Renames the selected definition, and the name in its code with it. */
element<HTMLInputElement>('definition-name').onchange = event => {
  const input = event.target as HTMLInputElement, before = selected.source;
  renameDefinition(project, selected, input.value);
  input.value = selected.name;
  if (selected.source !== before) editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: selected.source } });
  renderDefinitions(); grid.render(); mixerView(); save();
};
function newInstrument(sample?: string) {
  const definition = addInstrument(project, sample);
  selectDefinition(definition); editor.focus(); grid.render(); mixerView(); save();
  return definition;
}
async function importSamples() {
  if (!isTauri()) { status('Importing samples needs the desktop app'); return; }
  try {
    const files = await invoke<string[]>('import_samples', { id: project.id });
    for (const file of files) newInstrument(file);
    if (files.length) status(`Added ${files.length} sampler${files.length > 1 ? 's' : ''}`);
  } catch (error) { status(`Could not import samples: ${error}`); }
}
/** The buttons under the browser's list: new, import, and what can be done to a sound of this project. */
function manageSound(op: ManageOp, kind: SoundKind, item: BrowserItem | null) {
  if (op === 'new') { if (kind === 'instr') newInstrument(); else newLibraryEffect(); return; }
  if (op === 'import') { void importSamples(); return; }
  const definition = item?.value.startsWith('project:') ? project.definitions.find(d => d.id === item.value.slice(8)) : undefined;
  if (!definition) { status('Select one of this project\'s own sounds, in the Project folder'); return; }
  const library = kind === 'fx';
  if (op === 'edit') { selectDefinition(definition, true, library); editor.focus(); return; }
  if (op === 'duplicate') {
    const copy = library ? duplicateEffect(project, definition) : duplicateInstrument(project, definition);
    selectDefinition(copy, true, library); editor.focus(); save();
    return;
  }
  const problem = library ? removeEffect(project, definition.id) : removeInstrument(project, definition.id);
  if (problem) { status(problem); return; }
  states.delete(definition.id);
  selectDefinition(project.definitions.find(d => d.kind === 'instr')!, false); grid.render(); save();
}
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
  project = loaded; deletedTrack = null; deviceTrack = 0; chosenDevice.clear(); fxMode = false; refreshProjectDir(); selected = project.definitions.find(d => d.kind === 'instr') ?? project.definitions[0];
  const definition = selected;
  editor = new EditorView({ state: documentState(definition), parent: element('code') });
  selectDefinition(selected, false); arrangement.reset(); grid.resetHistory(); grid.selectTrack(0); mixerView(); patternControls(); timingInputs(); save();
  if (isTauri()) toggleConnection();
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
type Panel = 'code' | 'automation' | 'mix' | 'matrix';
const panels: Panel[] = ['code', 'automation', 'mix', 'matrix'];
function showPanel(panel: Panel, visible: boolean) {
  for (const candidate of panels) {
    const section = element(`${candidate}-panel`);
    if (visible || candidate === panel) section.hidden = !visible || candidate !== panel;
    document.querySelector(`[data-tab=${candidate}]`)!.setAttribute('aria-selected', String(!section.hidden));
  }
  element('code-tools').hidden = element('code-panel').hidden;
  element('bottom-dock').hidden = panels.every(candidate => element(`${candidate}-panel`).hidden);
  element('open-mix').setAttribute('aria-pressed', String(!element('mix-panel').hidden));
  element('open-matrix').setAttribute('aria-pressed', String(!element('matrix-panel').hidden));
  if (!element('mix-panel').hidden) refreshMix();
  watchStrips();
  refreshMatrix();
  if (element('code-panel').hidden) {
    element('app').classList.remove('editor-expanded'); setText('expand', '⤢');
  }
  if (editor) editor.requestMeasure();
}
for (const panel of panels) {
  document.querySelector<HTMLElement>(`[data-tab=${panel}]`)!.onclick = () => {
    showPanel(panel, true);
    if (panel === 'automation') envelopeEditor?.select(stripToColumn(project, deviceTrack));
    if (panel === 'code' && fxMode) showFx();
  };
}
element('open-mix').onclick = () => showPanel('mix', element('mix-panel').hidden);
element('open-matrix').onclick = () => showPanel('matrix', element('matrix-panel').hidden);
element('close-dock').onclick = () => { showPanel('code', false); element('grid').focus(); };
element('dock-resize').onpointerdown = event => {
  const dock = element('bottom-dock'), startY = event.clientY, startHeight = dock.offsetHeight;
  const target = event.currentTarget as HTMLElement; target.setPointerCapture(event.pointerId);
  target.onpointermove = move => { dock.style.height = `${Math.max(120, startHeight + startY - move.clientY)}px`; editor.requestMeasure(); };
  target.onpointerup = () => { target.onpointermove = null; target.onpointerup = null; };
};
function patternControls() {
  const current = arrangement.pattern;
  refreshMatrix();
  element('loop').setAttribute('aria-pressed', String(!!project.loop_range));
  element('section').setAttribute('aria-pressed', String(!!project.sections?.some(s => s.start === arrangement.position)));
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
  project.tracks.push({ name: `Track ${String(project.tracks.length + 1).padStart(2, '0')}`, fx: [], pan: 0, muted: false, volume_db: 0 });
  for (const pattern of project.patterns) { const rows = pattern.notes[0][0].notes.length; pattern.notes.push([blankColumn(rows)]); pattern.fx.push([emptyFxLane(rows)]); }
  grid.selectTrack(project.tracks.length - 1); editedPattern();
};
element('delete-track').onclick = () => { void action(async () => {
  if (audioStatus.playing || project.tracks.length <= 1) return;
  if (audioStatus.connected) await stopPlayback();
  const index = grid.activeTrack;
  if (index >= project.tracks.length) return; // the cursor is in a return or the master, which cannot be deleted here
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
showPanel('code', true);
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
  matrix.paint(state.playing ? state.sequence_position : null);
  if (state.playing && element<HTMLInputElement>('follow').checked && state.pattern_id && (arrangement.pattern.id !== state.pattern_id || state.sequence_position !== null && arrangement.position !== state.sequence_position)) {
    if (state.sequence_position !== null) arrangement.select(state.sequence_position, false);
    else { arrangement.choose(state.pattern_id); element<HTMLInputElement>('follow').checked = true; }
  }
  setText('play-position', state.playing && state.pattern_id ? `${state.sequence_position === null ? 'Loop' : `Step ${state.sequence_position + 1}`} · row ${state.row ?? '…'}` : '');
  element<HTMLInputElement>('bpm').disabled = state.playing;
  element<HTMLInputElement>('rows-per-beat').disabled = state.playing;
  setText('audio-status', audioError ?? state.error ?? (state.connected ? `${state.playing ? 'Playing' : 'Connected'} · ${state.sample_rate / 1000} kHz${state.skipped_rows ? ` · ${state.skipped_rows} skipped rows` : ''}${state.late_events ? ` · ${state.late_events} late events` : ''}` : 'Engine not connected'));
  envelopeEditor?.paint(state.pattern_id === arrangement.pattern.id ? state.row : null);
  grid.paint(state.pattern_id === arrangement.pattern.id ? state.row : null, state.row_phase, state.row_at_ms); renderApplyStatus();
});
void connectLanguageServer(status);

/** The menu bar: each item runs what its button or key already does. */
const menuActions: Record<string, () => void> = {
  new: () => {
    void invoke<boolean>('confirm', { message: 'Start a new project? This one is only kept if you have saved it.' })
      .then(ok => ok ? loadProject(JSON.stringify(emptyProject())).then(() => status('New project')) : undefined)
      .catch(error => status(`Could not start a new project: ${error}`));
  },
  open: () => element('import').click(),
  save: () => element('export').click(),
  'import-samples': () => void importSamples(),
  undo: () => { if (editor.hasFocus) editorUndo(editor); else if (document.activeElement instanceof HTMLInputElement) document.execCommand('undo'); else grid.undo(); },
  redo: () => { if (editor.hasFocus) editorRedo(editor); else if (document.activeElement instanceof HTMLInputElement) document.execCommand('redo'); else grid.redo(); },
  'play-pattern': () => element('play').click(),
  'play-song': () => element('play-song').click(),
  stop: () => element('stop').click(),
  'add-track': () => element('add-track').click(),
  'new-pattern': () => element('new-pattern').click(),
  reconnect: () => { void action(async () => { if (audioStatus.connected) await audioCommand({ op: 'disconnect' }); }).then(toggleConnection); },
  'new-instrument': () => newInstrument(),
  'new-effect': () => newLibraryEffect(),
  apply: () => element('apply').click(),
  code: () => showPanel('code', true),
  mixer: () => element('open-mix').click(),
  matrix: () => element('open-matrix').click(),
  browser: () => toggleBrowser(),
};
if (isTauri()) {
  listen<string>('menu', event => menuActions[event.payload]?.()).catch(error => status(`The menu is not connected: ${error}`));
  // the first connection starts the engine when none is running, so there is nothing to run by hand
  toggleConnection();
}
window.addEventListener('beforeunload', () => { clearInterval(audioTimer); clearInterval(scopeTimer); editor.destroy(); });
