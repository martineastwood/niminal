import { invoke, isTauri } from '@tauri-apps/api/core';
import { EditorView } from '@codemirror/view';
import { mountMidi, type MidiMessage } from './midi';
import { ProjectHistory } from './history';
import { patternTiming, validTiming, type Project } from './project';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { editorState, connectLanguageServer } from './editor';
import type { BrowserItem } from './browser';
import { ProjectPanel, neighbour } from './projectpanel';
import { openMenu, type MenuEntry } from './menu';
import { openPalette, openShortcuts, type Command } from './palette';
import { shortcutSections } from './shortcuts';
import { loadView, saveView, applyView, stepScale, clampFont, dragWidth, defaultView, type ViewPrefs } from './view';
import { SamplePanel } from './waveform';
import { pieceKey } from './slices';
import { SoundBrowser, type SoundKind, type SoundBrowserActions, type SamplePeaks } from './soundbrowser';
import type { Dragged } from './drag';
import { instrumentPresets, effectPresets, type Preset } from './library';
import { emptyProject, columnCount, emptyFxLane, blankColumn, type Device, readProject, dropUnusedPatterns, storageKey, maxReturns, resizePattern, removeTrack, restoreTrack, moveTrack, duplicateTrack, trackSource, type Definition, type Song, type DeletedTrack } from './project';
import { Arrangement } from './arrangement';
import { PatternGrid } from './grid';
import { EnvelopeEditor } from './envelope-editor';
import { renderMixer, renderInstrument } from './mixer';
import { addDevice, effectChoices, removeDevice, moveDevice, commandsFor, newEffect, duplicateEffect, effectUsers, removeEffect, stripOf, columnToStrip, stripToColumn } from './effects';
import { MixPanel } from './mix';
import { drawScope } from './scope';
import { renameDefinition, freeCodeName, slug } from './naming';
import { presetSource, addInstrument, addInstrumentFromPreset, duplicateInstrument, instrumentUsers, removeInstrument, sliceInstrument, sampleFile, maxSlices, addKit, samplerSource, withSample } from './instruments';
import { readPlayback, playbackNames, playbackHints } from './playback';
import { audioCommand, audioStatus, observeAudio } from './audio';
import './style.css';

type CodeKind = 'instr' | 'fx';
/** The name, kind badge and Apply button above a definition's code. */
const codeTools = (kind: CodeKind) => `<span id="${kind}-tools" class="code-tools"><span id="${kind}-kind" class="badge"></span><input id="${kind}-name" class="definition-name" aria-label="Definition name"><button id="${kind}-apply" class="apply" title="Apply the code and play a note through it (Cmd/Ctrl+Enter in the editor)">Apply ⌘↩</button><span id="${kind}-status" class="muted"></span></span>`;
const app = document.querySelector<HTMLDivElement>('#app')!;
app.innerHTML = `
  <div id="toolbar">
    <div class="group"><button id="play" title="Loop pattern (Space)">▶ Pattern</button><button id="play-song" title="Play from the selected sequence step to the end, or repeat the loop once it is reached">▶ Song</button><button id="stop" title="Stop">■</button><button id="record" aria-pressed="false" title="Record mode: typed notes and values are entered (Esc)">● Rec</button><label class="check" title="Follow the song as it plays: the pattern changes with it, and while recording the cursor rides the play position. Untick to move around and edit freely while it plays"><input id="follow" type="checkbox" checked> Follow</label><label class="check" title="Volume 0 to 100 given to notes entered in record mode; blank leaves it unset">Vel <input id="velocity" type="number" min="0" max="100" placeholder="–" style="width:3.5em"></label><span id="play-position" class="muted"></span><span id="midi-controls" class="group"></span></div>
    <div class="group"><label>BPM <input id="bpm" type="number" min="20" max="300" step="1"></label><label>LPB <input id="rows-per-beat" type="number" min="1" max="16" title="Rows per beat"></label><label>Swing <input id="swing" type="number" min="0" max="75" title="Swing: how much of every pair of rows' time moves to the first row, 0 to 75 percent. Acts on rows, so LPB sets the subdivision swung"></label><label>Oct <input id="octave" type="number" min="0" max="8" value="4"></label><label>Step <input id="entry-step" aria-label="Note entry step" title="Rows to advance after entering a note; 0 stays on this row" type="number" min="0" max="256" value="4"></label></div>
    <div class="group"><label>Rows <input id="pattern-rows" type="number" min="1" max="256" data-structure></label></div>
    <div class="group"><button id="undo" title="Undo (Cmd/Ctrl+Z)" aria-label="Undo">↶</button><button id="redo" title="Redo (Cmd/Ctrl+Shift+Z)" aria-label="Redo">↷</button><button id="add-track" data-structure title="Add track" aria-label="Add track">+ Track</button><button id="delete-track" data-structure title="Delete selected track" aria-label="Delete selected track">− Track</button><button id="duplicate-track" data-structure title="Duplicate the selected track, with its effects and notes" aria-label="Duplicate the selected track">⧉ Track</button><button id="restore-track" data-structure title="Restore last deleted track" aria-label="Restore last deleted track">↺</button></div>
    <div class="group push"><button id="open-mix" aria-pressed="false" title="Mixer: levels, pan, mute, solo and a scope for each channel">Mixer</button><button id="import">Open</button><button id="export">Save</button></div>
  </div>
  <div id="scopebar"><canvas id="master-scope" aria-label="Master scope"></canvas></div>
  <main>
    <div class="stage"><div class="top">
    <aside id="sequencer">
      <div class="side-heading"><span class="section-title">SEQUENCE</span></div>
      <div class="sequence-tools"><button id="sequence-add" title="Add a pattern after the selected step, or start a section">+ Add</button><button id="loop" title="Loop the selected step. Drag down the strip at the left of the steps to loop a range. Works while playing." aria-pressed="false">Loop</button><button id="sequence-cells" title="Show a cell for each track on every step, to mute it or play another pattern there" aria-pressed="false">Tracks</button></div>
      <div class="sequence-lock" hidden>Playing: stop to add, move or remove steps</div>
      <div id="sequence"></div>
    </aside>
    <div class="col-resize" id="left-resize" tabindex="0" role="separator" aria-orientation="vertical" aria-label="Resize the sequence bar"></div>
    <section class="workspace">
      <div class="dock-bar" id="view-bar"><button class="tab" data-view="pattern" title="The pattern editor">Pattern</button><button class="tab" data-view="instrument" title="The current instrument: its controls and code, shared by every track">Instrument</button><button class="tab" data-view="sample" title="The current instrument's sample">Sample</button><button class="tab" data-view="mix" title="Levels, pan, mute, solo and a scope for each channel">Mixer</button><span class="spacer"></span><button id="toggle-dock" class="tab" aria-pressed="false" title="Show or hide the selected track's effects and automation">Track FX</button></div>
      <div id="grid-wrap"><div id="grid" tabindex="0" role="grid" aria-label="Pattern editor"></div></div>
      <section class="main-view" id="instrument-view" hidden><div class="code-bar">${codeTools('instr')}</div><div class="code-workspace"><div id="instr-controls" aria-label="Instrument controls"></div><div id="instr-code"></div></div></section>
      <section class="main-view" id="sample-panel" hidden><div id="sample"></div></section>
      <section class="main-view" id="mix-panel" hidden><div id="mix"></div></section>
    </section>
    </div>
      <div id="bottom-dock" hidden>
        <div class="dock-resize" id="dock-resize"></div>
        <div class="dock-row"><section id="fx-browser" aria-label="Effect browser"></section><div class="dock-main">
        <div class="dock-bar"><span id="dock-track" class="dock-track" title="The selected track: its effects and automation"></span><button class="tab" data-tab="fx" title="The selected track's effect chain">FX</button><button class="tab" data-tab="automation" title="Automation for the selected track">Automation</button><button id="fx-code-toggle" class="tab" title="The code of the selected effect">Code</button>${codeTools('fx')}<span class="spacer"></span><button id="expand" title="Expand editor" aria-label="Expand editor">⤢</button><button id="close-dock" title="Close" aria-label="Close panel">×</button></div>
        <section id="automation-panel" hidden><div id="automation"></div></section>
        <section id="code-panel" hidden><div class="code-workspace"><div id="mixer" aria-label="Track FX controls" hidden></div><div id="code"></div></div></section>
        </div></div>
      </div>
    </div>
    <div class="col-resize" id="right-resize" tabindex="0" role="separator" aria-orientation="vertical" aria-label="Resize the right bar"></div>
    <div id="right-bar"><section id="project-panel" aria-label="Project instruments"></section><section id="sound-browser" aria-label="Sound browser"></section></div>
  </main>
  <footer><span id="status">Starting editor…</span><span class="spacer"></span><span id="audio-status"></span><label>Port <input id="engine-port" type="number" min="1" max="65535" value="7401"></label><input id="engine-token" type="password" placeholder="Token" aria-label="Engine token"><button id="connect">Connect</button><span id="save-status">Local autosave</span><span id="view-controls"><button id="toggle-sequence" aria-pressed="true" title="Show or hide the sequence bar (Cmd/Ctrl+\\)" aria-label="Sequence bar">◧</button><button id="toggle-side" aria-pressed="true" title="Show or hide the right bar (Cmd/Ctrl+Shift+B)" aria-label="Right bar">◨</button><button id="zoom-out" title="Smaller interface (Cmd/Ctrl+−)" aria-label="Smaller interface">A−</button><button id="zoom-level" title="Interface size; click for actual size" aria-label="Interface size"></button><button id="zoom-in" title="Larger interface (Cmd/Ctrl+=)" aria-label="Larger interface">A+</button><button id="open-palette" title="Command palette (Cmd/Ctrl+K)" aria-label="Command palette">⌘K</button></span></footer>`;

