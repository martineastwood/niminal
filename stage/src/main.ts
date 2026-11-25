import { redo, undo } from '@codemirror/commands';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { elapsedSince } from './animation';
import { createAudioSettings } from './audio-settings';
import { Scope, connectLanguageServer } from './editor';
import { onFrame, wake } from './frames';
import { Highlighter, evaluated, flashLines, markProblems, markSent } from './highlight';
import { LaneView } from './laneview';
import { $, input, renderShell, setupLayout } from './layout';
import { Action, EvalOutcome, Problem, Snapshot, stage } from './live';
import { loadLook } from './look';
import { createScope } from './scope';
import { settingsPage } from './settings';
import { createStageView } from './stage-view';
import { createTransport } from './transport';
import { createWorkbench } from './workbench';
import './style.css';

renderShell(document.querySelector<HTMLDivElement>('#app')!);

const connection = $('connection');
const problems = $('problems');
const editorStatus = $('editor-status');
const footerStatus = $('footer-status');

// numbered by the clock, so what the engine remembers from an earlier run never matches a new one
let evaluations = Date.now();
let connected = false;
let connectedOnce = false;
let lastClipCount = -1;
let linkLost = false;
/** Whether anything has been sent yet, so a quiet window does not look the same as a broken one. */
let sent = false;

function status(message: string) {
  footerStatus.textContent = message;
}

function setConnection(ok: boolean) {
  connected = ok;
  connection.classList.toggle('offline', !ok);
  connection.classList.toggle('online', ok);
  // the engine runs inside the stage, so it is nearly always there: say so only when it is not
  connection.hidden = ok;
  connection.querySelector('span')!.textContent = 'engine offline';
  $('setup').hidden = ok;
  scope.setConnected(ok);
}

/** A yes or no question in a native dialog: the webview's own `confirm` does nothing here. */
const ask = (message: string) => invoke<boolean>('confirm', { message }).catch(() => false);

async function ensureConnected() {
  if (connected) return true;
  status('starting the engine…');
  try {
    await stage.startEngine();
    setConnection(true);
    status('engine running');
    return true;
  } catch (error) {
    setConnection(false);
    const message = String(error);
    status(message);
    editorStatus.textContent = message;
    return false;
  }
}

/** Flash the editor: green when a change lands, amber when one is sent, red when it is refused. */
function flash(kind: 'landed' | 'sent' | 'refused') {
  const pane = $('backstage');
  pane.classList.remove('flash-landed', 'flash-sent', 'flash-refused');
  void pane.offsetWidth;
  pane.classList.add(`flash-${kind}`);
}

