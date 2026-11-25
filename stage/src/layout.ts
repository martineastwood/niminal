import { divider } from './divider';
import { mod } from './shortcuts';

export const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
export const input = (id: string) => $<HTMLInputElement>(id);

/** Set text only when it differs, so an unchanged label costs no layout. */
export function setText(node: HTMLElement, value: string) {
  if (node.textContent !== value) node.textContent = value;
}

export function renderShell(app: HTMLElement) {
  // The browser's own right-click menu (reload, inspect…) is not part of the app: only text fields keep theirs, for cut, copy and paste.
  document.addEventListener('contextmenu', event => {
    if (!(event.target as HTMLElement).closest?.('input, textarea, .cm-editor')) event.preventDefault();
  });

  app.innerHTML = `
  <header id="topbar">
    <div class="brand">
      <span class="mark">◈</span>
      <span class="name">niminal</span>
      <div class="brand-text">
        <span id="connection" class="connection offline"><i></i><span>engine offline</span></span>
      </div>
    </div>
    <div class="transport" id="transport">
      <div class="position"><span class="eyebrow">bar</span><strong id="bar">1</strong><span class="beat" id="beat">1.1</span><span class="beats" id="beats"></span></div>
      <div class="readout"><span class="eyebrow">tempo</span><span id="bpm">—</span><small>bpm</small><small>·</small><span id="meter">4/4</span></div>
      <div class="readout"><span class="eyebrow">out</span><span id="master" class="master" title="Output level in dB, peak held; click to clear the clip light"></span></div>
      <div class="readout" id="health-readout" hidden><span class="eyebrow">engine</span><span id="health" class="health ok">ok</span></div>
      <div class="readout" id="midi-readout" hidden><span class="eyebrow">midi</span><button id="midi" class="midi" title="Look for MIDI inputs again">—</button></div>
      <div class="readout" id="sync-readout"><span class="eyebrow">sync</span>
        <select id="clock-in" class="sync" title="Follow the MIDI clock of an input: its tempo, and bar 1 where it starts"><option value="">follow: off</option></select>
        <select id="clock-out" class="sync" title="Send MIDI clock, with start and stop, to an output"><option value="">send: off</option></select>
      </div>
      <div class="readout"><span class="eyebrow">voices</span><span id="voices">0</span></div>
      <div class="readout" id="waiting-readout" hidden><span class="eyebrow">waiting</span><span id="waiting">0</span></div>
    </div>
    <div class="top-actions">
      <span class="setup" id="setup">
        <label class="port" title="The engine runs inside the stage. The VS Code extension plays through the stage on this port; other sequencers reach the engine on the next one up.">tools port <input id="port" inputmode="numeric" value="7400" /></label>
        <input id="token" type="password" placeholder="token" aria-label="token other tools must give" />
        <button id="reconnect" title="Use this port and token, and start the engine again if it stopped">Apply</button>
        <span id="tools-problem" class="tools-problem"></span>
      </span>
      <button id="hush" title="Stop every clip, letting what sounds ring out">Hush</button>
      <button id="panic" class="danger" title="Stop everything now">Panic</button>
      <button id="rec" title="Mark the start of a replay; press again to end it. Nothing is captured as it happens: the session is logged, and the log is played back to audio afterwards">Replay</button>
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
          </span>
          <label class="quantize" title="When an evaluation lands">land <input id="quantize" placeholder="next 4bars" /></label>
          <button id="evaluate-all" title="Evaluate every tab, left to right" hidden>All</button>
          <button id="evaluate-file" title="Evaluate the whole tab (Cmd/Ctrl+Shift+Enter)">File <kbd>⇧⌘↩</kbd></button>
          <button id="evaluate" class="primary" title="Evaluate the selection, or the statement the cursor is in (Cmd/Ctrl+Enter)">Evaluate <kbd>⌘↩</kbd></button>
        </div>
      </div>
      <div id="tabs" class="tabs" role="tablist"></div>
      <div id="banner" role="alert" hidden></div>
      <div id="workbench">
        <div id="editor"></div>
      </div>
      <div id="problems" aria-live="polite"></div>
      <div id="editor-status"></div>
    </section>

    <div id="divider" role="separator" aria-orientation="vertical" aria-label="Resize the sidebar" tabindex="0" title="Drag to resize · double-click to reset"></div>

    <section id="stage">
      <div class="stage-head">
        <div class="scenes" id="scenes"></div>
        <div class="arrangements" id="arrangements"></div>
        <div class="stage-hint" id="stage-hint"></div>
      </div>
      <div class="stage-scroll"><div id="lanes"></div></div>
      <div id="scope" class="scope"></div>
      <div id="pending" class="pending"></div>
      <div id="notices" class="notices"></div>
      <div id="take" class="take" hidden></div>
    </section>
  </main>

  <footer>
    <span id="footer-status">starting…</span>
    <span class="spacer"></span>
    <span class="hint">${mod}↩ run block · ⇧${mod}↩ run file · ${mod}1…9 scenes · ${mod}. hush</span>
  </footer>
`;
}

// Text size: one scale for the whole window, so the code, the sidebar and the controls grow together.
const ZOOMS = [0.7, 0.85, 1, 1.15, 1.3, 1.5, 1.75, 2, 2.5, 3];
const zoomKey = 'niminal-stage-zoom';
const modeKey = 'niminal-stage-mode';
const railKey = 'niminal-stage-rail';

export function setupLayout(onMeasure: () => void) {
  let zoom = 1;
  function setZoom(value: number) {
    zoom = Math.min(Math.max(value, ZOOMS[0]), ZOOMS[ZOOMS.length - 1]);
    document.documentElement.style.setProperty('--z', String(zoom));
    localStorage.setItem(zoomKey, String(zoom));
    onMeasure();
  }
  function stepZoom(direction: 1 | -1) {
    const next = direction > 0 ? ZOOMS.find(z => z > zoom + 1e-6) : [...ZOOMS].reverse().find(z => z < zoom - 1e-6);
    if (next !== undefined) setZoom(next);
  }
  setZoom(Number(localStorage.getItem(zoomKey)) || 1);

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

  // The sidebar can be hidden to give the code the whole window; the choice is remembered.
  function setSidebarHidden(hidden: boolean) {
    document.body.classList.toggle('no-rail', hidden);
    localStorage.setItem(modeKey, hidden ? 'no-rail' : '');
    onMeasure();
  }
  setSidebarHidden((localStorage.getItem(modeKey) ?? '').split(',').includes('no-rail'));
  const toggleRail = () => setSidebarHidden(!document.body.classList.contains('no-rail'));

  // The divider between the code and the sidebar: drag it, or use the arrow keys, to give either more room.
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
  divider($('divider'), -1, () => $('stage').getBoundingClientRect().width, setRail, () => setRail(null));

  return { setZoom, stepZoom, toggleRail };
}