const element = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
function setText(id: string, value: string) {
  const node = element(id);
  if (node.textContent !== value) node.textContent = value;
}
const status = (text: string) => { setText('status', text); };
let project = emptyProject();
try { const raw = localStorage.getItem(storageKey); if (raw) project = readProject(raw); dropUnusedPatterns(project); }
catch (error) { status(`Could not restore project: ${error}`); }
let selected = project.definitions.find(d => d.kind === 'instr') ?? project.definitions[0];
/** The instrument notes are given as they are entered: the last one chosen in the browser or shown in the code pane. */
let currentInstrument = selected.id;
/** Instruments are edited in the Instrument view and effects in the dock, each in its own editor. `editor` and `selected` are whichever was opened last. */
let editor: EditorView;
const editors: Partial<Record<CodeKind, EditorView>> = {};
const shown: Partial<Record<CodeKind, Definition>> = {};
const parts = { instr: 'instr-code', fx: 'code' } as const;
const measure = () => { for (const each of Object.values(editors)) each.requestMeasure(); };
const codeFocused = () => Object.values(editors).some(each => each.hasFocus);
const replaceSource = (definition: Definition, source: string) => {
  const open = editors[definition.kind];
  open?.dispatch({ changes: { from: 0, to: open.state.doc.length, insert: source } });
};
const states = new Map<string, ReturnType<typeof editorState>>();
const appliedSources = new Map<string, string>();
const projectHistory = new ProjectHistory(JSON.stringify(project));
let restoringHistory = false;
function undoProject(redo = false) {
  const raw = redo ? projectHistory.redo() : projectHistory.undo();
  if (raw === undefined) return;
  const restored: Project = JSON.parse(raw);
  const structure = (p: Project) => JSON.stringify([p.tracks.length, p.sequence, p.patterns.map(p => [p.id, p.rows])]);
  if (audioStatus.playing && structure(project) !== structure(restored)) void action(stopPlayback);
  receiveMidi({ kind: 'reset' });
  restoringHistory = true;
  const selectedId = selected.id;
  envelopeEditor?.reset();
  project = restored;
  deviceTrack = deviceTrack >= project.tracks.length || deviceTrack < -project.returns.length - 1 ? 0 : deviceTrack;
  chosenDevice.clear(); deletedTrack = null;
  selected = project.definitions.find(d => d.id === selectedId) ?? project.definitions[0];
  states.clear();
  for (const kind of ['instr', 'fx'] as const) {
    const was = shown[kind], now = was && (project.definitions.find(d => d.id === was.id) ?? (kind === 'instr' ? project.definitions.find(d => d.kind === 'instr') : undefined));
    shown[kind] = now; if (now) editors[kind]?.setState(documentState(now));
  }
  arrangement.position = Math.min(arrangement.position, project.sequence.length - 1);
  arrangement.render(); grid.resetHistory(); grid.render(); selectDefinition(selected, false); mixerView(); patternControls(); timingInputs();
  editedPattern(); restoringHistory = false;
}
window.addEventListener('keydown', event => {
  if (!(event.metaKey || event.ctrlKey) || event.altKey) return;
  const key = event.key.toLowerCase();
  if (key !== 'z' && key !== 'y') return;
  event.preventDefault(); event.stopImmediatePropagation(); undoProject(key === 'y' || event.shiftKey);
}, true);
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
  return editorState(documentUri(definition.id), definition.source, source => { definition.source = source; save(`source:${definition.id}`); renderApplyStatus(); sampleChanged(); }, () => { void action(() => applyAndAudition(definition)); });
}
function refreshProjectDir() {
  projectDirReady = isTauri() ? invoke<string>('project_dir', { id: project.id }).then(dir => {
    projectDir = dir;
    // Documents opened before the folder was known have the wrong address; reopen them.
    states.clear(); if (editor) editor.setState(documentState(selected));
    void refreshSamples();
  }, error => status(`Project folder unavailable: ${error}`)) : Promise.resolve();
}
/** An effect opened from the library to edit its code, apart from any track's chain. */
let libraryFx: string | null = null;
const chosenDevice = new Map<number, string>();
let deletedTrack: DeletedTrack | null = null;
let wasPlaying = false;
let envelopeEditor: EnvelopeEditor | undefined;
let connectionPending: 'connect' | 'disconnect' | null = null;
let connectionAttempt: Promise<void> | null = null;
let autoConnect = true;
let nextConnectionAttempt = Date.now() + 2500;

let gesture = 0;
let editGesture: string | undefined;
window.addEventListener('pointerdown', () => { editGesture = `gesture:${++gesture}`; }, true);
window.addEventListener('pointerup', () => { editGesture = undefined; });
window.addEventListener('pointercancel', () => { editGesture = undefined; });
function save(group?: string) {
  if (!restoringHistory) projectHistory.record(JSON.stringify(project), editGesture ?? group);
  try { localStorage.setItem(storageKey, JSON.stringify(project)); element('save-status').textContent = 'Saved locally'; }
  catch { element('save-status').textContent = 'Autosave failed · use Save project'; }
}

