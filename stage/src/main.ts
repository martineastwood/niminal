import { EditorView } from '@codemirror/view';
import { demoSource } from './demo';
import { Scope, connectLanguageServer, niminalEditor } from './editor';
import { blockAround } from '../../editors/codemirror/blocks';
import { LaneView } from './laneview';
import { Highlighter, evaluated, flashLines } from './highlight';
import { Action, EvalOutcome, Problem, Scene, Snapshot, stage } from './live';
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
      <label class="port">port <input id="port" inputmode="numeric" value="7401" /></label>
      <input id="token" type="password" placeholder="token" aria-label="engine token" />
      <button id="reconnect" title="Connect to the engine">Connect</button>
      <button id="hush" title="Stop every clip, letting what sounds ring out">Hush</button>
      <button id="panic" class="danger" title="Stop everything now">Panic</button>
    </div>
  </header>

  <main>
    <section id="backstage">
      <div class="pane-head">
        <span class="pane-title">code</span>
        <span id="landing" class="landing" aria-live="polite"></span>
        <div class="editor-actions">
          <button id="open" title="Open a .nml file">Open</button>
          <button id="save" title="Save the file">Save</button>
          <button id="demo" title="Load the stage's opening piece">Demo</button>
          <label class="quantize" title="When an evaluation lands">land <input id="quantize" placeholder="next 4bars" /></label>
          <button id="evaluate-file" title="Evaluate the whole file (Cmd/Ctrl+Shift+Enter)">File <kbd>⇧⌘↩</kbd></button>
          <button id="evaluate" class="primary" title="Evaluate the selection, or the block the cursor is in (Cmd/Ctrl+Enter)">Evaluate <kbd>⌘↩</kbd></button>
        </div>
      </div>
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
    <span class="hint">click a scene to play · reverse and thin show up in the lanes</span>
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

const storageKey = 'niminal-stage:source';
const pathKey = 'niminal-stage:path';
const portKey = 'niminal-stage:port';

let view: EditorView | null = null;
// numbered by the clock, so what the engine remembers from an earlier run never matches a new one
let evaluations = Date.now();
let filePath: string | null = localStorage.getItem(pathKey);
let fileDir: string | null = null;
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

function installEditor(source: string, path: string | null) {
  const state = niminalEditor(
    source,
    uriFor(path ?? '/niminal-stage/piece.nml'),
    scope => void evaluate(scope),
    saveLocal,
  );
  if (view) view.setState(state);
  else {
    view = new EditorView({ state, parent: $('editor') });
  }
  filePath = path;
  if (path) localStorage.setItem(pathKey, path);
  else localStorage.removeItem(pathKey);
  view.focus();
}

function saveLocal() {
  if (!view) return;
  localStorage.setItem(storageKey, view.state.doc.toString());
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

function sceneButton(scene: Scene) {
  const button = document.createElement('button');
  button.className = 'scene';
  const active = scene.running === scene.clips.length && scene.clips.length > 0;
  button.classList.toggle('active', active);
  button.classList.toggle('partial', scene.running > 0 && !active);
  button.title = active ? `${scene.name} is playing; click to stop it` : `Start ${scene.name}`;
  const icon = document.createElement('span');
  icon.className = 'scene-icon';
  icon.textContent = active ? '■' : '▶';
  const name = document.createElement('span');
  name.className = 'scene-name';
  name.textContent = scene.name;
  const count = document.createElement('span');
  count.className = 'scene-count';
  count.textContent = `${scene.running}/${scene.clips.length}`;
  button.append(icon, name, count);
  button.addEventListener('click', () => {
    void act({ action: active ? 'stop' : 'start', target: scene.name });
  });
  return button;
}

function updateScenes(list: Scene[]) {
  const signature = JSON.stringify(list.map(scene => [scene.name, scene.running, scene.clips]));
  if (signature === lastSceneSignature) return;
  lastSceneSignature = signature;
  scenes.replaceChildren(...list.map(sceneButton));
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

function updatePending(snapshot: Snapshot) {
  updateLanding(snapshot);
  pending.replaceChildren();
  if (!snapshot.pending.length) {
    pending.classList.add('empty');
    return;
  }
  pending.classList.remove('empty');
  for (const change of snapshot.pending) {
    const row = document.createElement('div');
    row.className = 'pending-row';
    const when = document.createElement('span');
    when.className = 'pending-when';
    when.textContent = `bar ${change.position.bar} · ${change.position.beat.toFixed(2)}`;
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

function updateNotices(snapshot: Snapshot) {
  for (const notice of snapshot.notices) {
    noticeLog.push(notice);
    status(notice);
  }
  for (const landed of snapshot.landed) {
    noticeLog.push(`landed: ${landed.changes.join(' · ')}`);
  }
  if (snapshot.landed.length) flash('landed');
  noticeLog = noticeLog.slice(-6);
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
  meter.textContent = `${snapshot.beats_per_bar}/4`;
  voices.textContent = String(t.voices);
  waiting.textContent = String(snapshot.pending.length);
}

function showProblems(list: Problem[]) {
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
  const id = ++evaluations;
  if (view) {
    view.dispatch({ effects: evaluated.of({ id, base: from, text }) });
    flashLines(view, from, to);
  }
  try {
    const outcome = await stage.evaluate(text, input('quantize').value.trim(), fileDir, id);
    showOutcome(outcome);
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
    installEditor(opened.source, opened.path);
    fileDir = opened.dir;
    localStorage.setItem(storageKey, opened.source);
    status(`opened ${opened.path}`);
    showProblems([]);
    status(`opened ${opened.path}; evaluate it when you are ready`);
  } catch (error) {
    status(String(error));
  }
}

async function saveFile() {
  if (!view) return;
  try {
    if (!filePath) {
      const path = await stage.saveFileAs(view.state.doc.toString());
      if (!path) return;
      filePath = path;
      fileDir = path.replace(/[\\/][^\\/]*$/, '');
      localStorage.setItem(pathKey, path);
    } else {
      await stage.saveFile(filePath, view.state.doc.toString());
    }
    localStorage.setItem(storageKey, view.state.doc.toString());
    status(`saved ${filePath}`);
  } catch (error) {
    status(String(error));
  }
}

function loadDemo() {
  installEditor(demoSource, null);
  fileDir = null;
  localStorage.setItem(storageKey, demoSource);
  showProblems([]);
  status('the opening piece is loaded; evaluate it when you are ready');
}

let linkLost = false;

async function refresh() {
  let snapshot: Snapshot;
  try {
    snapshot = await stage.snapshot();
  } catch (error) {
    setConnection(false);
    status(String(error));
    return;
  }
  setConnection(snapshot.connected);
  if (!snapshot.connected) {
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
  void act({ action, target, change });
});

$('evaluate').addEventListener('click', () => void evaluate());
$('evaluate-file').addEventListener('click', () => void evaluate('file'));
$('open').addEventListener('click', () => void openFile());
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
  if (!mod) return;
  if (event.key === '.') {
    event.preventDefault();
    void act({ action: 'hush' });
  }
  if (event.key === 'Enter' && !(event.target instanceof HTMLElement && event.target.closest('.cm-editor'))) {
    event.preventDefault();
    void evaluate();
  }
});

const stored = localStorage.getItem(storageKey);
fileDir = filePath ? filePath.replace(/[\\/][^\\/]*$/, '') : null;
installEditor(stored ?? demoSource, filePath);
input('port').value = localStorage.getItem(portKey) ?? '7401';
setConnection(false);
void connectLanguageServer(message => status(message));
void ensureConnected();
void poll();