/** A refused evaluation is shown big at the top of the code, where it cannot be missed from across a room. */
let bannerTimer: ReturnType<typeof setTimeout> | undefined;
/** A message shown as a problem that points at no line. */
const problem = (message: string): Problem => ({ message, help: null, line: null, column: null, in_definition: null });
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
  for (const p of list) {
    const row = document.createElement('div');
    row.className = 'problem';
    const where = p.line ? `${p.line}${p.column ? `:${p.column}` : ''}` : '';
    if (where) {
      const location = document.createElement('span');
      location.className = 'problem-where';
      location.textContent = where;
      row.append(location);
    }
    const message = document.createElement('span');
    message.textContent = p.message;
    row.append(message);
    if (p.help) {
      const help = document.createElement('span');
      help.className = 'problem-help';
      help.textContent = p.help;
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

const highlighter = new Highlighter(() => workbench.view ?? undefined);
const workbench = createWorkbench({ evaluate, showProblems, status, ask, highlighter });
const layout = setupLayout(() => workbench.view?.requestMeasure());
const transport = createTransport({ status, showBanner, problem, writeLearned: workbench.writeLearned });
const stageView = createStageView({
  ensureConnected,
  showOutcome,
  refresh,
  status,
  showBanner,
  problem,
  flash,
  cancelPending,
  quantize: () => input('quantize').value.trim(),
});
const laneView = new LaneView($('lanes'), (action, target, change) => {
  if (action === 'connect') void reconnect();
  else void act({ action, target, change });
});
const scope = createScope($('scope'));

async function evaluate(scope: Scope = 'chunk') {
  if (!(await ensureConnected())) return;
  const code = workbench.chunk(scope);
  if (scope === 'step') workbench.stepOn(code ? code.to : null);
  if (!code) {
    if (scope !== 'step') status('put the cursor in a block of code, or select some');
    return;
  }
  const { text, from, to } = code;
  const tab = workbench.current;
  const id = ++evaluations;
  const view = workbench.view;
  if (view) {
    view.dispatch({ effects: evaluated.of({ id, base: from, text }) });
    flashLines(view, from, to);
  }
  try {
    const outcome = await stage.evaluate(text, input('quantize').value.trim(), workbench.fileDir(), id);
    showOutcome(outcome);
    if (view && tab === workbench.current && outcome.problems.length) markProblems(view, from, text, outcome.problems);
    if (view && tab === workbench.current && outcome.accepted?.id) markSent(view, outcome.accepted.id, from, to);
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

/** Run `new`, after asking, since it clears every clip, scene and the rest of what the session holds. */
async function newSession() {
  if (await ask('New session: what is playing stops and your clips and scenes are cleared. Continue?')) await act({ action: 'new' });
}

async function cancelPending(id: number) {
  try {
    await stage.cancel(id);
    await refresh();
  } catch (error) {
    status(String(error));
  }
}

/** What the window shows with no engine: nothing playing, and the panels saying so. */
function offline(): Snapshot {
  return {
    connected: false,
    transport: null,
    beats_per_bar: 4,
    meter: [4, 4],
    sample_rate: 0,
    channels: 0,
    clips: [],
    scenes: [],
    arrangements: [],
    pending: [],
    landed: [],
    notices: [],
    master: [],
    levels: { nodes: {}, buses: {} },
    health: null,
    learned: [],
    midi: { inputs: [], last: null },
    tools_problem: null,
    tools_port: 7400,
    osc_port: null,
    engine_error: null,
  };
}

async function refresh() {
  let snapshot: Snapshot;
  try {
    snapshot = await stage.snapshot();
  } catch (error) {
    setConnection(false);
    status(String(error));
    draw(offline());
    return;
  }
  setConnection(snapshot.connected);
  if (!snapshot.connected) {
    draw(offline());
    // the engine went away or never answered: look for it again now and then, without sending anything
    if (connectedOnce && !linkLost) {
      linkLost = true;
      status('lost the engine; looking for it again…');
    }
    return;
  }
  linkLost = false;
  connectedOnce = true;
  draw(snapshot);
}

let animationSnapshot: Snapshot | null = null;
let animationUpdatedAt = 0;
let moving = false;
function animate(): boolean {
  if (animationSnapshot?.transport) {
    const elapsed = elapsedSince(animationUpdatedAt);
    transport.paintPosition(animationSnapshot, elapsed);
    stageView.paintCountdowns(animationSnapshot, elapsed);
  }
  return isMoving(animationSnapshot);
}
onFrame(animate);

/** Whether the window has anything that moves between polls: a clip playing or a change counting down. */
function isMoving(snapshot: Snapshot | null) {
  return !!snapshot?.transport && (snapshot.pending.length > 0 || snapshot.clips.some(clip => clip.running));
}

function draw(snapshot: Snapshot) {
  animationSnapshot = snapshot;
  moving = isMoving(snapshot);
  wake();
  animationUpdatedAt = performance.now();
  // a mistake in drawing one part of the window must not stop the others, or the polling
  const parts: [string, () => void][] = [
    ['transport', () => transport.updateTransport(snapshot)],
    ['meters', () => transport.updateMaster(snapshot)],
    ['health', () => transport.updateHealth(snapshot)],
    ['midi', () => transport.updateMidi(snapshot)],
    ['scenes', () => stageView.updateScenes(snapshot)],
    ['arrangements', () => stageView.updateArrangements(snapshot)],
    ['pending', () => stageView.updatePending(snapshot)],
    ['notices', () => stageView.updateNotices(snapshot)],
    ['tools', () => updateToolsProblem(snapshot)],
    ['lanes', () => laneView.update(snapshot)],
    ['highlights', () => highlighter.update(snapshot)],
    ['status', () => updateClipCount(snapshot)],
    ['state', () => updateEvalState(snapshot)],
  ];
  for (const [name, render] of parts) {
    try {
      render();
    } catch (error) {
      console.error(`drawing the ${name} failed`, error);
      editorStatus.textContent = `drawing the ${name} failed: ${error}`;
    }
  }
}

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

/** The port other tools reach the stage on can't be opened: say why, and keep the port and token fields in reach. */
let portShown = false;
function updateToolsProblem(snapshot: Snapshot) {
  if (!portShown && snapshot.tools_port) {
    input('port').value = String(snapshot.tools_port);
    portShown = true;
  }
  const msg = snapshot.tools_problem ?? '';
  $('tools-problem').textContent = msg;
  $('tools-problem').title = msg;
  $('setup').hidden = connected && !msg;
}

async function reconnect() {
  try {
    // other tools use the port and token as they are now, and any socket that was not open is opened
    await ensureConnected();
    await stage.toolsApply(Number(input('port').value) || 7400, input('token').value.trim());
    status('other tools reach the stage on the port above');
  } catch (error) {
    status(String(error));
  }
}

let polls = 0;
async function poll() {
  await refresh();
  // an engine that has gone is looked for every couple of seconds
  if (linkLost && ++polls % 25 === 0) {
    try {
      await stage.startEngine();
    } catch {
      // still not there
    }
  }
  // a hidden window draws nothing, so it only needs to keep its place
  setTimeout(poll, document.hidden ? 500 : moving ? 90 : 250);
}

loadLook();
const audioSettings = createAudioSettings({
  ensureConnected,
  ask,
  status,
  onApplied: () => {
    stageView.resetSignatures();
    lastClipCount = -1;
  },
});
const settings = settingsPage({ sections: [audioSettings.panel], onOpen: () => void audioSettings.refresh() });
document.body.append(settings.element);
audioSettings.bind();

$('banner').addEventListener('click', () => showBanner([]));
$('evaluate').addEventListener('click', () => void evaluate());
$('evaluate-file').addEventListener('click', () => void evaluate('file'));
$('evaluate-all').addEventListener('click', () => void workbench.evaluateAll());
$('hush').addEventListener('click', () => void act({ action: 'hush' }));
$('panic').addEventListener('click', () => void act({ action: 'panic' }));
$('reconnect').addEventListener('click', () => void reconnect());

window.addEventListener('keydown', event => {
  const mod = event.metaKey || event.ctrlKey;
  if (!mod) {
    if (event.key === 'Escape') {
      if (settings.isOpen) settings.close();
      else showBanner([]);
    }
    return;
  }
  if (event.key === ',') {
    event.preventDefault();
    settings.toggle();
  } else if (event.key === '=' || event.key === '+') {
    event.preventDefault();
    layout.stepZoom(1);
  } else if (event.key === '-') {
    event.preventDefault();
    layout.stepZoom(-1);
  } else if (event.key === '0') {
    event.preventDefault();
    layout.setZoom(1);
  } else if (event.key.toLowerCase() === 'n' && event.shiftKey) {
    // the same as running `new`: clear the session's clips, scenes and the rest
    event.preventDefault();
    void newSession();
  } else if (event.key.toLowerCase() === 't' && !event.shiftKey) {
    event.preventDefault();
    workbench.newTab();
  } else if (event.shiftKey && (event.key === '[' || event.key === '{')) {
    event.preventDefault();
    workbench.step(-1);
  } else if (event.shiftKey && (event.key === ']' || event.key === '}')) {
    event.preventDefault();
    workbench.step(1);
  } else if (event.key.toLowerCase() === 'b' && !event.shiftKey) {
    event.preventDefault();
    layout.toggleRail();
  } else if (/^[1-9]$/.test(event.key) && !event.shiftKey) {
    // the nth scene, as clicking it would
    const scene = stageView.sceneAt(Number(event.key) - 1);
    if (scene) {
      event.preventDefault();
      void stageView.launch(scene);
    }
  }
  if (event.key === '.') {
    event.preventDefault();
    void act({ action: 'hush' });
  }
  if (event.key === '>') {
    event.preventDefault();
    void act({ action: 'panic' });
  }
  if (event.key === 'Enter' && !(event.target instanceof HTMLElement && event.target.closest('.cm-editor'))) {
    event.preventDefault();
    void evaluate();
  }
});

/** The menu bar: each item runs what its button or key already does. */
const menuActions: Record<string, () => void> = {
  new: workbench.newTab,
  'new-session': () => void newSession(),
  open: () => void workbench.openFile(),
  'open-project': () => void workbench.openProject(),
  save: () => void workbench.saveFile(),
  'save-all': () => void workbench.saveAll(),
  'close-tab': () => void workbench.closeTab(),
  undo: () => workbench.view && undo(workbench.view),
  redo: () => workbench.view && redo(workbench.view),
  sidebar: layout.toggleRail,
  'zoom-in': () => layout.stepZoom(1),
  'zoom-out': () => layout.stepZoom(-1),
  'zoom-reset': () => layout.setZoom(1),
  settings: settings.toggle,
  'previous-tab': () => workbench.step(-1),
  'next-tab': () => workbench.step(1),
};
if (isTauri()) {
  listen<string>('menu', event => menuActions[event.payload]?.()).catch(error => status(`The menu is not connected: ${error}`));
}

workbench.restoreTabs();
setConnection(false);
void connectLanguageServer(message => status(message));
void ensureConnected();
void poll();