function selectDefinition(definition: Definition, reveal = true, library = false) {
  const kind = definition.kind, open = shown[kind], current = editors[kind];
  if (open && current) states.set(open.id, current.state);
  selected = definition; shown[kind] = definition;
  if (kind === 'instr') currentInstrument = definition.id;
  if (reveal) { if (kind === 'instr') showView('instrument'); else showPanel('code', true); }
  const state = states.get(definition.id) ?? documentState(definition);
  if (current) current.setState(state);
  else editors[kind] = new EditorView({ state, parent: element(parts[kind]) });
  editor = editors[kind]!;
  element<HTMLInputElement>(`${kind}-name`).value = definition.name;
  element(`${kind}-kind`).textContent = kind === 'instr' ? 'INSTRUMENT' : 'EFFECT';
  if (kind === 'fx') { libraryFx = library ? definition.id : null; element('mixer').hidden = false; element('code').hidden = false; element('fx-tools').hidden = element('code-panel').hidden; }
  renderDefinitions(); renderApplyStatus(); mixerView(); sampleChanged();
}
/** The browser's Project folder lists the definitions, so it follows them. */
function renderDefinitions() { soundBrowser.refresh(); projectPanel.refresh(); }
/** Effect and instrument parameters share one table: definition ids are unique across both. */
function allParameters() { return { ...audioStatus.instrument_parameters, ...audioStatus.effect_parameters }; }
function song(): Song {
  const instrument_values = Object.fromEntries(project.definitions.filter(d => d.kind === 'instr' && d.values).map(d => [d.id, d.values!]));
  return { bpm: project.bpm, rows_per_beat: project.rows_per_beat, swing: project.swing ?? 0, tracks: project.tracks, returns: project.returns, master: project.master, patterns: project.patterns, sequence: project.sequence, loop_range: project.loop_range, matrix: project.matrix, instrument_values };
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
function shownEffect() { return element('code').hidden ? null : shown.fx?.id ?? null; }
let fxCodeShown = false;
function mixerView() {
  element('bottom-dock').classList.toggle('show-code', fxCodeShown && !element('code').hidden || libraryFx !== null);
  refreshMix(); browserTarget(); soundBrowser.refresh(); fxBrowser.refresh(); projectPanel.refresh();
  element('dock-track').textContent = stripOf(project, deviceTrack).name; markTabs();
  if (shown.instr) renderInstrument(element('instr-controls'), shown.instr, allParameters(), editedPattern, (target, value) => grid.writeParameter(stripToColumn(project, deviceTrack), target, value), deviceTrack, changeSelectedSource);
  if (libraryFx !== null) { element('mixer').hidden = true; return; }
  element('mixer').hidden = false;
  envelopeEditor?.select(stripToColumn(project, deviceTrack));
  if (element('code-panel').hidden) return;
  if (shownEffect() !== (currentDevice()?.definition ?? null)) { showFx(); return; }
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
      const cleared = commandsFor(project, deviceTrack, id);
      removeDevice(project, deviceTrack, id);
      if (cleared) status(`Cleared ${cleared} effect command${cleared > 1 ? 's' : ''} written for it`);
      chosenDevice.delete(deviceTrack); showFx(); editedPattern(); grid.render();
    },
    rename: (id, name) => {
      const definition = project.definitions.find(d => d.id === stripOf(project, deviceTrack).fx.find(device => device.id === id)?.definition);
      if (!definition) return;
      const before = definition.source;
      renameDefinition(project, definition, name);
      if (definition.source !== before) { if (shown.fx === definition) replaceSource(definition, definition.source); else states.delete(definition.id); }
      renderDefinitions(); grid.render(); mixerView(); save();
    },
    move: (id, delta) => { if (moveDevice(project, deviceTrack, id, delta)) { editedPattern(); grid.render(); mixerView(); } },
    changed: () => { editedPattern(); grid.render(); mixerView(); },
    lanesRemoved: () => grid.resetHistory(),
  });
}
/** Show the selected track's chain: the controls beside the code of its selected effect. */
function showFx() {
  libraryFx = null;
  const device = currentDevice();
  const definition = device && project.definitions.find(d => d.id === device.definition);
  if (definition) selectDefinition(definition);
  else { showPanel('code', true); element('mixer').hidden = false; element('code').hidden = true; element('fx-tools').hidden = true; }
  mixerView();
}
/** Moving to another track shows that track's effects, unless the dock is on its automation or hidden. */
function followTrack() {
  if (!element('code-panel').hidden) showFx(); else mixerView();
}
function openTrackFx() { showFx(); if (!element('code').hidden) editor.focus(); }
async function stopPlayback() {
  receiveMidi({ kind: 'reset' });
  transportRevision++; starting = false;
  await audioCommand({ op: 'stop' });
}
async function togglePlay(mode: 'pattern' | 'song' = 'pattern', startRow = 0) {
  if (audioStatus.playing || starting) { await stopPlayback(); return; }
  const revision = ++transportRevision;
  starting = true;
  try {
    for (const id of usedInstruments()) await ensureSound(id);
    for (const track of [...project.tracks, ...project.returns, project.master]) {
      for (const device of track.fx) await ensureSound(device.definition);
      if (revision !== transportRevision) return;
    }
    await audioCommand({ op: 'play', song: song(), mode, pattern_id: arrangement.pattern.id, start_position: arrangement.position, start_row: startRow });
  } finally { if (revision === transportRevision) starting = false; }
}
/** Renders the whole song to a WAV file, and with `stems` a file for each sounding track beside it. */
async function exportAudio(stems: boolean) {
  if (!audioStatus.connected) { status('Connect to the engine to export'); return; }
  const path = await invoke<string | null>('export_path');
  if (!path) return;
  status('Exporting…');
  await action(async () => {
    await ensureAssignments();
    await audioCommand({ op: 'export', song: song(), path, stems });
    status(`Exported ${path}`);
  });
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
async function audition(id: string, note: string, track: number, ms = 300, expression: { commands?: string[] } = {}) {
  await ensureSound(id); await ensureAssignments();
  await audioCommand({ op: 'update', song: song() });
  await audioCommand({ op: 'audition', instrument: id, note, track, ms, ...expression });
}
/** Slots in the engine for what the sound browser is playing; they are not part of the project. */
const previewInstrument = 'preview_instr';
/** Plays instrument code on the selected track, without adding it to the project. */
async function previewSource(source: string, note = 'C-3', ms = 700) {
  const track = noteTrack();
  await projectDirReady;
  await ensureAssignments();
  await audioCommand({ op: 'apply', id: previewInstrument, source, kind: 'instr', dir: projectDir });
  await audioCommand({ op: 'update', song: song() });
  await audioCommand({ op: 'audition', instrument: previewInstrument, note, track, ms });
}
const previewPreset = (preset: Preset) => previewSource(presetSource(preset, 'browser_preview'));
/** The strip of the grid column the cursor is in. */
const activeStrip = () => columnToStrip(project, Math.min(grid.activeTrack, columnCount(project) - 1));
/** The track notes are auditioned on: the cursor's, or the last track when it is in a return or the master. */
const noteTrack = () => Math.min(grid.activeTrack, project.tracks.length - 1);
const arrangement = new Arrangement(element('sequence'), () => project, manual => {
  if (manual) element<HTMLInputElement>('follow').checked = false;
  grid.render(); deviceTrack = activeStrip(); mixerView(); patternControls();
}, editedPattern);
const grid = new PatternGrid(element('grid'), () => project, () => arrangement.pattern, () => Number(element<HTMLInputElement>('octave').value), () => { editedPattern(); mixerView(); },
  (id, note, track, expression) => {
    if (!audioStatus.connected) return;
    if (expression?.hold !== undefined) holdNote(expression.hold, id, note, track); else void action(() => audition(id, note, track, 300, expression));
  },
  fromCursor => { void action(() => togglePlay('pattern', fromCursor ? grid.cursorRow : 0)); },
  column => { const strip = columnToStrip(project, column); if (deviceTrack !== strip) { deviceTrack = strip; followTrack(); } },
  () => Number(element<HTMLInputElement>('entry-step').value), status, action => {
    const last = project.sequence.length - 1;
    if (action === 'previous') arrangement.select(Math.max(0, arrangement.position - 1));
    else if (action === 'next') arrangement.select(Math.min(last, arrangement.position + 1));
    else if (action === 'first') arrangement.select(0);
    else if (action === 'last') arrangement.select(last);
    else if (audioStatus.playing) return; // the sequence cannot change while it plays
    else if (action === 'new') arrangement.insert('new');
    else arrangement.remove();
  }, () => currentInstrument, () => { const text = element<HTMLInputElement>('velocity').value.trim(), n = Number(text); return text && Number.isFinite(n) ? Math.min(100, Math.max(0, Math.round(n))) : null; });
// Keep key-down compilation, playback and key-up in arrival order, even for a short tap.
let midiPending = Promise.resolve();
let midiRevision = 0;
const midiKeys = new Set<number>();
function midiAction(work: () => Promise<unknown>) { midiPending = midiPending.then(() => action(work)); }
/** A note of the computer keyboard sounds until its key comes up, in order with the MIDI keys it shares the engine's held notes with. */
function holdNote(key: number, instrument: string, note: string, track: number) {
  const revision = midiRevision;
  midiAction(async () => {
    if (!audioStatus.connected || revision !== midiRevision) return;
    await ensureSound(instrument); await ensureAssignments();
    if (revision !== midiRevision) return;
    await audioCommand({ op: 'update', song: song() });
    await audioCommand({ op: 'midi_on', key, instrument, note, track, velocity: 127 });
  });
}
function receiveMidi(message: MidiMessage) {
  if (message.kind === 'on') {
    const target = grid.midiNoteOn(message.note, message.velocity);
    if (!target?.instrument) return;
    const instrument = target.instrument, key = message.channel * 128 + message.note;
    midiKeys.add(key);
    const revision = midiRevision;
    midiAction(async () => {
      if (!audioStatus.connected || revision !== midiRevision) return;
      await ensureSound(instrument); await ensureAssignments();
      if (revision !== midiRevision) return;
      await audioCommand({ op: 'update', song: song() });
      if (revision !== midiRevision) return;
      await audioCommand({ op: 'midi_on', key, instrument, note: target.note, track: target.track, velocity: target.velocity });
    });
  } else if (message.kind === 'reset') {
    midiRevision++; midiKeys.clear();
    midiAction(async () => { if (audioStatus.connected) await audioCommand({ op: 'midi_off', key: null }); });
  } else {
    const keys = message.kind === 'off' ? [message.channel * 128 + message.note] : [...midiKeys].filter(key => Math.floor(key / 128) === message.channel);
    for (const key of keys) {
      midiKeys.delete(key);
      midiAction(async () => { if (audioStatus.connected) await audioCommand({ op: 'midi_off', key }); });
    }
  }
}
grid.releaseHeld = key => midiAction(async () => { if (audioStatus.connected) await audioCommand({ op: 'midi_off', key }); });
void mountMidi(element('midi-controls'), receiveMidi, status).catch(error => status(`MIDI: ${error}`));
const presetItem = (p: Preset): BrowserItem => ({ value: p.id, name: p.name, folder: p.folder, description: p.description });
/** Loads a ready-made instrument into an existing slot: its name and code are replaced, and the slot keeps its number. */
function loadPresetIntoSlot(definition: Definition, preset: Preset) {
  definition.name = preset.name;
  definition.source = presetSource(preset, freeCodeName(project, slug(preset.name), definition));
  delete definition.values;
  states.delete(definition.id);
  if (shown[definition.kind] === definition) replaceSource(definition, definition.source);
  else selectDefinition(definition, false);
  renderDefinitions(); grid.render(); mixerView(); save();
  applyNewInstrument(definition);
}
/** Replaces the code of the instrument being edited, as typing would, and applies it so the change is heard. */
function changeSelectedSource(source: string) {
  const instrument = shown.instr!;
  replaceSource(instrument, source);
  if (audioStatus.connected) void action(() => applyAndAudition(instrument));
  else mixerView();
}
/** New instruments are ready to play even when the engine still needs to start. */
function applyNewInstrument(definition: Definition) {
  void action(async () => {
    await ensureConnected();
    if (appliedSources.get(definition.id) !== definition.source) await applyDefinition(definition);
    await audition(definition.id, 'C-3', Math.max(0, deviceTrack));
  });
}
/** Makes an instrument from a preset and chooses it, so the notes entered next use it. */
function useInstrumentPreset(preset: Preset) {
  const definition = addInstrumentFromPreset(project, preset);
  selectDefinition(definition, false); grid.render(); mixerView(); save();
  applyNewInstrument(definition);
}
/** Adds an effect to the end of the selected strip's chain and shows its code: a preset's id, or `fx:<id>` for one already in the project. */
function addEffectToStrip(choice: string) {
  const added = addDevice(project, deviceTrack, choice);
  if (!added) { status('This strip has no room for another effect'); return; }
  chosenDevice.set(deviceTrack, added.device.id); showFx(); editedPattern(); grid.render();
  if (audioStatus.connected) void action(() => ensureSound(added.definition.id));
}
/** The project's samples (paths relative to its folder), as the Samples tab lists them. */
let samples: string[] = [];
async function refreshSamples() {
  if (!isTauri()) return;
  try { samples = await invoke<string[]>('list_samples', { id: project.id }); } catch { samples = []; }
  soundBrowser.refresh();
}
/** What each sample looks like and how long it plays, read once. */
const looks = new Map<string, Promise<SamplePeaks | null>>();
function samplePeaks(file: string): Promise<SamplePeaks | null> {
  const key = `${project.id}/${file}`;
  if (!looks.has(key)) {
    looks.set(key, !isTauri() ? Promise.resolve(null) : invoke<{ frames: number; rate: number; peaks: number[] }>('sample_peaks', { id: project.id, file, buckets: 160 })
      .then(({ frames, rate, peaks }) => ({ peaks, seconds: frames / rate }), () => null));
  }
  return looks.get(key)!;
}
/** What the browser lists: the library only. The project's own instruments have their panel above, and its effects are on the tracks. The samples are the project's own, since that is the pool to choose them from: loose ones at the top, kits as folders. */
function browserItems(kind: SoundKind): BrowserItem[] {
  if (kind === 'sample') return samples.map(file => {
    const parts = file.replace(/^samples\//, '').split('/'), name = parts.pop()!.replace(/\.[^.]+$/, '');
    return { value: `sample:${file}`, name, folder: parts.join('/'), description: file };
  });
  return kind === 'instr' ? instrumentPresets.map(presetItem) : effectChoices(project, deviceTrack);
}
const sampleOf = (item: { value: string }) => item.value.slice('sample:'.length);
/** Makes a sampler for a sample of the project. */
async function newSampler(file: string) {
  newInstrument(file, (await samplePeaks(file))?.seconds ?? 0);
}
/** Puts a sample in a slot: a sampler plays it instead of its old one, any other instrument becomes a sampler of it. */
async function loadSampleIntoSlot(definition: Definition, file: string) {
  const seconds = (await samplePeaks(file))?.seconds ?? 0, swapped = withSample(definition.source, file);
  if (!swapped) definition.name = file.split('/').pop()!.replace(/\.[^.]+$/, '');
  setDefinitionSource(definition, swapped ?? samplerSource(freeCodeName(project, slug(definition.name), definition), file, seconds));
  delete definition.values;
  renderDefinitions(); grid.render(); mixerView(); save();
  applyNewInstrument(definition);
}
const browserActions: SoundBrowserActions = {
  audition: (kind, item) => {
    if (!audioStatus.connected) return;
    if (kind === 'sample') { void action(() => previewSource(samplerSource('browser_preview', sampleOf(item)), 'C-4', 2000)); return; }
    if (kind === 'fx') return; // effects are not previewed
    const preset = instrumentPresets.find(p => p.id === item.value);
    if (preset) void action(() => previewPreset(preset)); // a project instrument or a blank one has nothing to preview
  },
  pick: (kind, item, replace) => {
    if (kind === 'fx') { addEffectToStrip(item.value); return; }
    // an instrument becomes the current one, and the cursor goes back to the grid to enter notes with it
    const slot = project.definitions.find(d => d.id === currentInstrument);
    if (kind === 'sample') { if (replace && slot) void loadSampleIntoSlot(slot, sampleOf(item)); else void newSampler(sampleOf(item)); element('grid').focus(); return; }
    const preset = instrumentPresets.find(p => p.id === item.value)!;
    if (replace && slot) loadPresetIntoSlot(slot, preset);
    else useInstrumentPreset(preset);
    element('grid').focus();
  },
  menu: (kind, item, anchor) => {
    const replacing = project.definitions.find(d => d.id === currentInstrument)?.name ?? 'the current instrument';
    const use = (replace: boolean) => () => browserActions.pick(kind, item, replace);
    openMenu(anchor, kind === 'fx' ? [{ text: 'Add to the selected track', run: use(false) }]
      : [{ text: kind === 'sample' ? 'New sampler' : 'Add as a new instrument', run: use(false) }, { text: `Put in "${replacing}"`, run: use(true) }]);
  },
  manage: op => { if (op === 'kit') void importKit(); else void importSamples(); },
  peaks: item => item.value.startsWith('sample:') ? samplePeaks(sampleOf(item)) : Promise.resolve(null),
  leave: () => {},
};
const soundBrowser = new SoundBrowser(element('sound-browser'), browserItems, browserActions, () => element('grid').focus());
/** The effects list at the bottom left of the dock, under the sequence bar, which adds to the selected track's chain. */
const fxBrowser = new SoundBrowser(element('fx-browser'), browserItems, browserActions, () => element('grid').focus(), 'fx');
/** The browser is as wide as the sequence bar above it, whatever that bar's width works out to (its minimum depends on the track count and on hidden names). */
new ResizeObserver(() => { const width = element('sequencer').offsetWidth; if (width) element('fx-browser').style.flexBasis = `${width}px`; }).observe(element('sequencer'));
const projectInstruments = () => project.definitions.filter(d => d.kind === 'instr');
/** Makes an instrument the current one and shows its code. */
function chooseInstrument(definition: Definition) {
  selectDefinition(definition, false);
}
/** Asks whether a cut instrument may play its whole sample as one, which drops its pieces and makes the notes written for them play the whole sample. */
const askToPlayAsOne = (definition: Definition) => isTauri()
  ? invoke<boolean>('confirm', { message: `Play "${definition.name}" as one sample? Its pieces go, and notes written for them (C-4, C#4, …) will each play the whole sample from its start.` })
  : Promise.resolve(true);
/** What can be done to an instrument or effect of the project. */
function definitionMenu(definition: Definition): MenuEntry[] {
  const library = definition.kind === 'fx';
  return [
    { text: 'Edit code', run: () => { selectDefinition(definition, true, library); editor.focus(); } },
    { text: 'Duplicate', run: () => duplicateSound(definition) },
    { text: 'Delete', run: () => deleteSound(definition) },
  ];
}
const projectPanel = new ProjectPanel(element('project-panel'),
  () => projectInstruments().map(d => { const users = instrumentUsers(project, d.id).length; const how = readPlayback(d.source); return { id: d.id, name: d.name, note: users ? `×${users}` : undefined, tag: how ? { text: playbackNames[how].toLowerCase(), title: playbackHints[how] } : undefined }; }),
  () => currentInstrument, {
    choose: id => { const definition = project.definitions.find(d => d.id === id); if (definition) chooseInstrument(definition); },
    edit: id => { const definition = project.definitions.find(d => d.id === id); if (definition) { selectDefinition(definition); editor.focus(); } },
    add: anchor => openMenu(anchor, [
      { text: 'Blank instrument', run: () => newInstrument() },
      { text: 'Sampler from audio files…', run: () => void importSamples() },
      { text: 'Kit from audio files…', run: () => void importKit() },
      { text: 'Browse the instrument library', run: () => toggleBrowser('instr') },
    ]),
    menu: (id, anchor) => { const definition = project.definitions.find(d => d.id === id); if (definition) openMenu(anchor, definitionMenu(definition)); },
    remove: id => { const definition = project.definitions.find(d => d.id === id); if (definition) deleteSound(definition); },
    drop: (item, id) => dropOnSlot(item, id),
    leave: () => element('grid').focus(),
  });
/** A row of the browser dropped on an instrument of the project (which it replaces) or on the empty space of the list (which adds it). */
function dropOnSlot(item: Dragged, id: string | null) {
  const slot = id ? project.definitions.find(d => d.id === id) : undefined;
  if (item.kind === 'sample') { const file = sampleOf(item); if (slot) { selectDefinition(slot, false); void loadSampleIntoSlot(slot, file); } else void newSampler(file); return; }
  const preset = instrumentPresets.find(p => p.id === item.value);
  if (!preset) return;
  if (slot) { selectDefinition(slot, false); loadPresetIntoSlot(slot, preset); } else useInstrumentPreset(preset);
}
/** Puts new code in an instrument, in the editor if it is the one open, and applies it so the change is heard. */
function setDefinitionSource(definition: Definition, source: string) {
  if (shown[definition.kind] === definition) replaceSource(definition, source);
  else { definition.source = source; states.delete(definition.id); save(); }
}
const samplePanel = new SamplePanel(element('sample'), {
  current: () => project.definitions.find(d => d.id === currentInstrument && d.kind === 'instr') ?? null,
  loadPeaks: file => invoke('sample_peaks', { id: project.id, file, buckets: 2000 }),
  edit: (definition, source) => { setDefinitionSource(definition, source); if (audioStatus.connected) void action(() => applyDefinition(definition)); },
  hear: (definition, piece) => { if (audioStatus.connected) void action(() => audition(definition.id, pieceKey(piece), noteTrack())); },
  detect: (file, max = maxSlices, sensitivity = 0.5) => invoke('detect_slices', { id: project.id, file, max, sensitivity }),
  cut: (_, pieces) => sliceCurrent(pieces),
  tempo: () => Number(element<HTMLInputElement>('bpm').value) || 120,
  uncut: definition => askToPlayAsOne(definition),
});
let sampleTimer = 0;
/** Draws the Sample tab again shortly after the instrument or its code changes, if it is showing. */
function sampleChanged() {
  if (element('sample-panel').hidden) return;
  clearTimeout(sampleTimer); sampleTimer = window.setTimeout(() => void samplePanel.refresh(), 250);
}

/** A new instrument that plays the current sampler in pieces, one per key from C-4: a count of equal pieces, or the shares of the sample where each starts. */
function sliceCurrent(pieces: number | number[]) {
  const source = project.definitions.find(d => d.id === currentInstrument);
  const definition = source && sliceInstrument(project, source, pieces);
  if (!definition) { status(`Choose a sampler first: cutting needs an instrument that plays a sample, into 2 to ${maxSlices} pieces`); return; }
  selectDefinition(definition, false); grid.render(); mixerView(); save();
  applyNewInstrument(definition);
  status(`${definition.name}: C-4 plays the first of ${typeof pieces === 'number' ? pieces : pieces.length} pieces`);
}
/** Option+Up and Option+Down: the previous or next instrument of the project, from anywhere but a text box. */
function stepInstrument(delta: number) {
  const id = neighbour(projectInstruments().map(d => d.id), currentInstrument, delta), next = projectInstruments().find(d => d.id === id);
  if (next) chooseInstrument(next);
}
document.addEventListener('keydown', event => {
  if (!event.altKey || event.metaKey || event.ctrlKey || event.shiftKey || (event.key !== 'ArrowUp' && event.key !== 'ArrowDown')) return;
  if ((event.target as HTMLElement).closest('input, select, textarea, .cm-editor')) return;
  event.preventDefault(); event.stopPropagation(); stepInstrument(event.key === 'ArrowDown' ? 1 : -1);
}, true);
// Notes sound from anywhere but a text box, the code editor or the grid (which handles its own keys).
document.addEventListener('keydown', event => {
  if ((event.target as HTMLElement).closest('input, select, textarea, .cm-editor, #grid, [contenteditable], .modal-backdrop')) return;
  grid.auditionKey(event);
});
/** Shows or hides the browser; showing it puts the cursor in its search box. */
function toggleBrowser(kind?: SoundKind) {
  const section = element('sound-browser');
  if (!view.side) changeView({ side: true });
  if (!section.hidden && section.contains(document.activeElement) && !kind) { section.hidden = true; element('grid').focus(); return; }
  section.hidden = false;
  if (kind) soundBrowser.show(kind); else soundBrowser.focus();
}
function browserTarget() {
  const instruments = project.definitions.filter(d => d.kind === 'instr'), at = instruments.findIndex(d => d.id === currentInstrument), strip = stripOf(project, deviceTrack);
  soundBrowser.setTarget(`Notes use ${at < 0 ? 'no instrument' : `${String(at).padStart(2, '0')} ${instruments[at].name}`} · effects go on ${strip.name} · Shift+Enter or Shift+double-click replaces the current instrument`);
}
envelopeEditor = new EnvelopeEditor(element('automation'), () => project, () => arrangement.pattern, () => allParameters(),
  () => { editedPattern(); grid.render(); });
const mix = new MixPanel(element('mix'), () => project, {
  openFx: (track, device) => { if (track < 0) deviceTrack = track; else grid.selectTrack(track); if (device) chosenDevice.set(deviceTrack, device); showFx(); },
  edited: () => { editedPattern(); grid.render(); },
  addReturn: () => {
    if (project.returns.length >= maxReturns) return;
    project.returns.push({ name: `Return ${String.fromCharCode(65 + project.returns.length)}`, fx: [], pan: 0, muted: false, volume_db: 0 });
    for (const pattern of project.patterns) { pattern.return_fx.push([emptyFxLane()]); pattern.commands.splice(project.returns.length - 1, 0, [Array(pattern.rows).fill(null)]); }
    editedPattern(); grid.render(); mixerView();
  },
  removeReturn: index => {
    project.returns.splice(index, 1);
    for (const pattern of project.patterns) { pattern.return_fx.splice(index, 1); pattern.commands.splice(index, 1); }
    for (const track of [...project.tracks, ...project.returns]) {
      // A strip that fed the removed return goes back to the master; later returns shift down.
      if (track.output?.kind === 'return') track.output = track.output.index === index ? { kind: 'master' } : track.output.index > index ? { kind: 'return', index: track.output.index - 1 } : track.output;
      for (const device of track.fx) if (device.target !== undefined && device.target > index) device.target--;
    }
    if (deviceTrack < -1) { deviceTrack = activeStrip(); if (!element('code-panel').hidden) showFx(); }
    editedPattern(); grid.render(); mixerView();
  },
});
/** What the pattern editor shows for a track whose notes in the selected step come from somewhere else. */
grid.slot = track => {
  const source = trackSource(project, arrangement.position, track);
  if (!source) return null;
  return source.kind === 'mute' ? { kind: 'mute', text: 'Muted in this step: its notes here are not played (see the cells on its sequence step)' } : { kind: 'alias', text: `Plays pattern ${project.patterns.findIndex(p => p.id === source.id)} in this step: edits here are not heard (see the cells on its sequence step)` };
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
let quietPolls = 0, ticks = 0;
async function pollScopes() {
  // after two silent seconds, look five times a second instead of twenty; sound brings it back
  if (quietPolls > 40 && ++ticks % 4 !== 0) return;
  if (isTauri() && audioStatus.connected) { try { scopes = await invoke<Record<string, number[]>>('audio_scopes'); } catch { scopes = {}; } }
  else scopes = {};
  quietPolls = Object.values(scopes).every(frames => frames.every(v => Math.abs(v) < 0.001)) ? quietPolls + 1 : 0;
  drawScope(element<HTMLCanvasElement>('master-scope'), scopes.s_m);
  if (!element('mix-panel').hidden) mix.paint(scopes);
}
const scopeTimer = window.setInterval(() => { void pollScopes(); }, 50);
function newLibraryEffect(preset = 'blank') {
  const definition = newEffect(project, preset)!;
  selectDefinition(definition, true, true); editor.focus(); save();
}
// The browser's own right-click menu (reload, inspect…) is not part of the app: only text fields keep theirs, for cut, copy and paste.
document.addEventListener('contextmenu', event => { if (!(event.target as HTMLElement).closest?.('input, textarea, .cm-editor')) event.preventDefault(); });
grid.trackMenu = () => {
  // the toolbar buttons already know when each is allowed, so the menu runs them
  const entry = (text: string, id: string): MenuEntry[] => element<HTMLButtonElement>(id).disabled ? [] : [{ text, run: () => element(id).click() }];
  return [{ heading: 'Track' }, ...entry('Add track', 'add-track'), ...entry('Duplicate', 'duplicate-track'), ...entry('Delete', 'delete-track'), ...entry('Restore deleted track', 'restore-track')];
};
grid.follows = () => element<HTMLInputElement>('follow').checked;
grid.modeChanged = on => { element('grid-wrap').classList.toggle('recording', on); element('record').setAttribute('aria-pressed', String(on)); };
element('record').onclick = () => { grid.setRecording(!grid.recording); element('grid').focus(); };
// Toggle even when editors or popups consume Escape, while preserving their close/cancel action.
window.addEventListener('keydown', event => {
  if (event.key !== 'Escape' || (event.target as HTMLElement).closest?.('.modal-backdrop')) return;
  event.preventDefault();
  if (!event.repeat) grid.setRecording(!grid.recording);
  // the cursor goes back to the grid, once a popup that took the Escape has put focus back on its button; the code editor keeps its own
  if (!(event.target as HTMLElement).closest?.('.cm-editor')) setTimeout(() => element('grid').focus());
}, true);
// As in Renoise, whatever you click or choose leaves the cursor in the grid, so the keys sound notes and enter them at once: a button, a tick box or
// slider, a choice made in a drop-down, and Enter in a text or number box. Not where the keyboard is wanted: the code editor and its bar, the grid's own
// cell editor, the sound browser's search, and pop-ups (menus, the palette). These look at the target before its own handler can redraw it.
const keepsKeys = '.cm-editor, .modal-backdrop, .palette, .menu, .browser, #grid, #sound-browser';
const keepInGrid = (target: Element) => setTimeout(() => { const active = document.activeElement; if (!active || active === document.body || active === target) element('grid').focus(); });
document.addEventListener('click', event => {
  const target = event.target as HTMLElement;
  const control = target.closest?.('button, input[type=checkbox]');
  if (control && !control.closest(`${keepsKeys}, .code-bar`)) keepInGrid(control);
}, true);
let pickedSelect: Element | null = null, slid: Element | null = null, lastKey = '';
document.addEventListener('pointerdown', event => {
  const target = event.target as HTMLElement;
  pickedSelect = target instanceof HTMLSelectElement ? target : target.closest?.('select') ?? null;
  slid = target instanceof HTMLInputElement && target.type === 'range' ? target : null;
}, true);
document.addEventListener('pointerup', () => { if (slid && !slid.closest(keepsKeys)) keepInGrid(slid); slid = null; }, true);
// a choice, as opposed to arrowing through the options of a closed drop-down, which also fires change: picked with the pointer, or confirmed with Enter or Space
document.addEventListener('keydown', event => { lastKey = event.key; }, true);
document.addEventListener('change', event => {
  const select = event.target;
  if (select instanceof HTMLSelectElement && !select.closest(keepsKeys) && (select === pickedSelect || lastKey === 'Enter' || lastKey === ' ')) keepInGrid(select);
  pickedSelect = null;
}, true);
// Enter in a text or number box commits it (as leaving it does) and gives the keyboard back
document.addEventListener('keydown', event => {
  const box = event.target;
  if (event.key !== 'Enter' || !(box instanceof HTMLInputElement) || !['text', 'number', 'search', 'password'].includes(box.type) || box.closest(keepsKeys)) return;
  setTimeout(() => { if (document.activeElement === box) { box.blur(); element('grid').focus(); } });
}, true);
grid.projectUndo = undoProject;
element('undo').onclick = () => undoProject();
element('redo').onclick = () => undoProject(true);
function renderApplyStatus() {
  for (const kind of ['instr', 'fx'] as const) {
    const definition = shown[kind]; if (!definition) continue;
    const ready = (kind === 'fx' ? audioStatus.effects[definition.id] : audioStatus.instruments[definition.id]) !== undefined, applied = appliedSources.get(definition.id) === definition.source;
    element<HTMLButtonElement>(`${kind}-apply`).disabled = !audioStatus.connected;
    element(`${kind}-apply`).title = audioStatus.connected ? 'Apply the code and play a note through it (Cmd/Ctrl+Enter in the editor)' : 'Connect the engine to apply';
    element(`${kind}-apply`).classList.toggle('dirty', audioStatus.connected && (!ready || !applied));
    setText(`${kind}-status`, !ready ? 'Not applied' : applied ? 'Applied' : 'Unapplied edits · playing last applied version');
  }
}
function renderConnectButton() {
  setText('connect', connectionPending === 'connect' ? 'Connecting…' : connectionPending === 'disconnect' ? 'Disconnecting…' : audioStatus.connected ? 'Disconnect' : 'Connect');
  element<HTMLButtonElement>('connect').disabled = connectionPending !== null;
  element<HTMLInputElement>('engine-port').disabled = connectionPending !== null || audioStatus.connected;
  element<HTMLInputElement>('engine-token').disabled = connectionPending !== null || audioStatus.connected;
}
/** Like Stage, connect before an action and share an attempt already in progress. */
async function ensureConnected() {
  autoConnect = true;
  if (connectionAttempt) return connectionAttempt;
  if (audioStatus.connected) return;
  connectionPending = 'connect';
  renderConnectButton();
  setText('audio-status', 'Connecting…');
  connectionAttempt = (async () => {
    transportRevision++; starting = false;
    appliedSources.clear();
    const port = Number(element<HTMLInputElement>('engine-port').value);
    if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('Enter an engine port between 1 and 65535');
    try { await audioCommand({ op: 'connect', port, token: element<HTMLInputElement>('engine-token').value.trim() || null }); }
    catch (error) {
      // the engine is the user's to start; this keeps looking for one
      if (String(error).includes("can't reach the server")) throw new Error(`No engine on port ${port}: run \`niminal server\` and the tracker connects to it`);
      throw error;
    }
    // everything on screen, then every instrument the song plays and every effect on its strips
    for (const definition of new Set([selected, shown.instr, shown.fx])) if (definition) await applyDefinition(definition);
    await ensureAssignments();
  })();
  try { await connectionAttempt; }
  finally {
    connectionAttempt = null; connectionPending = null;
    nextConnectionAttempt = Date.now() + 2500;
    renderConnectButton();
  }
}
/** An explicit disconnect stays offline until Connect or an action asks for the engine. */
function toggleConnection() {
  if (connectionPending) return;
  if (!audioStatus.connected) { void action(ensureConnected); return; }
  autoConnect = false;
  connectionPending = 'disconnect';
  renderConnectButton();
  void action(async () => {
    transportRevision++; starting = false;
    appliedSources.clear();
    await audioCommand({ op: 'disconnect' });
  }).finally(() => { connectionPending = null; renderConnectButton(); });
}
element('connect').onclick = toggleConnection;
// transport buttons hand focus straight back to the grid, so notes can be entered as soon as it plays
element('play').onclick = () => { element('grid').focus(); void action(() => togglePlay('pattern')); };
element('play-song').onclick = () => { element('grid').focus(); void action(() => togglePlay('song')); };
element('stop').onclick = () => { element('grid').focus(); void action(stopPlayback); };
for (const kind of ['instr', 'fx'] as const) element(`${kind}-apply`).onclick = () => { const definition = shown[kind]; if (definition) void action(() => applyAndAudition(definition)); };
function timingInputs() {
  renderTiming();
  element<HTMLInputElement>('bpm').value = String(project.bpm);
  element<HTMLInputElement>('rows-per-beat').value = String(project.rows_per_beat);
  element<HTMLInputElement>('swing').value = String(project.swing ?? 0);
}
for (const [id, field, min, max] of [['bpm', 'bpm', 20, 300], ['rows-per-beat', 'rows_per_beat', 1, 16]] as const) {
  element<HTMLInputElement>(id).onchange = event => {
    const input = event.target as HTMLInputElement;
    project[field] = Math.min(max, Math.max(min, Math.trunc(Number(input.value) || min)));
    if (field === 'rows_per_beat') grid.render(); // the beat and bar shading follows it
    timingInputs(); editedPattern();
  };
}
element<HTMLInputElement>('swing').onchange = event => {
  const swing = Math.min(75, Math.max(0, Math.round(Number((event.target as HTMLInputElement).value) || 0)));
  if (swing) project.swing = swing; else delete project.swing;
  timingInputs(); grid.render(); editedPattern();
};
element<HTMLInputElement>('entry-step').onchange = event => {
  const input = event.target as HTMLInputElement; input.value = String(Math.max(0, Math.min(256, Math.trunc(Number(input.value) || 0))));
};
element('loop').onclick = () => { arrangement.toggleLoop(); patternControls(); };
element('sequence-cells').onclick = () => changeView({ matrix: !view.matrix });
element<HTMLInputElement>('octave').onchange = event => {
  const input = event.target as HTMLInputElement;
  input.value = String(Math.min(8, Math.max(0, Math.trunc(Number(input.value) || 0))));
};
/** Renames the definition shown, and the name in its code with it. */
for (const kind of ['instr', 'fx'] as const) {
  element<HTMLInputElement>(`${kind}-name`).onchange = event => {
    const input = event.target as HTMLInputElement, definition = shown[kind]!, before = definition.source;
    renameDefinition(project, definition, input.value);
    input.value = definition.name;
    if (definition.source !== before) replaceSource(definition, definition.source);
    renderDefinitions(); grid.render(); mixerView(); save();
  };
}
function newInstrument(sample?: string, seconds = 0) {
  const definition = addInstrument(project, sample, seconds);
  selectDefinition(definition); editor.focus(); grid.render(); mixerView(); save();
  applyNewInstrument(definition);
  return definition;
}
/** Adds samplers for copied-in audio files, and lists them in the browser. */
function addImported(files: { file: string; seconds: number }[]) {
  for (const { file, seconds } of files) newInstrument(file, seconds);
  if (files.length) status(`Added ${files.length} sampler${files.length > 1 ? 's' : ''}`);
  void refreshSamples();
}
/** Adds an instrument that plays a copied-in kit folder, and lists its samples in the browser. */
function addImportedKit(kit: { folder: string; members: string[] } | null) {
  void refreshSamples();
  if (!kit) return;
  const definition = addKit(project, kit.folder, kit.members);
  if (!definition) { status(`A kit holds 1 to ${maxSlices} samples, one on each key from C-4`); return; }
  selectDefinition(definition); editor.focus(); grid.render(); mixerView(); save();
  applyNewInstrument(definition);
  status(`${definition.name}: ${kit.members.length} samples, C-4 plays ${kit.members[0]}`);
}
async function importSamples() {
  if (!isTauri()) { status('Importing samples needs the desktop app'); return; }
  try { addImported(await invoke<{ file: string; seconds: number }[]>('import_samples', { id: project.id })); }
  catch (error) { status(`Could not import samples: ${error}`); }
}
/** Several audio files as one instrument with a sample on each key from C-4. */
async function importKit() {
  if (!isTauri()) { status('Importing samples needs the desktop app'); return; }
  try { addImportedKit(await invoke<{ folder: string; members: string[] } | null>('import_kit', { id: project.id })); }
  catch (error) { status(`Could not import the kit: ${error}`); }
}
/** Audio files dropped on the window from outside: one is a sampler, several are asked about. */
async function importDropped(paths: string[]) {
  try {
    if (paths.length === 1) { addImported(await invoke<{ file: string; seconds: number }[]>('import_dropped', { id: project.id, paths })); return; }
    openMenu(element('right-bar'), [
      { text: `${paths.length} samplers, one for each file`, run: () => void invoke<{ file: string; seconds: number }[]>('import_dropped', { id: project.id, paths }).then(addImported, error => status(`Could not import samples: ${error}`)) },
      { text: 'One kit, a sample on each key', run: () => void invoke<{ folder: string; members: string[] } | null>('import_dropped_kit', { id: project.id, paths }).then(addImportedKit, error => status(`Could not import the kit: ${error}`)) },
    ]);
  } catch (error) { status(`Could not import samples: ${error}`); }
}
if (isTauri()) void getCurrentWebview().onDragDropEvent(event => {
  const bar = element('right-bar'), kind = event.payload.type;
  bar.classList.toggle('file-drop', kind === 'enter' || kind === 'over');
  if (kind === 'drop' && event.payload.paths.length) void importDropped(event.payload.paths);
});
function duplicateSound(definition: Definition) {
  const library = definition.kind === 'fx';
  const copy = library ? duplicateEffect(project, definition) : duplicateInstrument(project, definition);
  selectDefinition(copy, true, library); editor.focus(); save();
  if (!library) applyNewInstrument(copy);
}
function deleteSound(definition: Definition) {
  const problem = definition.kind === 'fx' ? removeEffect(project, definition.id) : removeInstrument(project, definition.id);
  if (problem) { status(problem); return; }
  states.delete(definition.id);
  selectDefinition(project.definitions.find(d => d.kind === 'instr')!, false); grid.render(); save();
}
element('expand').onclick = () => {
  showPanel('code', true);
  const expanded = element('app').classList.toggle('editor-expanded');
  setText('expand', expanded ? '⤡' : '⤢'); measure();
};
document.addEventListener('keydown', event => {
  if (event.key === 'F6') { event.preventDefault(); if (codeFocused()) { showView('pattern'); element('grid').focus(); } else { showView('instrument'); editors.instr?.focus(); } }
});
async function loadProject(raw: string) {
  const loaded = readProject(raw); dropUnusedPatterns(loaded);
  receiveMidi({ kind: 'reset' });
  transportRevision++; starting = false;
  if (audioStatus.connected) await audioCommand({ op: 'disconnect' });
  appliedSources.clear();
  for (const kind of ['instr', 'fx'] as const) { editors[kind]?.destroy(); delete editors[kind]; delete shown[kind]; }
  states.clear();
  envelopeEditor?.reset();
  projectHistory.reset(JSON.stringify(loaded));
  project = loaded; deletedTrack = null; deviceTrack = 0; chosenDevice.clear(); libraryFx = null; refreshProjectDir(); selected = project.definitions.find(d => d.kind === 'instr') ?? project.definitions[0];
  selectDefinition(selected, false); arrangement.reset(); grid.resetHistory(); grid.selectTrack(0); mixerView(); patternControls(); timingInputs(); save();
  if (isTauri()) void action(ensureConnected);
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
type Panel = 'code' | 'automation';
const panels: Panel[] = ['code', 'automation'];
const tabPanel = (tab: string): Panel => tab === 'fx' ? 'code' : tab as Panel;
function markTabs() {
  for (const button of document.querySelectorAll<HTMLElement>('[data-tab]')) button.setAttribute('aria-selected', String(!element(`${tabPanel(button.dataset.tab!)}-panel`).hidden));
  // the effects tab and the Code tab share the panel: one or the other is showing
  const code = element('bottom-dock').classList.contains('show-code'), inPanel = !element('code-panel').hidden;
  document.querySelector('[data-tab=fx]')!.setAttribute('aria-selected', String(inPanel && !code)); element('fx-code-toggle').setAttribute('aria-selected', String(inPanel && code));
}
/** The dock: the selected track's effects and automation. */
function showPanel(panel: Panel, visible: boolean) {
  for (const candidate of panels) {
    const section = element(`${candidate}-panel`);
    if (visible || candidate === panel) section.hidden = !visible || candidate !== panel;
  }
  markTabs();
  element('fx-tools').hidden = element('code-panel').hidden || element('code').hidden;
  element('bottom-dock').hidden = panels.every(candidate => element(`${candidate}-panel`).hidden);
  element('toggle-dock').setAttribute('aria-pressed', String(!element('bottom-dock').hidden));
  if (element('code-panel').hidden) {
    element('app').classList.remove('editor-expanded'); setText('expand', '⤢');
  }
  measure();
}
for (const button of document.querySelectorAll<HTMLElement>('[data-tab]')) {
  const tab = button.dataset.tab!;
  button.onclick = () => {
    fxCodeShown = false;
    showPanel(tabPanel(tab), true);
    if (tab === 'automation') envelopeEditor?.select(stripToColumn(project, deviceTrack));
    else if (tab === 'fx') showFx();
  };
}
/** The main area: the pattern editor, or one of the views that take its place. */
type View = 'pattern' | 'instrument' | 'sample' | 'mix';
const viewSections = { instrument: 'instrument-view', sample: 'sample-panel', mix: 'mix-panel' };
let mainView: View = 'pattern';
function showView(next: View) {
  mainView = next;
  element('grid-wrap').hidden = next !== 'pattern';
  for (const [view, id] of Object.entries(viewSections)) element(id).hidden = view !== next;
  for (const button of document.querySelectorAll<HTMLElement>('[data-view]')) button.setAttribute('aria-selected', String(button.dataset.view === next));
  element('open-mix').setAttribute('aria-pressed', String(next === 'mix'));
  element('app').classList.remove('editor-expanded'); setText('expand', '⤢');
  if (next === 'mix') refreshMix();
  watchStrips();
  if (next === 'sample') void samplePanel.refresh();
  measure();
}
for (const button of document.querySelectorAll<HTMLElement>('[data-view]')) {
  button.onclick = () => { showView(button.dataset.view as View); if (button.dataset.view === 'pattern') element('grid').focus(); };
}
element('open-mix').onclick = () => showView(mainView === 'mix' ? 'pattern' : 'mix');
element('fx-code-toggle').onclick = () => { fxCodeShown = true; showPanel('code', true); showFx(); if (!element('code').hidden) editor.focus(); };
element('toggle-dock').onclick = () => { if (element('bottom-dock').hidden) { showPanel('code', true); showFx(); } else showPanel('code', false); };
element('close-dock').onclick = () => { showPanel('code', false); element('grid').focus(); };
element('dock-resize').onpointerdown = event => {
  event.preventDefault();
  const dock = element('bottom-dock'), startY = event.clientY, startHeight = dock.offsetHeight;
  const target = event.currentTarget as HTMLElement; target.setPointerCapture(event.pointerId);
  target.onpointermove = move => { dock.style.height = `${Math.max(120, startHeight + startY - move.clientY)}px`; measure(); };
  target.onpointerup = () => { target.onpointermove = null; target.onpointerup = null; };
};
const timingControls = document.createElement('div');
timingControls.className = 'group';
timingControls.innerHTML = `<label>Pattern BPM <input id="pattern-bpm" aria-label="Pattern BPM" type="number" min="20" max="300" step="any"></label><label>LPB <input id="pattern-lpb" aria-label="Pattern rows per quarter note" type="number" min="1" max="16"></label><label>Meter <input id="meter-beats" aria-label="Beats per bar" type="number" min="1" max="32"> / <select id="meter-unit" aria-label="Meter beat unit"><option>1</option><option>2</option><option>4</option><option>8</option><option>16</option></select></label><label>Swing <input id="pattern-swing" aria-label="Pattern swing" type="number" min="0" max="75" title="Swing for this pattern, 0 to 75 percent; blank follows the song. 0 is straight even when the song swings"></label><label>Tempo points <input id="tempo-points" aria-label="Tempo automation points" placeholder="0:125, 32:80" title="Row:BPM points, separated by commas. Linear ramps between points; BPM counts quarter notes."></label><button id="reset-timing">Use song timing</button>`;
element('pattern-rows').parentElement!.parentElement!.append(timingControls);
function renderTiming() {
  const t = patternTiming(project, arrangement.pattern);
  element<HTMLInputElement>('pattern-bpm').value = String(t.bpm);
  element<HTMLInputElement>('pattern-lpb').value = String(t.rows_per_beat);
  element<HTMLInputElement>('meter-beats').value = String(t.beats_per_bar);
  element<HTMLSelectElement>('meter-unit').value = String(t.beat_unit);
  element<HTMLInputElement>('pattern-swing').value = arrangement.pattern.swing === undefined ? '' : String(arrangement.pattern.swing);
  element<HTMLInputElement>('pattern-swing').placeholder = String(project.swing ?? 0);
  element<HTMLInputElement>('tempo-points').value = t.tempo.map(p => `${p.row}:${p.value}`).join(', ');
}
for (const id of ['pattern-bpm', 'pattern-lpb', 'meter-beats', 'meter-unit', 'tempo-points']) element(id).onchange = () => {
  const points = element<HTMLInputElement>('tempo-points').value.trim();
  const timing = { bpm: Number(element<HTMLInputElement>('pattern-bpm').value), rows_per_beat: Number(element<HTMLInputElement>('pattern-lpb').value), beats_per_bar: Number(element<HTMLInputElement>('meter-beats').value), beat_unit: Number(element<HTMLSelectElement>('meter-unit').value), tempo: points ? points.split(',').map(text => { const parts = text.trim().split(':'); return { row: parts.length === 2 && parts[0].trim() ? Number(parts[0]) : NaN, value: parts[1]?.trim() ? Number(parts[1]) : NaN }; }) : [] };
  if (!validTiming(timing, arrangement.pattern.rows)) { status('Use ascending Row:BPM points within the pattern, BPM 20–300, LPB 1–16 and meter 1–32.'); renderTiming(); return; }
  arrangement.pattern.timing = timing; grid.render(); editedPattern();
};
element<HTMLInputElement>('pattern-swing').onchange = event => {
  const text = (event.target as HTMLInputElement).value.trim();
  if (text === '') delete arrangement.pattern.swing; else arrangement.pattern.swing = Math.min(75, Math.max(0, Math.round(Number(text) || 0)));
  renderTiming(); grid.render(); editedPattern();
};
element('reset-timing').onclick = () => { delete arrangement.pattern.timing; delete arrangement.pattern.swing; renderTiming(); grid.render(); editedPattern(); };
function patternControls() {
  renderTiming();
  const current = arrangement.pattern;
  element('loop').setAttribute('aria-pressed', String(!!project.loop_range));
  element<HTMLInputElement>('pattern-rows').value = String(current.rows);
}
element<HTMLInputElement>('pattern-rows').onchange = event => {
  if (audioStatus.playing) { patternControls(); return; }
  resizePattern(arrangement.pattern, Number((event.target as HTMLInputElement).value));
  grid.render(); patternControls(); editedPattern();
};
element('sequence-add').onclick = event => arrangement.addMenu(event.currentTarget as HTMLElement);
element('add-track').onclick = () => {
  if (audioStatus.playing || project.tracks.length >= 16) return;
  project.tracks.push({ name: `Track ${String(project.tracks.length + 1).padStart(2, '0')}`, fx: [], pan: 0, muted: false, volume_db: 0 });
  for (const pattern of project.patterns) { const rows = pattern.notes[0][0].notes.length; pattern.notes.push([blankColumn(rows)]); pattern.fx.push([emptyFxLane()]); }
  grid.selectTrack(project.tracks.length - 1); arrangement.render(); editedPattern();
};
element('duplicate-track').onclick = () => { void action(async () => {
  if (audioStatus.playing || project.tracks.length >= 16) return;
  if (audioStatus.connected) await stopPlayback();
  const index = grid.activeTrack;
  if (index >= project.tracks.length) return;
  const copy = duplicateTrack(project, index);
  if (copy === null) return;
  envelopeEditor?.reset(); grid.resetHistory(); grid.selectTrack(copy); mixerView(); arrangement.render(); editedPattern();
}); };
element('delete-track').onclick = () => { void action(async () => {
  if (audioStatus.playing || project.tracks.length <= 1) return;
  if (audioStatus.connected) await stopPlayback();
  const index = grid.activeTrack;
  if (index >= project.tracks.length) return; // the cursor is in a return or the master, which cannot be deleted here
  envelopeEditor?.reset();
  deletedTrack = removeTrack(project, index);
  grid.resetHistory(); grid.selectTrack(Math.min(index, project.tracks.length - 1)); mixerView(); arrangement.render(); editedPattern();
}); };
grid.onMoveTrack = (from, to) => { void action(async () => {
  if (audioStatus.playing) return;
  envelopeEditor?.reset();
  if (!moveTrack(project, from, to)) return;
  deletedTrack = null; chosenDevice.clear(); grid.resetHistory(); grid.selectTrack(to); mixerView(); arrangement.render(); editedPattern();
}); };
element('restore-track').onclick = () => {
  if (audioStatus.playing || !deletedTrack || project.tracks.length >= 16) return;
  const index = deletedTrack.index;
  envelopeEditor?.reset();
  if (!restoreTrack(project, deletedTrack)) return;
  deletedTrack = null; grid.resetHistory(); grid.selectTrack(index); mixerView(); arrangement.render(); editedPattern();
};
refreshProjectDir();
if (envelopeEditor) envelopeEditor.projectHistory = { undo: undoProject, canUndo: () => projectHistory.canUndo, canRedo: () => projectHistory.canRedo };
selectDefinition(selected, false); arrangement.reset(); grid.render(); mixerView(); patternControls(); timingInputs();
showView('pattern'); showPanel('code', true); showFx();
const audioTimer = observeAudio(state => {
  if (state.playing !== wasPlaying) { wasPlaying = state.playing; mixerView(); }
  // Replacing a button's text node during a mouse press can cancel its click
  // in WebKit. Only change labels when the displayed state actually changes.
  renderConnectButton();
  if (isTauri() && autoConnect && !state.connected && !connectionPending && Date.now() >= nextConnectionAttempt) {
    void action(ensureConnected);
  }
  element<HTMLButtonElement>('play').disabled = !state.connected || state.playing;
  element<HTMLButtonElement>('stop').disabled = !state.connected;
  element<HTMLButtonElement>('play-song').disabled = !state.connected || state.playing;
  for (const input of document.querySelectorAll<HTMLInputElement | HTMLButtonElement>('[data-structure]')) {
    if (input.closest('#mixer')) continue;
    input.disabled = state.playing || (input.id === 'add-track' || input.id === 'duplicate-track') && project.tracks.length >= 16 || input.id === 'delete-track' && project.tracks.length <= 1 || input.id === 'restore-track' && (!deletedTrack || project.tracks.length >= 16);
  }
  arrangement.paint(state.sequence_position, state.playing);
  element('sequencer').querySelector<HTMLElement>('.sequence-lock')!.hidden = !state.playing;
  if (state.playing && element<HTMLInputElement>('follow').checked && state.pattern_id && state.sequence_position !== null && (arrangement.pattern.id !== state.pattern_id || arrangement.position !== state.sequence_position)) {
    if (state.sequence_position !== null) arrangement.select(state.sequence_position, false);
  }
  setText('play-position', state.playing && state.pattern_id ? `${state.sequence_position === null ? 'Loop' : `Step ${state.sequence_position + 1}`} · row ${state.row ?? '…'}` : '');
  element<HTMLInputElement>('bpm').disabled = false;
  element<HTMLInputElement>('rows-per-beat').disabled = false;
  setText('audio-status', audioError ?? state.error ?? (state.connected ? `${state.playing ? 'Playing' : 'Connected'} · ${state.sample_rate / 1000} kHz${state.skipped_rows ? ` · ${state.skipped_rows} skipped rows` : ''}${state.late_events ? ` · ${state.late_events} late events` : ''}` : 'Engine not connected'));
  envelopeEditor?.paint(state.pattern_id === arrangement.pattern.id ? state.row : null);
  grid.paint(state.pattern_id === arrangement.pattern.id ? state.row : null, state.row_phase, state.row_at_ms); renderApplyStatus();
});
void connectLanguageServer(status);


// Interface size, code font and side bars: kept between sessions
let view = loadView(), focusMode = false;
function renderView() {
  applyView(view, focusMode);
  setText('zoom-level', `${Math.round(view.scale * 100)}%`);
  element('toggle-sequence').setAttribute('aria-pressed', String(view.sequence));
  element('sequence-cells').setAttribute('aria-pressed', String(view.matrix));
  if (arrangement) arrangement.setCells(view.matrix);
  element('toggle-side').setAttribute('aria-pressed', String(view.side));
  measure();
}
function changeView(change: Partial<ViewPrefs>, message?: string) {
  view = { ...view, ...change }; saveView(view); renderView();
  if (message) status(message);
}
function zoom(delta: number) { const scale = delta === 0 ? 1 : stepScale(view.scale, delta); changeView({ scale }, `Interface ${Math.round(scale * 100)}%`); }
function codeFont(delta: number) { const px = delta === 0 ? defaultView.codeFont : clampFont(view.codeFont + delta); changeView({ codeFont: px }, `Code font ${px}px`); }
/** Cmd/Ctrl +, − and 0 size the code in the editor and the whole interface anywhere else. */
function zoomHere(delta: number) { if (codeFocused()) codeFont(delta); else zoom(delta); }
function toggleSequence() { changeView({ sequence: !view.sequence }); if (!view.sequence) element('grid').focus(); }
function toggleSide() { changeView({ side: !view.side }); if (!view.side) element('grid').focus(); }
function toggleFocusMode() { focusMode = !focusMode; renderView(); element('grid').focus(); }
/** Ctrl+1 to 5: the grid, sequence, instruments, sound browser and code, showing what is hidden. */
function focusRegion(n: number) {
  if (focusMode) { focusMode = false; renderView(); }
  if (n === 1) element('grid').focus();
  else if (n === 2) { if (!view.sequence) changeView({ sequence: true }); (document.querySelector<HTMLElement>('#sequence .selected > button') ?? element('sequence').querySelector<HTMLElement>('button'))?.focus(); }
  else if (n === 3) { if (!view.side) changeView({ side: true }); projectPanel.focus(); }
  else if (n === 4) { if (!view.side) changeView({ side: true }); element('sound-browser').hidden = false; soundBrowser.focus(); }
  else if (n === 5) {
    showView('instrument'); editors.instr?.focus();
  }
}
dragWidth(element('left-resize'), 1, () => view.sequenceWidth, width => { view.sequenceWidth = width; document.documentElement.style.setProperty('--sequence-width', `${width}px`); }, () => saveView(view));
dragWidth(element('right-resize'), -1, () => view.sideWidth, width => { view.sideWidth = width; document.documentElement.style.setProperty('--side-width', `${width}px`); }, () => saveView(view));
element('left-resize').ondblclick = () => changeView({ sequenceWidth: defaultView.sequenceWidth });
element('right-resize').ondblclick = () => changeView({ sideWidth: defaultView.sideWidth });
element('toggle-sequence').onclick = toggleSequence;
element('toggle-side').onclick = toggleSide;
element('zoom-in').onclick = () => zoom(1);
element('zoom-out').onclick = () => zoom(-1);
element('zoom-level').onclick = () => zoom(0);
element('open-palette').onclick = () => menuActions.palette();
renderView();

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
  'import-kit': () => void importKit(),
  'export-wav': () => void exportAudio(false),
  'export-stems': () => void exportAudio(true),
  undo: () => undoProject(),
  redo: () => undoProject(true),
  'play-pattern': () => element('play').click(),
  'play-song': () => element('play-song').click(),
  stop: () => element('stop').click(),
  'add-track': () => element('add-track').click(),
  'duplicate-track': () => element('duplicate-track').click(),
  'select-track': () => { element('grid').focus(); grid.selectWholeTrack(); },
  'cut-track': () => { element('grid').focus(); grid.cutTrack(); },
  'reverse-selection': () => { element('grid').focus(); grid.reverseSelection(); },
  'randomize-selection': () => { element('grid').focus(); grid.randomizeSelection(); },
  'new-pattern': () => { if (!audioStatus.playing) arrangement.insert('new'); },
  reconnect: () => { void action(async () => { if (audioStatus.connected) await audioCommand({ op: 'disconnect' }); }).then(toggleConnection); },
  'new-instrument': () => newInstrument(),
  'new-effect': () => newLibraryEffect(),
  apply: () => element(`${selected.kind}-apply`).click(),
  code: () => showView('instrument'),
  mixer: () => element('open-mix').click(),
  sample: () => showView('sample'),
  browser: () => toggleBrowser(),
  record: () => element('record').click(),
  automation: () => showPanel('automation', true),
  'zoom-in': () => zoomHere(1),
  'zoom-out': () => zoomHere(-1),
  'zoom-reset': () => zoomHere(0),
  'code-font-larger': () => codeFont(1),
  'code-font-smaller': () => codeFont(-1),
  'code-font-reset': () => codeFont(0),
  'toggle-sequence': toggleSequence,
  'toggle-side': toggleSide,
  'toggle-dock': () => element('toggle-dock').click(),
  'focus-mode': toggleFocusMode,
  contrast: () => changeView({ contrast: !view.contrast }, view.contrast ? 'High contrast' : 'Normal contrast'),
  shortcuts: () => openShortcuts(shortcutSections),
  palette: () => openPalette(commands),
};
/** Everything the palette lists: the menu's actions by name, and the key that also runs each. */
const commands: Command[] = ([
  ['Play pattern', 'play-pattern', 'Space'], ['Play song', 'play-song', 'F5'], ['Stop', 'stop', 'F8'], ['Record mode on or off', 'record', 'Esc'],
  ['Undo', 'undo', 'Cmd/Ctrl+Z'], ['Redo', 'redo', 'Cmd/Ctrl+Shift+Z'],
  ['Add track', 'add-track'], ['Duplicate track', 'duplicate-track'], ['Select track', 'select-track', 'Cmd/Ctrl+Shift+A'], ['Cut track', 'cut-track'],
  ['Reverse selection', 'reverse-selection'], ['Randomize selection', 'randomize-selection'], ['New pattern', 'new-pattern'],
  ['New instrument', 'new-instrument'], ['New effect', 'new-effect'], ['Apply and audition', 'apply', 'Cmd/Ctrl+Enter'],
  ['Show code', 'code', 'F6'], ['Show mixer', 'mixer'], ['Show sample', 'sample'], ['Show automation', 'automation'],
  ['Sound browser', 'browser', 'Cmd/Ctrl+B'],
  ['Show or hide the sequence bar', 'toggle-sequence', 'Cmd/Ctrl+\\'], ['Show or hide the right bar', 'toggle-side', 'Cmd/Ctrl+Shift+B'], ['Show or hide the bottom dock (track FX and automation)', 'toggle-dock', 'Cmd/Ctrl+J'], ['Focus mode: only the grid', 'focus-mode', 'Cmd/Ctrl+Shift+F'],
  ['Interface larger', 'zoom-in', 'Cmd/Ctrl+='], ['Interface smaller', 'zoom-out', 'Cmd/Ctrl+−'], ['Interface actual size', 'zoom-reset', 'Cmd/Ctrl+0'],
  ['Code font larger', 'code-font-larger'], ['Code font smaller', 'code-font-smaller'], ['Code font actual size', 'code-font-reset'],
  ['High contrast on or off', 'contrast'],
  ['New project', 'new'], ['Open project', 'open', 'Cmd/Ctrl+O'], ['Save project', 'save', 'Cmd/Ctrl+S'],
  ['Import samples', 'import-samples'], ['Import samples as a kit', 'import-kit'], ['Export WAV', 'export-wav'], ['Export WAV and track stems', 'export-stems'],
  ['Restart engine connection', 'reconnect'], ['Keyboard shortcuts', 'shortcuts', 'Cmd/Ctrl+/'],
] as [string, string, string?][]).map(([label, id, keys]) => ({ label, keys, run: () => menuActions[id]() }));
// Keys that work wherever focus is. Zoom is the menu's in the app, where the menu takes the keys first.
window.addEventListener('keydown', event => {
  if (event.isComposing) return;
  const mod = event.metaKey || event.ctrlKey, plain = !event.altKey;
  let action: string | undefined;
  if (mod && plain && !event.shiftKey && event.code === 'KeyK' || mod && plain && event.shiftKey && event.code === 'KeyP') action = 'palette';
  else if (event.key === 'F1' || mod && plain && !event.shiftKey && event.code === 'Slash') action = 'shortcuts';
  else if (mod && plain && !event.shiftKey && event.code === 'Backslash') action = 'toggle-sequence';
  else if (mod && plain && event.shiftKey && event.code === 'KeyB') action = 'toggle-side';
  else if (mod && plain && !event.shiftKey && event.code === 'KeyJ') action = 'toggle-dock';
  else if (mod && plain && event.shiftKey && event.code === 'KeyF') action = 'focus-mode';
  else if (!mod && !event.altKey && !event.shiftKey && event.key === 'F5') action = 'play-song';
  else if (!mod && !event.altKey && !event.shiftKey && event.key === 'F8') action = 'stop';
  else if (!isTauri() && mod && plain && (event.key === '=' || event.key === '+')) action = 'zoom-in';
  else if (!isTauri() && mod && plain && event.key === '-') action = 'zoom-out';
  else if (!isTauri() && mod && plain && event.key === '0') action = 'zoom-reset';
  else if (event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey && /^Digit[1-5]$/.test(event.code)) {
    event.preventDefault(); event.stopPropagation(); focusRegion(Number(event.code.slice(5))); return;
  }
  if (!action) return;
  event.preventDefault(); event.stopPropagation(); menuActions[action]();
}, true);
if (isTauri()) {
  listen<string>('menu', event => menuActions[event.payload]?.()).catch(error => status(`The menu is not connected: ${error}`));
  // Connect to an engine that is running; starting one is up to the user.
  void action(ensureConnected);
}
window.addEventListener('beforeunload', () => { clearInterval(audioTimer); clearInterval(scopeTimer); for (const each of Object.values(editors)) each.destroy(); });

// Enter in a toolbar or timing field commits the value and hands the keyboard back to the grid
for (const panel of [element('toolbar'), timingControls]) panel.addEventListener('keydown', event => {
  const input = event.target as HTMLElement;
  if (event.key !== 'Enter' || !(input instanceof HTMLInputElement)) return;
  event.preventDefault(); input.blur(); element('grid').focus();
});
