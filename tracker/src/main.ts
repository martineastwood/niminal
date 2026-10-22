import { invoke, isTauri } from '@tauri-apps/api/core';
import { EditorView } from '@codemirror/view';
import { editorState, connectLanguageServer } from './editor';
import { newProject, readProject, keyboardNote, storageKey, type Definition } from './project';
import './style.css';

const app = document.querySelector<HTMLDivElement>('#app')!;
app.innerHTML = `
  <header><div class="brand">niminal <span>tracker</span></div><span class="badge">EDITOR PROTOTYPE</span><div class="project-actions"><button id="import">Open project</button><button id="export">Save project</button></div></header>
  <main>
    <aside><div class="section-title">SOUND LIBRARY</div><div id="definitions"></div><div class="add-buttons"><button id="new-instrument">+ Instrument</button><button id="new-effect">+ Effect</button></div><p class="aside-help">Select a sound to edit its code.<br>Project copies are saved locally.</p></aside>
    <section class="workspace">
      <div class="pattern-toolbar"><div><strong>Pattern 00</strong><span class="muted">32 rows · 4 tracks</span></div><label>Octave <input id="octave" type="number" min="0" max="8" value="4"></label><span class="muted">Playback coming next</span></div>
      <div id="grid" tabindex="0" role="grid" aria-label="Pattern editor"></div>
      <div class="grid-help">Z S X D C V G B H N J M: notes · Arrows: move · Delete: clear · F6: switch editor</div>
      <section class="instrument-editor"><div class="editor-toolbar"><div><span id="definition-kind" class="badge"></span><input id="definition-name" aria-label="Definition name"></div><button id="expand">Expand editor</button></div><div id="code"></div><div class="editor-help">Hover for documentation or errors · Ctrl+Space: completion · Shift+Alt+F: format</div></section>
    </section>
  </main><footer><span id="status">Starting editor…</span><span id="save-status">Local autosave</span></footer>`;

const element = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const status = (text: string) => { element('status').textContent = text; };
let project = newProject();
try { const raw = localStorage.getItem(storageKey); if (raw) project = readProject(raw); }
catch (error) { status(`Could not restore project: ${error}`); }
let selected = project.definitions[0];
let row = 0, track = 0;
let editor: EditorView;
const states = new Map<string, ReturnType<typeof editorState>>();

function save() {
  try { localStorage.setItem(storageKey, JSON.stringify(project)); element('save-status').textContent = 'Saved locally'; }
  catch { element('save-status').textContent = 'Autosave failed · use Save project'; }
}

function selectDefinition(definition: Definition) {
  if (editor) states.set(selected.id, editor.state);
  selected = definition;
  const state = states.get(definition.id) ?? editorState(`file:///niminal-tracker/${definition.id}.nml`, definition.source, source => { definition.source = source; save(); });
  if (editor) editor.setState(state);
  else editor = new EditorView({ state, parent: element('code') });
  element<HTMLInputElement>('definition-name').value = definition.name;
  element('definition-kind').textContent = definition.kind === 'instr' ? 'INSTRUMENT' : 'EFFECT';
  renderDefinitions();
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
function renderGrid() {
  const grid = element('grid');
  grid.innerHTML = '<div class="grid-row grid-header"><span>ROW</span><span>01 / LEAD</span><span>02 / BASS</span><span>03 / DRUMS</span><span>04 / TEXTURE</span></div>';
  for (let r = 0; r < 32; r++) {
    const line = document.createElement('div'); line.className = `grid-row ${r % 4 === 0 ? 'beat' : ''}`; line.setAttribute('role', 'row');
    const number = document.createElement('span'); number.className = 'row-number'; number.textContent = String(r).padStart(2, '0'); line.append(number);
    for (let t = 0; t < 4; t++) {
      const cell = document.createElement('div'); cell.setAttribute('role', 'gridcell');
      cell.className = `note ${project.tracks[t][r] ? 'filled' : ''} ${r === row && t === track ? 'cursor' : ''}`;
      cell.id = `cell-${r}-${t}`; cell.setAttribute('aria-selected', String(r === row && t === track));
      cell.textContent = project.tracks[t][r] ?? '···';
      cell.onclick = () => { row = r; track = t; renderGrid(); grid.focus(); };
      line.append(cell);
    }
    grid.append(line);
  }
  grid.setAttribute('aria-activedescendant', `cell-${row}-${track}`);
  element(`cell-${row}-${track}`).scrollIntoView({ block: 'nearest' });
}
element('grid').addEventListener('keydown', event => {
  if (event.ctrlKey || event.metaKey || event.altKey) return;
  let edit = false;
  switch (event.key) {
    case 'ArrowUp': row = (row + 31) % 32; break;
    case 'ArrowDown': row = (row + 1) % 32; break;
    case 'ArrowLeft': track = (track + 3) % 4; break;
    case 'ArrowRight': track = (track + 1) % 4; break;
    case 'Delete': case 'Backspace': project.tracks[track][row] = null; edit = true; break;
    default: {
      const note = keyboardNote(event.key, Number(element<HTMLInputElement>('octave').value));
      if (!note) return;
      project.tracks[track][row] = note; row = (row + 1) % 32; edit = true;
    }
  }
  event.preventDefault(); renderGrid(); if (edit) save();
});
element<HTMLInputElement>('octave').onchange = event => {
  const input = event.target as HTMLInputElement;
  input.value = String(Math.min(8, Math.max(0, Math.trunc(Number(input.value) || 0))));
};
element<HTMLInputElement>('definition-name').oninput = event => { selected.name = (event.target as HTMLInputElement).value; renderDefinitions(); save(); };
function addDefinition(kind: 'instr' | 'fx') {
  const id = `${kind}_${crypto.randomUUID().replaceAll('-', '_')}`;
  const name = `${kind === 'instr' ? 'voice' : 'effect'}${project.definitions.length + 1}`;
  const definition: Definition = { id, kind, name, source: kind === 'instr' ? `/// Describe your instrument here.\ninstr ${name}(freq: hz) {\n  osc(sine, freq) * env[0.15 | 200ms 0]\n}\n` : `/// Describe your effect here.\nfx ${name} {\n  out = it.lpf(cutoff: 2400hz)\n}\n` };
  project.definitions.push(definition); selectDefinition(definition); editor.focus(); save();
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
function loadProject(raw: string) {
  const loaded = readProject(raw);
  editor.destroy(); states.clear();
  project = loaded; selected = project.definitions[0];
  const definition = selected;
  editor = new EditorView({ state: editorState(`file:///niminal-tracker/${definition.id}.nml`, definition.source, source => { definition.source = source; save(); }), parent: element('code') });
  selectDefinition(selected); renderGrid(); save();
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
    try { const raw = await invoke<string | null>('open_project'); if (raw !== null) loadProject(raw); }
    catch (error) { status(`Could not open project: ${error}`); }
    return;
  }
  const input = document.createElement('input'); input.type = 'file'; input.accept = '.json';
  input.onchange = async () => {
    const file = input.files?.[0]; if (!file) return;
    try { loadProject(await file.text()); }
    catch (error) { status(`Could not open project: ${error}`); }
  };
  input.click();
};
selectDefinition(selected); renderGrid();
void connectLanguageServer(status);
window.addEventListener('beforeunload', () => { editor.destroy(); });
