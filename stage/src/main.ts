import { EditorState } from '@codemirror/state';
import { redo, undo } from '@codemirror/commands';
import { EditorView } from '@codemirror/view';
import { isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { demoSource } from './demo';
import { Scope, connectLanguageServer, niminalEditor } from './editor';
import { blockAround } from '../../editors/codemirror/blocks';
import { LaneView } from './laneview';
import { Highlighter, evaluated, flashLines, markProblems } from './highlight';
import { mod } from './shortcuts';
import { sceneCommand, sceneStates } from './scenes';
import { Action, EvalOutcome, Problem, Scene, Snapshot, stage } from './live';
import { StoredTabs, loadTabs, moveItem, tabTitle, tabsKey } from './tabs';
import './style.css';

const app = document.querySelector<HTMLDivElement>('#app')!;
app.innerHTML = `
  <header id="topbar">
    <div class="brand">
      <span class="mark">◈</span>
      <div class="brand-text">
        <h1>niminal <em>stage</em></h1>
        <span id="connection" class="connection offline"><i></i><span>offline</span></span>
      </div>
    </div>
    <div class="transport" id="transport">
      <div class="position"><span class="eyebrow">bar</span><strong id="bar">1</strong><span class="beat" id="beat">1.1</span></div>
      <div class="readout"><span class="eyebrow">tempo</span><span id="bpm">—</span><small>bpm</small></div>
      <div class="readout"><span class="eyebrow">meter</span><span id="meter">4/4</span></div>
      <div class="readout"><span class="eyebrow">voices</span><span id="voices">0</span></div>
      <div class="readout"><span class="eyebrow">waiting</span><span id="waiting">0</span></div>
    </div>
    <div class="top-actions">
      <span class="setup">
        <label class="port">port <input id="port" inputmode="numeric" value="7401" /></label>
        <input id="token" type="password" placeholder="token" aria-label="engine token" />
        <button id="reconnect" title="Connect to the engine">Connect</button>
      </span>
      <span class="zoom" title="Text size: Cmd/Ctrl + and −, 0 to reset, or pinch the trackpad">
        <button id="zoom-out" aria-label="Smaller text">A−</button>
        <button id="zoom-reset" aria-label="Reset text size">100%</button>
        <button id="zoom-in" aria-label="Larger text">A+</button>
      </span>
      <button id="rail-toggle" title="Show or hide the sidebar (Cmd/Ctrl+B)">Sidebar</button>
      <button id="perform" title="Performance mode: just the code and what is playing (Cmd/Ctrl+Shift+P)">Perform</button>
      <button id="hush" title="Stop every clip, letting what sounds ring out">Hush</button>
      <button id="panic" class="danger" title="Stop everything now">Panic</button>
    </div>
  </header>

  <main>
    <section id="backstage">
      <div class="pane-head">
        <span class="pane-title">code</span>
        <span id="eval-state" class="eval-state"></span>
        <span id="landing" class="landing" aria-live="polite"></span>
        <div class="editor-actions">
          <span class="file-actions">
            <button id="new" title="A new empty tab (Cmd/Ctrl+T)">New</button>
            <button id="open" title="Open a .nml file in a tab">Open</button>
            <button id="open-project" title="Open a folder: main.nml and what it uses, or every .nml file, as tabs">Project</button>
            <button id="save" title="Save the file">Save</button>
            <button id="demo" title="Load the stage's opening piece">Demo</button>
          </span>
          <label class="quantize" title="When an evaluation lands">land <input id="quantize" placeholder="next 4bars" /></label>
          <button id="evaluate-all" title="Evaluate every tab, left to right" hidden>All</button>
          <button id="evaluate-file" title="Evaluate the whole tab (Cmd/Ctrl+Shift+Enter)">File <kbd>⇧⌘↩</kbd></button>
          <button id="evaluate" class="primary" title="Evaluate the selection, or the block the cursor is in (Cmd/Ctrl+Enter)">Evaluate <kbd>⌘↩</kbd></button>
        </div>
      </div>
      <div id="tabs" class="tabs" role="tablist"></div>
      <div id="banner" role="alert" hidden></div>
      <div id="editor"></div>
      <div id="problems" aria-live="polite"></div>
      <div id="editor-status"></div>
    </section>

    <div id="divider" role="separator" aria-orientation="vertical" aria-label="Resize the sidebar" tabindex="0" title="Drag to resize · double-click to reset"></div>

    <section id="stage">
      <div class="stage-head">
        <div class="scenes" id="scenes"></div>
        <div class="stage-hint" id="stage-hint"></div>
      </div>
      <div class="stage-scroll"><div id="lanes"></div></div>
      <div id="pending" class="pending"></div>
      <div id="notices" class="notices"></div>
    </section>
  </main>

  <footer>
    <span id="footer-status">starting…</span>
    <span class="spacer"></span>
    <span class="hint">${mod}↩ run block · ⇧${mod}↩ run file · ${mod}1…9 scenes · ${mod}. hush</span>
  </footer>
`;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const input = (id: string) => $<HTMLInputElement>(id);
const connection = $('connection');
const bar = $('bar');
const beat = $('beat');
const bpm = $('bpm');
const meter = $('meter');
const voices = $('voices');
const waiting = $('waiting');
const scenes = $('scenes');
const pending = $('pending');
const notices = $('notices');
const problems = $('problems');
const editorStatus = $('editor-status');
const footerStatus = $('footer-status');
const stageHint = $('stage-hint');

const portKey = 'niminal-stage:port';

let view: EditorView | null = null;
// numbered by the clock, so what the engine remembers from an earlier run never matches a new one
let evaluations = Date.now();
/** A file in the window: each has its own editor state, so its cursor, undo history and highlights are its own. */
type Tab = { id: number; path: string | null; title: string; state: EditorState; saved: string };
let tabs: Tab[] = [];
let current: Tab;
let tabCount = 0;
let connected = false;
let connectedOnce = false;
let lastSceneSignature = '';
let lastClipCount = -1;
let noticeLog: string[] = [];

/** The code an evaluation sends and where it begins in the editor: the selection, else the block the cursor is in. */
function chunk(scope: Scope) {
  if (!view) return { text: demoSource, from: 0, to: 0 };
  const doc = view.state.doc;
  if (scope === 'file') return { text: doc.toString(), from: 0, to: doc.length };
  const selection = view.state.selection.main;
  if (!selection.empty) {
    return { text: view.state.sliceDoc(selection.from, selection.to), from: selection.from, to: selection.to };
  }
  const lines = doc.toString().split('\n');
  const block = blockAround(lines, doc.lineAt(selection.head).number - 1);
  if (!block) return null;
  const from = doc.line(block.start + 1).from;
  const to = doc.line(block.end + 1).to;
  return { text: view.state.sliceDoc(from, to), from, to };
}

function uriFor(path: string) {
  return `file://${path
    .split('/')
    .map(encodeURIComponent)
    .join('/')}`;
}

function status(message: string) {
  footerStatus.textContent = message;
}

function setConnection(ok: boolean) {
  connected = ok;
  connection.classList.toggle('offline', !ok);
  connection.classList.toggle('online', ok);
  connection.querySelector('span')!.textContent = ok ? 'engine connected' : 'offline';
}

function makeTab(source: string, path: string | null, title: string, saved = source): Tab {
  const id = ++tabCount;
  const uri = uriFor(path ?? `/niminal-stage/${title}-${id}.nml`);
  const state = niminalEditor(source, uri, scope => void evaluate(scope), () => changed(id));
  return { id, path, title: tabTitle(path, title), state, saved };
}

/** The directory `use` and samples are found from: the folder of the file in the current tab. */
function fileDir() {
  return current.path ? current.path.replace(/[\\/][^\\/]*$/, '') : null;
}

/** Keep the current tab's state with the tab, as the editor is about to show another. */
function sync() {
  if (view) current.state = view.state;
}

function show(tab: Tab) {
  sync();
  current = tab;
  if (view) view.setState(tab.state);
  else view = new EditorView({ state: tab.state, parent: $('editor') });
  highlighter.reset();
  showProblems([]);
  renderTabs();
  persist();
  view.focus();
}

function addTab(source: string, path: string | null, title: string) {
  const tab = makeTab(source, path, title);
  tabs.push(tab);
  show(tab);
}

function closeTab(tab: Tab) {
  const index = tabs.indexOf(tab);
  if (isDirty(tab) && !confirm(`${tab.title} has changes that are not saved. Close it anyway?`)) return;
  tabs.splice(index, 1);
  if (tabs.length === 0) tabs.push(makeTab('', null, 'untitled'));
  if (tab === current) show(tabs[Math.max(0, index - 1)]);
  else {
    renderTabs();
    persist();
  }
}

function step(direction: 1 | -1) {
  if (tabs.length > 1) show(tabs[(tabs.indexOf(current) + direction + tabs.length) % tabs.length]);
}

function isDirty(tab: Tab) {
  return (tab === current && view ? view.state.doc.toString() : tab.state.doc.toString()) !== tab.saved;
}

let dragging: Tab | null = null;

function droppedBefore(event: DragEvent, item: HTMLElement) {
  const box = item.getBoundingClientRect();
  return event.clientX < box.left + box.width / 2;
}

function clearDropMarks() {
  for (const item of $('tabs').children) item.classList.remove('drop-before', 'drop-after');
}

function renderTabs() {
  $('evaluate-all').hidden = tabs.length < 2;
  $('tabs').replaceChildren(
    ...tabs.map(tab => {
      const item = document.createElement('div');
      item.className = 'tab';
      item.dataset.id = String(tab.id);
      item.setAttribute('role', 'tab');
      item.classList.toggle('active', tab === current);
      item.classList.toggle('dirty', isDirty(tab));
      item.title = tab.path ?? 'not saved to a file yet';
      const name = document.createElement('span');
      name.className = 'tab-name';
      name.textContent = tab.title;
      const close = document.createElement('button');
      close.className = 'tab-close';
      close.textContent = '×';
      close.title = 'Close this tab';
      close.addEventListener('click', event => {
        event.stopPropagation();
        closeTab(tab);
      });
      item.append(name, close);
      item.addEventListener('click', () => tab !== current && show(tab));
      // a middle click closes, as in a browser
      item.addEventListener('auxclick', event => event.button === 1 && closeTab(tab));
      // drag a tab to reorder: it lands before the tab it is dropped on's midpoint, else after it
      item.draggable = true;
      item.addEventListener('dragstart', event => {
        dragging = tab;
        event.dataTransfer?.setData('text/plain', tab.title);
      });
      item.addEventListener('dragend', () => {
        dragging = null;
        clearDropMarks();
      });
      item.addEventListener('dragover', event => {
        if (!dragging) return;
        event.preventDefault();
        clearDropMarks();
        item.classList.add(droppedBefore(event, item) ? 'drop-before' : 'drop-after');
      });
      item.addEventListener('drop', event => {
        if (!dragging) return;
        event.preventDefault();
        const from = tabs.indexOf(dragging);
        moveItem(tabs, from, tabs.indexOf(tab) + (droppedBefore(event, item) ? 0 : 1));
        dragging = null;
        renderTabs();
        persist();
      });
      return item;
    }),
  );
}

function changed(id: number) {
  if (current.id === id) $('tabs').querySelector(`[data-id="${id}"]`)?.classList.toggle('dirty', isDirty(current));
  persist();
}

function persist() {
  sync();
  const stored: StoredTabs = {
    tabs: tabs.map(tab => ({ path: tab.path, title: tab.title, text: tab.state.doc.toString(), saved: tab.saved })),
    active: tabs.indexOf(current),
  };
  localStorage.setItem(tabsKey, JSON.stringify(stored));
}

async function ensureConnected() {
  if (connected) return true;
  const port = Number(input('port').value) || 7401;
  localStorage.setItem(portKey, String(port));
  status('looking for the engine…');
  try {
    await stage.connect(port, input('token').value.trim());
    setConnection(true);
    status('connected');
    return true;
  } catch (error) {
    setConnection(false);
    status(String(error));
    editorStatus.textContent = String(error);
    return false;
  }
}

function sceneButton(scene: Scene, index: number, shown: ReturnType<typeof sceneStates>) {
  const button = document.createElement('button');
  button.className = 'scene';
  const { state, within } = shown.get(scene.name) ?? { state: 'off' as const };
  const active = state === 'playing';
  button.classList.toggle('active', active);
  button.classList.toggle('included', state === 'included');
  button.classList.toggle('partial', state === 'partial');
  button.title = {
    playing: `${scene.name} is playing; click to stop it`,
    included: `Part of ${within}, which is playing; click to switch down to just ${scene.name}`,
    partial: `Some of ${scene.name} is playing; click to switch to all of it`,
    off: `Switch to ${scene.name}: the other scenes' clips stop as it starts`,
  }[state];
  const icon = document.createElement('span');
  icon.className = 'scene-icon';
  icon.textContent = active ? '■' : state === 'included' ? '⊂' : '▶';
  const name = document.createElement('span');
  name.className = 'scene-name';
  name.textContent = scene.name;
  const count = document.createElement('span');
  count.className = 'scene-count';
  count.textContent = `${scene.running}/${scene.clips.length}`;
  button.append(icon, name, count);
  if (index < 9) {
    const key = document.createElement('span');
    key.className = 'scene-key';
    key.textContent = `${mod}${index + 1}`;
    button.append(key);
  }
  button.addEventListener('click', () => void launch(scene));
  return button;
}

async function launch(scene: Scene) {
  if (!(await ensureConnected())) return;
  try {
    showOutcome(await stage.evaluate(sceneCommand(scene, sceneList), input('quantize').value.trim(), null, 0));
    await refresh();
  } catch (error) {
    status(String(error));
  }
}

let sceneList: Scene[] = [];

function updateScenes(list: Scene[]) {
  sceneList = list;
  const signature = JSON.stringify(list.map(scene => [scene.name, scene.running, scene.clips]));
  if (signature === lastSceneSignature) return;
  lastSceneSignature = signature;
  const shown = sceneStates(list);
  scenes.replaceChildren(...list.map((scene, index) => sceneButton(scene, index, shown)));
  stageHint.textContent = list.length === 0 ? 'no scenes yet' : `${list.length} scene${list.length === 1 ? '' : 's'}`;
}

/** Flash the editor: green when a change lands, amber when one is sent, red when it is refused. */
function flash(kind: 'landed' | 'sent' | 'refused') {
  const pane = $('backstage');
  pane.classList.remove('flash-landed', 'flash-sent', 'flash-refused');
  void pane.offsetWidth;
  pane.classList.add(`flash-${kind}`);
}

/** How long until the next change lands, counted in beats so it is felt rather than read. */
function updateLanding(snapshot: Snapshot) {
  const landing = $('landing');
  const t = snapshot.transport;
  const next = snapshot.pending.reduce<number | null>(
    (soonest, p) => (soonest == null ? p.lands_at : Math.min(soonest, p.lands_at)),
    null,
  );
  if (!t || next == null || !snapshot.sample_rate) {
    landing.textContent = '';
    landing.classList.remove('on');
    return;
  }
  const beats = Math.max(0, ((next - t.sample) / snapshot.sample_rate) * (t.bpm / 60));
  landing.textContent = `lands in ${beats.toFixed(1)} beats`;
  landing.classList.add('on');
}

/** What lands next, with a countdown in beats that is updated in place, so its buttons can be clicked. */
let pendingSignature = '';
function updatePending(snapshot: Snapshot) {
  updateLanding(snapshot);
  if (!snapshot.pending.length) {
    pending.replaceChildren();
    pending.classList.add('empty');
    pendingSignature = '';
    return;
  }
  pending.classList.remove('empty');
  const t = snapshot.transport;
  const beatsUntil = (landsAt: number) =>
    t && snapshot.sample_rate ? Math.max(0, ((landsAt - t.sample) / snapshot.sample_rate) * (t.bpm / 60)) : 0;
  const signature = JSON.stringify(snapshot.pending.map(change => [change.id, change.changes]));
  if (signature !== pendingSignature) {
    pendingSignature = signature;
    pending.replaceChildren();
    const title = document.createElement('div');
    title.className = 'pending-title';
    title.textContent = 'next';
    pending.append(title);
    for (const change of snapshot.pending) {
      const row = document.createElement('div');
      row.className = 'pending-row';
      row.dataset.lands = String(change.lands_at);
      const when = document.createElement('span');
      when.className = 'pending-when';
      const text = document.createElement('span');
      text.className = 'pending-text';
      text.textContent = change.changes.join(' · ') || 'a change';
      const cancel = document.createElement('button');
      cancel.className = 'pending-cancel';
      cancel.textContent = '×';
      cancel.title = 'Cancel this change';
      cancel.addEventListener('click', () => void cancelPending(change.id));
      row.append(when, text, cancel);
      pending.append(row);
    }
  }
  for (const row of pending.querySelectorAll<HTMLElement>('.pending-row')) {
    const when = row.querySelector('.pending-when')!;
    when.textContent = `in ${beatsUntil(Number(row.dataset.lands)).toFixed(1)} beats`;
  }
}

function updateNotices(snapshot: Snapshot) {
  for (const notice of snapshot.notices) {
    noticeLog.push(notice);
    status(notice);
    // the engine can turn down a change that compiled, when it lands: that must not pass unseen
    if (/was refused by the engine/.test(notice)) {
      showBanner([{ message: notice, help: null, line: null, column: null, in_definition: null }], 'not applied');
      flash('refused');
    }
  }
  for (const landed of snapshot.landed) {
    noticeLog.push(`landed: ${landed.changes.join(' · ')}`);
  }
  if (snapshot.landed.length) flash('landed');
  noticeLog = noticeLog.slice(-1);
  notices.replaceChildren(
    ...noticeLog.map(message => {
      const row = document.createElement('div');
      row.className = 'notice';
      row.textContent = message;
      return row;
    }),
  );
}

function updateTransport(snapshot: Snapshot) {
  const t = snapshot.transport;
  if (!t) {
    bar.textContent = '1';
    beat.textContent = '1.1';
    bpm.textContent = '—';
    voices.textContent = '0';
    waiting.textContent = '0';
    return;
  }
  bar.textContent = String(t.bar);
  beat.textContent = t.beat.toFixed(2);
  bpm.textContent = t.bpm.toFixed(1);
  meter.textContent = snapshot.meter.join('/');
  voices.textContent = String(t.voices);
  waiting.textContent = String(snapshot.pending.length);
}

/** A refused evaluation is shown big at the top of the code, where it cannot be missed from across a room. */
let bannerTimer: ReturnType<typeof setTimeout> | undefined;
/** How long the banner stays up; the problems list below the code keeps the detail. */
const BANNER_SECONDS = 6;

function showBanner(list: Problem[], heading = 'not sent') {
  const banner = $('banner');
  clearTimeout(bannerTimer);
  if (!list.length) {
    banner.hidden = true;
    return;
  }
  const first = list[0];
  const where = first.line ? ` · line ${first.line}` : '';
  banner.textContent = `✕ ${heading}${where} · ${first.message}${list.length > 1 ? ` (+${list.length - 1} more)` : ''}`;
  banner.hidden = false;
  bannerTimer = setTimeout(() => showBanner([]), BANNER_SECONDS * 1000);
}

function showProblems(list: Problem[]) {
  showBanner(list);
  problems.replaceChildren();
  editorStatus.textContent = '';
  if (!list.length) return;
  for (const problem of list) {
    const row = document.createElement('div');
    row.className = 'problem';
    const where = problem.line ? `${problem.line}${problem.column ? `:${problem.column}` : ''}` : '';
    if (where) {
      const location = document.createElement('span');
      location.className = 'problem-where';
      location.textContent = where;
      row.append(location);
    }
    const message = document.createElement('span');
    message.textContent = problem.message;
    row.append(message);
    if (problem.help) {
      const help = document.createElement('span');
      help.className = 'problem-help';
      help.textContent = problem.help;
      row.append(help);
    }
    problems.append(row);
  }
}

function showOutcome(outcome: EvalOutcome) {
  showProblems(outcome.problems);
  if (outcome.problems.length) {
    status('the code did not compile');
    flash('refused');
    return false;
  }
  const accepted = outcome.accepted;
  if (accepted?.id) flash('sent');
  sent = true;
  if (!accepted?.id) {
    status('nothing to do');
  } else {
    status(`lands at bar ${accepted.position.bar} beat ${accepted.position.beat.toFixed(2)}`);
  }
  return true;
}

async function evaluate(scope: Scope = 'chunk') {
  if (!(await ensureConnected())) return;
  const code = chunk(scope);
  if (!code) {
    status('put the cursor in a block of code, or select some');
    return;
  }
  const { text, from, to } = code;
  const tab = current;
  const id = ++evaluations;
  if (view) {
    view.dispatch({ effects: evaluated.of({ id, base: from, text }) });
    flashLines(view, from, to);
  }
  try {
    const outcome = await stage.evaluate(text, input('quantize').value.trim(), fileDir(), id);
    showOutcome(outcome);
    if (view && tab === current && outcome.problems.length) markProblems(view, from, text, outcome.problems);
    await refresh();
  } catch (error) {
    status(String(error));
  }
}

async function act(action: Action) {
  if (!(await ensureConnected())) return;
  try {
    const outcome = await stage.perform(action, input('quantize').value.trim());
    showOutcome(outcome);
    await refresh();
  } catch (error) {
    status(String(error));
  }
}

async function cancelPending(id: number) {
  try {
    await stage.cancel(id);
    await refresh();
  } catch (error) {
    status(String(error));
  }
}

async function openFile() {
  try {
    const opened = await stage.openFile();
    if (!opened) return;
    const open = tabs.find(tab => tab.path === opened.path);
    if (open) show(open);
    else addTab(opened.source, opened.path, 'untitled');
    status(`opened ${opened.path}; evaluate it when you are ready`);
  } catch (error) {
    status(String(error));
  }
}

/** Open a folder's files as tabs: `main.nml` and what it uses, or every file in the folder without one. */
async function openProject() {
  try {
    const project = await stage.openProject();
    if (!project) return;
    let first: Tab | null = null;
    for (const file of project.files) {
      let tab = tabs.find(t => t.path === file.path);
      if (!tab) {
        tab = makeTab(file.source, file.path, 'untitled');
        tabs.push(tab);
      }
      first ??= tab;
    }
    if (first) show(first);
    const what = project.entry ? `evaluate ${tabTitle(project.entry, 'main.nml')} to play it` : 'evaluate the tabs left to right with All';
    status(`opened ${project.files.length} file${project.files.length === 1 ? '' : 's'} from ${project.dir}; ${what}`);
  } catch (error) {
    status(String(error));
  }
}

async function saveFile(tab = current) {
  if (!view) return;
  const text = tab === current ? view.state.doc.toString() : tab.state.doc.toString();
  try {
    if (!tab.path) {
      const path = await stage.saveFileAs(text);
      if (!path) return;
      tab.path = path;
      tab.title = tabTitle(path, tab.title);
    } else {
      await stage.saveFile(tab.path, text);
    }
    tab.saved = text;
    renderTabs();
    persist();
    status(`saved ${tab.path}`);
  } catch (error) {
    status(String(error));
  }
}

/** Save every tab that has a file and has changed. */
async function saveAll() {
  for (const tab of tabs) if (tab.path && isDirty(tab)) await saveFile(tab);
}

function loadDemo() {
  addTab(demoSource, null, 'demo');
  status('the opening piece is loaded; evaluate it when you are ready');
}

/** Send every tab in turn, left to right, so a tab of instruments goes before the clips that play them. */
async function evaluateAll() {
  for (const tab of tabs) {
    if (tab !== current) show(tab);
    await evaluate('file');
  }
}

let linkLost = false;

/** What the window shows with no engine: nothing playing, and the panels saying so. */
function offline(): Snapshot {
  return {
    connected: false,
    revision: 0,
    transport: null,
    beats_per_bar: 4,
    meter: [4, 4],
    sample_rate: 0,
    channels: 0,
    clips: [],
    scenes: [],
    pending: [],
    landed: [],
    notices: [],
    error: null,
  };
}

function drawOffline() {
  const snapshot = offline();
  laneView.update(snapshot);
  updateEvalState(snapshot);
}

async function refresh() {
  let snapshot: Snapshot;
  try {
    snapshot = await stage.snapshot();
  } catch (error) {
    setConnection(false);
    status(String(error));
    drawOffline();
    return;
  }
  setConnection(snapshot.connected);
  if (!snapshot.connected) {
    drawOffline();
    // the engine went away or never answered: look for it again now and then, without sending anything
    if (connectedOnce && !linkLost) {
      linkLost = true;
      status('lost the engine; looking for it again…');
    }
    return;
  }
  linkLost = false;
  connectedOnce = true;
  // a mistake in drawing one part of the window must not stop the others, or the polling
  const parts: [string, () => void][] = [
    ['transport', () => updateTransport(snapshot)],
    ['scenes', () => updateScenes(snapshot.scenes)],
    ['pending', () => updatePending(snapshot)],
    ['notices', () => updateNotices(snapshot)],
    ['lanes', () => laneView.update(snapshot)],
    ['highlights', () => highlighter.update(snapshot)],
    ['status', () => updateClipCount(snapshot)],
    ['state', () => updateEvalState(snapshot)],
  ];
  for (const [name, draw] of parts) {
    try {
      draw();
    } catch (error) {
      console.error(`drawing the ${name} failed`, error);
      editorStatus.textContent = `drawing the ${name} failed: ${error}`;
    }
  }
}

/** Whether anything has been sent yet, so a quiet window does not look the same as a broken one. */
let sent = false;
function updateEvalState(snapshot: Snapshot) {
  const chip = $('eval-state');
  if (!snapshot.connected) chip.textContent = 'offline';
  else if (!sent && snapshot.clips.length === 0) chip.textContent = 'ready · not evaluated yet';
  else chip.textContent = `${snapshot.clips.length} clip${snapshot.clips.length === 1 ? '' : 's'}${snapshot.clips.some(c => c.running) ? ' · playing' : ' · stopped'}`;
  chip.classList.toggle('playing', snapshot.clips.some(c => c.running));
}

function updateClipCount(snapshot: Snapshot) {
  if (lastClipCount === snapshot.clips.length) return;
  lastClipCount = snapshot.clips.length;
  status(
    snapshot.clips.length
      ? `${snapshot.clips.length} clip${snapshot.clips.length === 1 ? '' : 's'} on the stage`
      : 'the stage is empty',
  );
}

let polls = 0;
async function poll() {
  await refresh();
  // an engine that has gone is looked for every couple of seconds
  if (linkLost && ++polls % 25 === 0) {
    try {
      await stage.connect(Number(input('port').value) || 7401, input('token').value.trim());
    } catch {
      // still not there
    }
  }
  setTimeout(poll, 90);
}

$('banner').addEventListener('click', () => showBanner([]));

// Text size: one scale for the whole window, so the code, the sidebar and the controls grow together.
const ZOOMS = [0.7, 0.85, 1, 1.15, 1.3, 1.5, 1.75, 2, 2.5, 3];
const zoomKey = 'niminal-stage-zoom';
let zoom = 1;
function setZoom(value: number) {
  zoom = Math.min(Math.max(value, ZOOMS[0]), ZOOMS[ZOOMS.length - 1]);
  document.documentElement.style.setProperty('--z', String(zoom));
  $('zoom-reset').textContent = `${Math.round(zoom * 100)}%`;
  localStorage.setItem(zoomKey, String(zoom));
  view?.requestMeasure();
}
function stepZoom(direction: 1 | -1) {
  const next = direction > 0 ? ZOOMS.find(z => z > zoom + 1e-6) : [...ZOOMS].reverse().find(z => z < zoom - 1e-6);
  if (next !== undefined) setZoom(next);
}
setZoom(Number(localStorage.getItem(zoomKey)) || 1);
$('zoom-in').addEventListener('click', () => stepZoom(1));
$('zoom-out').addEventListener('click', () => stepZoom(-1));
$('zoom-reset').addEventListener('click', () => setZoom(1));
// pinching on a trackpad arrives as a wheel with the control key held
let pinch = 0;
window.addEventListener(
  'wheel',
  event => {
    if (!event.ctrlKey) return;
    event.preventDefault();
    pinch -= event.deltaY;
    if (Math.abs(pinch) < 12) return;
    stepZoom(pinch > 0 ? 1 : -1);
    pinch = 0;
  },
  { passive: false },
);

// Performance mode keeps the code, what is playing and the way to stop; the sidebar can go too.
const modeKey = 'niminal-stage-mode';
function setMode(name: 'perform' | 'no-rail', on: boolean) {
  document.body.classList.toggle(name, on);
  const saved = new Set((localStorage.getItem(modeKey) ?? '').split(',').filter(Boolean));
  if (on) saved.add(name);
  else saved.delete(name);
  localStorage.setItem(modeKey, [...saved].join(','));
  view?.requestMeasure();
}
for (const name of (localStorage.getItem(modeKey) ?? '').split(',')) {
  if (name === 'perform' || name === 'no-rail') document.body.classList.add(name);
}
$('perform').addEventListener('click', () => setMode('perform', !document.body.classList.contains('perform')));
$('rail-toggle').addEventListener('click', () => setMode('no-rail', !document.body.classList.contains('no-rail')));

// The divider between the code and the sidebar: drag it, or use the arrow keys, to give either more room.
const railKey = 'niminal-stage-rail';
const mainPane = document.querySelector<HTMLElement>('main')!;
function setRail(width: number | null, save = true) {
  if (width === null) {
    mainPane.style.removeProperty('--rail');
    localStorage.removeItem(railKey);
    return;
  }
  const rail = Math.round(Math.min(Math.max(width, 220), Math.max(220, mainPane.clientWidth - 320)));
  mainPane.style.setProperty('--rail', `${rail}px`);
  if (save) localStorage.setItem(railKey, String(rail));
}
const savedRail = Number(localStorage.getItem(railKey));
if (savedRail) setRail(savedRail, false);
const divider = $('divider');
divider.addEventListener('pointerdown', event => {
  // without this the drag also selects the text and controls it passes over
  event.preventDefault();
  divider.setPointerCapture(event.pointerId);
  divider.classList.add('dragging');
  document.body.classList.add('resizing');
});
divider.addEventListener('pointermove', event => {
  if (divider.hasPointerCapture(event.pointerId)) setRail(mainPane.getBoundingClientRect().right - event.clientX);
});
divider.addEventListener('pointerup', event => {
  divider.releasePointerCapture(event.pointerId);
  divider.classList.remove('dragging');
  document.body.classList.remove('resizing');
});
divider.addEventListener('pointercancel', () => {
  divider.classList.remove('dragging');
  document.body.classList.remove('resizing');
});
divider.addEventListener('dblclick', () => setRail(null));
divider.addEventListener('keydown', event => {
  const step = event.shiftKey ? 80 : 20;
  const width = $('stage').getBoundingClientRect().width;
  if (event.key === 'ArrowLeft') setRail(width + step);
  else if (event.key === 'ArrowRight') setRail(width - step);
  else return;
  event.preventDefault();
});

const highlighter = new Highlighter(() => view ?? undefined);
const laneView = new LaneView($('lanes'), (action, target, change) => {
  if (action === 'connect') $('reconnect').click();
  else void act({ action, target, change });
});

$('evaluate').addEventListener('click', () => void evaluate());
$('evaluate-file').addEventListener('click', () => void evaluate('file'));
$('new').addEventListener('click', () => addTab('', null, 'untitled'));
$('evaluate-all').addEventListener('click', () => void evaluateAll());
$('open').addEventListener('click', () => void openFile());
$('open-project').addEventListener('click', () => void openProject());
$('save').addEventListener('click', () => void saveFile());
$('demo').addEventListener('click', loadDemo);
$('hush').addEventListener('click', () => void act({ action: 'hush' }));
$('panic').addEventListener('click', () => void act({ action: 'panic' }));
$('reconnect').addEventListener('click', async () => {
  try {
    await stage.disconnect();
    setConnection(false);
    lastSceneSignature = '';
    lastClipCount = -1;
    await ensureConnected();
  } catch (error) {
    status(String(error));
  }
});

window.addEventListener('keydown', event => {
  const mod = event.metaKey || event.ctrlKey;
  if (!mod) {
    if (event.key === 'Escape') showBanner([]);
    return;
  }
  if (event.key === '=' || event.key === '+') {
    event.preventDefault();
    stepZoom(1);
  } else if (event.key === '-') {
    event.preventDefault();
    stepZoom(-1);
  } else if (event.key === '0') {
    event.preventDefault();
    setZoom(1);
  } else if (event.key.toLowerCase() === 'p' && event.shiftKey) {
    event.preventDefault();
    $('perform').click();
  } else if (event.key.toLowerCase() === 't' && !event.shiftKey) {
    event.preventDefault();
    $('new').click();
  } else if (event.shiftKey && (event.key === '[' || event.key === '{')) {
    event.preventDefault();
    step(-1);
  } else if (event.shiftKey && (event.key === ']' || event.key === '}')) {
    event.preventDefault();
    step(1);
  } else if (event.key.toLowerCase() === 'b' && !event.shiftKey) {
    event.preventDefault();
    $('rail-toggle').click();
  } else if (/^[1-9]$/.test(event.key) && !event.shiftKey) {
    // the nth scene, as clicking it would
    const scene = sceneList[Number(event.key) - 1];
    if (scene) {
      event.preventDefault();
      void launch(scene);
    }
  }
  if (event.key === '.') {
    event.preventDefault();
    void act({ action: 'hush' });
  }
  if (event.key === 'Enter' && !(event.target instanceof HTMLElement && event.target.closest('.cm-editor'))) {
    event.preventDefault();
    void evaluate();
  }
});

/** The menu bar: each item runs what its button or key already does. */
const menuActions: Record<string, () => void> = {
  new: () => $('new').click(),
  open: () => void openFile(),
  'open-project': () => void openProject(),
  save: () => void saveFile(),
  'save-all': () => void saveAll(),
  'close-tab': () => closeTab(current),
  undo: () => view && undo(view),
  redo: () => view && redo(view),
  sidebar: () => $('rail-toggle').click(),
  perform: () => $('perform').click(),
  'zoom-in': () => stepZoom(1),
  'zoom-out': () => stepZoom(-1),
  'zoom-reset': () => setZoom(1),
  'previous-tab': () => step(-1),
  'next-tab': () => step(1),
};
if (isTauri()) {
  listen<string>('menu', event => menuActions[event.payload]?.()).catch(error => status(`The menu is not connected: ${error}`));
}

const stored = loadTabs(localStorage.getItem(tabsKey));
if (stored) {
  tabs = stored.tabs.map(tab => makeTab(tab.text, tab.path, tab.title, tab.saved));
  show(tabs[stored.active]);
} else {
  addTab(demoSource, null, 'demo');
}
input('port').value = localStorage.getItem(portKey) ?? '7401';
setConnection(false);
void connectLanguageServer(message => status(message));
void ensureConnected();
void poll();
