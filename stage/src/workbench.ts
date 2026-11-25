import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { blockAround, nextStatement, paragraphAround } from '../../editors/codemirror/blocks';
import { demoSource } from './demo';
import { Scope, niminalEditor } from './editor';
import { Highlighter } from './highlight';
import { $ } from './layout';
import { learnedRange } from './learn';
import { OpenedFile, Problem, stage } from './live';
import { dropInto } from './samples';
import { StoredTabs, loadTabs, moveItem, tabTitle, tabsKey } from './tabs';

/** A file in the window: each has its own editor state, so its cursor, undo history and highlights are its own. */
export type Tab = { id: number; path: string | null; title: string; state: EditorState; saved: string };

function uriFor(path: string) {
  return `file://${path
    .split('/')
    .map(encodeURIComponent)
    .join('/')}`;
}

export function createWorkbench(deps: {
  evaluate: (scope?: Scope) => Promise<void>;
  showProblems: (list: Problem[]) => void;
  status: (message: string) => void;
  ask: (message: string) => Promise<boolean>;
  highlighter: Highlighter;
}) {
  let view: EditorView | null = null;
  let tabs: Tab[] = [];
  let current: Tab;
  let tabCount = 0;
  let dragging: Tab | null = null;

  /** The code an evaluation sends and where it begins in the editor: the selection, else the statement the cursor is in (the paragraph around it for `paragraph`). */
  function chunk(scope: Scope) {
    if (!view) return { text: demoSource, from: 0, to: 0 };
    const doc = view.state.doc;
    if (scope === 'file') return { text: doc.toString(), from: 0, to: doc.length };
    const selection = view.state.selection.main;
    if (!selection.empty) {
      return { text: view.state.sliceDoc(selection.from, selection.to), from: selection.from, to: selection.to };
    }
    const lines = doc.toString().split('\n');
    const block = (scope === 'paragraph' ? paragraphAround : blockAround)(lines, doc.lineAt(selection.head).number - 1);
    if (!block) return null;
    const from = doc.line(block.start + 1).from;
    const to = doc.line(block.end + 1).to;
    return { text: view.state.sliceDoc(from, to), from, to };
  }

  /** Move the cursor to the start of the next statement after what was evaluated (`to`), or after the cursor's line when there was nothing to evaluate. */
  function stepOn(to: number | null) {
    if (!view) return;
    const doc = view.state.doc;
    const line = doc.lineAt(to ?? view.state.selection.main.head).number - 1;
    const next = nextStatement(doc.toString().split('\n'), line);
    if (next === null) return;
    view.dispatch({ selection: { anchor: doc.line(next + 1).from }, scrollIntoView: true });
  }

  function makeTab(source: string, path: string | null, title: string, saved = source): Tab {
    const id = ++tabCount;
    const uri = uriFor(path ?? `/niminal-stage/${title}-${id}.nml`);
    const tab: Tab = {
      id,
      path,
      title: tabTitle(path, title),
      saved,
      state: niminalEditor(source, uri, scope => void deps.evaluate(scope), (state, docChanged) => {
        tab.state = state;
        if (docChanged) changed(id);
      }),
    };
    return tab;
  }

  /** The directory `use` and samples are found from: the folder of the file in the current tab. */
  function fileDir() {
    return current.path ? current.path.replace(/[\\/][^\\/]*$/, '') : null;
  }

  function show(tab: Tab) {
    current = tab;
    if (view) view.setState(tab.state);
    else view = new EditorView({ state: tab.state, parent: $('editor') });
    deps.highlighter.reset();
    deps.showProblems([]);
    renderTabs();
    persist();
    view.focus();
  }

  function addTab(source: string, path: string | null, title: string) {
    const tab = makeTab(source, path, title);
    tabs.push(tab);
    show(tab);
  }

  async function closeTab(tab: Tab = current) {
    if (isDirty(tab) && !(await deps.ask(`${tab.title} has changes that are not saved. Close it anyway?`))) return;
    const index = tabs.indexOf(tab);
    if (index < 0) return;
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
    return tab.state.doc.toString() !== tab.saved;
  }

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
          void closeTab(tab);
        });
        item.append(name, close);
        item.addEventListener('click', () => tab !== current && show(tab));
        // a middle click closes, as in a browser
        item.addEventListener('auxclick', event => event.button === 1 && void closeTab(tab));
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
    const stored: StoredTabs = {
      tabs: tabs.map(tab => ({ path: tab.path, title: tab.title, text: tab.state.doc.toString(), saved: tab.saved })),
      active: tabs.indexOf(current),
    };
    localStorage.setItem(tabsKey, JSON.stringify(stored));
  }

  function writeLearned(control: string, source: string) {
    for (const tab of tabs) {
      const doc = tab.state.doc.toString();
      const range = learnedRange(doc, control);
      if (!range) continue;
      const changes = { from: range.from, to: range.to, insert: source };
      if (tab === current && view) view.dispatch({ changes });
      else tab.state = tab.state.update({ changes }).state;
      deps.status(`${control} learned ${source}`);
      persist();
      return;
    }
  }

  /** Show the file at `path` in its tab, opening one with what `read` gives if it has none. */
  async function openTab(path: string, read: () => Promise<OpenedFile>) {
    const open = tabs.find(tab => tab.path === path);
    if (open) show(open);
    else addTab((await read()).source, path, 'untitled');
    deps.status(`opened ${path}; evaluate it when you are ready`);
  }

  async function openFile() {
    try {
      const opened = await stage.openFile();
      if (opened) await openTab(opened.path, async () => opened);
    } catch (error) {
      deps.status(String(error));
    }
  }

  /** Open one file from the file browse panel in a tab, or switch to it if it is already open. */
  async function openPath(path: string) {
    try {
      await openTab(path, () => stage.openPath(path));
    } catch (error) {
      deps.status(String(error));
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
      deps.status(`opened ${project.files.length} file${project.files.length === 1 ? '' : 's'} from ${project.dir}; ${what}`);
    } catch (error) {
      deps.status(String(error));
    }
  }

  /** What the file holds now, or null if it can't be read (it may have been moved or deleted). */
  async function diskText(path: string): Promise<string | null> {
    try {
      return (await stage.openPath(path)).source;
    } catch {
      return null;
    }
  }

  /** Put `text` in a tab in place of what it holds, keeping the cursor near where it was. */
  function replaceText(tab: Tab, text: string) {
    const changes = { from: 0, to: tab.state.doc.length, insert: text };
    if (tab === current && view) {
      view.dispatch({ changes, selection: { anchor: Math.min(view.state.selection.main.head, text.length) } });
    } else {
      tab.state = tab.state.update({ changes }).state;
    }
  }

  /** The text of each file that changed on disk and that its tab has already been told about. */
  const toldAbout = new Map<Tab, string>();
  let checkingDisk = false;

  /** Another program (an editor, a sync tool) may have changed the files in the tabs. A tab with no edits of its
   *  own follows the file; one with edits is told, and asks before it replaces the file. */
  async function checkDisk() {
    if (checkingDisk) return;
    checkingDisk = true;
    try {
      for (const tab of [...tabs]) {
        if (!tab.path) continue;
        const disk = await diskText(tab.path);
        if (disk === null || disk === tab.saved) {
          toldAbout.delete(tab);
        } else if (!isDirty(tab)) {
          replaceText(tab, disk);
          tab.saved = disk;
          toldAbout.delete(tab);
          renderTabs();
          persist();
          deps.status(`${tab.title} changed on disk, so it was reloaded`);
        } else if (toldAbout.get(tab) !== disk) {
          toldAbout.set(tab, disk);
          deps.status(`${tab.title} changed on disk and here. Saving will ask before it replaces the file.`);
        }
      }
    } finally {
      checkingDisk = false;
    }
  }
  setInterval(() => void (document.hasFocus() && checkDisk()), 1500);
  window.addEventListener('focus', () => void checkDisk());

  async function saveFile(tab = current) {
    if (!view) return;
    const text = tab.state.doc.toString();
    try {
      if (tab.path) {
        const disk = await diskText(tab.path);
        if (
          disk !== null &&
          disk !== tab.saved &&
          !(await deps.ask(`${tab.title} changed on disk since it was opened here. Replace what is there with this version?`))
        ) {
          return;
        }
      }
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
      deps.status(`saved ${tab.path}`);
    } catch (error) {
      deps.status(String(error));
    }
  }

  /** Save every tab that has a file and has changed. */
  async function saveAll() {
    for (const tab of tabs) if (tab.path && isDirty(tab)) await saveFile(tab);
  }

  /** Send every tab in turn, left to right, so a tab of instruments goes before the clips that play them. */
  async function evaluateAll() {
    for (const tab of tabs) {
      if (tab !== current) show(tab);
      await deps.evaluate('file');
    }
  }

  /** Files dropped on the window: audio goes into the code where it lands, as `sample("path")`, and a piece opens
   *  in a tab. Tauri reports the drop in physical pixels over the whole window, so the editor is found from that. */
  function overEditor(x: number, y: number) {
    if (!view) return false;
    const box = view.dom.getBoundingClientRect();
    return x >= box.left && x <= box.right && y >= box.top && y <= box.bottom;
  }
  function listenForDrops() {
    void getCurrentWebview()
      .onDragDropEvent(event => {
        const drop = event.payload;
        if (drop.type === 'leave') {
          view?.dom.classList.remove('dropping');
          return;
        }
        const x = drop.position.x / window.devicePixelRatio;
        const y = drop.position.y / window.devicePixelRatio;
        if (drop.type !== 'drop') {
          view?.dom.classList.toggle('dropping', overEditor(x, y));
          return;
        }
        view?.dom.classList.remove('dropping');
        for (const path of drop.paths.filter(p => /\.nml$/i.test(p))) void openPath(path);
        if (!view) return;
        const told = dropInto(view, drop.paths, overEditor(x, y) ? view.posAtCoords({ x, y }, false) : null, fileDir());
        if (told) deps.status(told);
      })
      .catch(error => deps.status(`Dropping files is not connected: ${error}`));
  }
  if (isTauri()) {
    try {
      listenForDrops();
    } catch (error) {
      // a window without the native side (a test, a plain browser) has nothing to drop onto
      console.warn('dropping files is not available', error);
    }
  }

  function restoreTabs() {
    const stored = loadTabs(localStorage.getItem(tabsKey));
    if (stored) {
      tabs = stored.tabs.map(tab => makeTab(tab.text, tab.path, tab.title, tab.saved));
      show(tabs[stored.active]);
    } else {
      addTab(demoSource, null, 'demo');
    }
  }

  return {
    get view() {
      return view;
    },
    get current() {
      return current;
    },
    chunk,
    stepOn,
    fileDir,
    newTab: () => addTab('', null, 'untitled'),
    closeTab,
    step,
    openFile,
    openProject,
    saveFile,
    saveAll,
    evaluateAll,
    writeLearned,
    restoreTabs,
  };
}
