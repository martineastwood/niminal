import { closeMatrixMenu, matrixCell } from './matrix';
import { draggable, dropTarget } from './drag';
import { openMenu, type MenuEntry } from './menu';
import { dropUnusedPatterns, makePattern, tidyMatrix, trackSource, type Pattern, type Project, type Section } from './project';

const pad = (n: number) => String(n).padStart(2, '0');

export class Arrangement {
  position = 0;
  /** Steps cannot be added, moved, removed or repointed while the song plays */
  private locked = false;
  private showCells = false;
  private closeMenu = () => {};

  constructor(private parent: HTMLElement, private project: () => Project, private selected: (manual: boolean) => void, private edited: () => void) {}

  get pattern(): Pattern { return this.project().patterns.find(p => p.id === this.project().sequence[this.position])!; }

  reset() { this.position = 0; this.render(); }

  select(position: number, manual = true) {
    this.position = position;
    this.render(); this.selected(manual);
  }

  insert(kind: 'new' | 'variation' | 'reuse') {
    const project = this.project();
    if (project.sequence.length >= 1024 || (kind !== 'reuse' && project.patterns.length >= 128)) return;
    const pattern = kind === 'reuse' ? this.pattern : makePattern(project, kind === 'variation' ? this.pattern : undefined);
    project.sequence.splice(this.position + 1, 0, pattern.id);
    // A step added before the loop pushes it along; one added inside it joins the loop.
    const loop = project.loop_range;
    if (loop && this.position + 1 <= loop.start) { loop.start++; loop.end++; } else if (loop && this.position + 1 <= loop.end) loop.end++;
    if (project.matrix && this.position + 1 < project.matrix.length) project.matrix.splice(this.position + 1, 0, []);
    // Sections that began at or after the new step move down with their steps; the new step joins the section before it.
    for (const section of project.sections ?? []) if (section.start >= this.position + 1) section.start++;
    const joined = this.sectionAt(this.position + 1); if (joined) delete joined.collapsed;
    this.select(this.position + 1); this.edited();
  }

  /** Points a step at another pattern; the one it played goes if nothing else uses it. */
  repoint(index: number, id: string) {
    const project = this.project();
    if (project.sequence[index] === id) return;
    project.sequence[index] = id; dropUnusedPatterns(project); this.select(index); this.edited();
  }

  /** The section a step belongs to, if it is after the first section's start. */
  private sectionAt(position: number): Section | undefined {
    return (this.project().sections ?? []).filter(s => s.start <= position).at(-1);
  }

  /** One past a section's last step. */
  private sectionEnd(section: Section) {
    const project = this.project();
    return (project.sections ?? []).find(s => s.start > section.start)?.start ?? project.sequence.length;
  }

  /** Starts a section at the selected step, or removes the one that starts there. */
  toggleSection(position = this.position) {
    const project = this.project(), sections = project.sections ??= [];
    const existing = sections.findIndex(s => s.start === position);
    if (existing >= 0) sections.splice(existing, 1);
    else { sections.push({ start: position, name: `Section ${sections.length + 1}` }); sections.sort((a, b) => a.start - b.start); }
    if (!sections.length) delete project.sections;
    this.render(); this.edited();
  }

  /** Loops a section, or clears the loop if it already loops exactly that section. */
  loopSection(section: Section) {
    const project = this.project(), loop = project.loop_range, end = this.sectionEnd(section) - 1;
    if (loop && loop.start === section.start && loop.end === end) delete project.loop_range; else project.loop_range = { start: section.start, end };
    this.render(); this.edited();
  }

  /** Called before the selected step is removed: later sections move up, and one left with no steps goes. */
  private removeFromSections() {
    const project = this.project(), sections = project.sections;
    if (!sections) return;
    const q = this.position, emptied = new Set(sections.filter(s => s.start === q && this.sectionEnd(s) - s.start <= 1));
    project.sections = sections.filter(s => !emptied.has(s));
    for (const section of project.sections) if (section.start > q) section.start--;
    if (!project.sections.length) delete project.sections;
  }

  /** Loops the selected step, or clears the loop. */
  toggleLoop() {
    const project = this.project();
    if (project.loop_range) delete project.loop_range; else project.loop_range = { start: this.position, end: this.position };
    this.render(); this.edited();
  }

  /** Loops one step, or clears the loop if it already loops just that step. */
  loopStep(position: number) {
    const project = this.project(), loop = project.loop_range;
    if (loop && loop.start === position && loop.end === position) delete project.loop_range; else project.loop_range = { start: position, end: position };
    this.render(); this.edited();
  }

  /** Loops from the selected step to `position`, in either direction. */
  extendLoop(position: number) {
    this.project().loop_range = { start: Math.min(this.position, position), end: Math.max(this.position, position) };
    this.render(); this.edited();
  }

  move(delta: number) { this.moveTo(this.position, this.position + delta); }

  /** Moves a step to another place in the sequence. The loop and the sections stay where they are; a step's mutes and redirects go with it. */
  moveTo(from: number, to: number) {
    const project = this.project(), sequence = project.sequence;
    if (this.locked || from === to || from < 0 || to < 0 || from >= sequence.length || to >= sequence.length) return;
    sequence.splice(to, 0, ...sequence.splice(from, 1));
    const matrix = project.matrix;
    if (matrix) { while (matrix.length < sequence.length) matrix.push([]); matrix.splice(to, 0, ...matrix.splice(from, 1)); tidyMatrix(project); }
    this.select(to); this.edited();
  }

  remove() {
    const sequence = this.project().sequence;
    if (sequence.length <= 1) return;
    this.removeFromSections();
    const project = this.project();
    if (project.matrix && this.position < project.matrix.length) { project.matrix.splice(this.position, 1); tidyMatrix(project); }
    sequence.splice(this.position, 1); dropUnusedPatterns(this.project());
    const loop = this.project().loop_range;
    if (loop && this.position < loop.start) { loop.start--; loop.end--; } else if (loop && this.position <= loop.end) { if (loop.start === loop.end) delete this.project().loop_range; else loop.end--; }
    this.select(Math.min(this.position, sequence.length - 1)); this.edited();
  }


  /** Shows or hides the cell for each track on every step. */
  setCells(on: boolean) { this.showCells = on; this.render(); }

  private open(anchor: HTMLElement, entries: MenuEntry[]) { this.closeMenu(); this.closeMenu = openMenu(anchor, entries); }

  private sectionEntry(position: number): MenuEntry {
    const has = (this.project().sections ?? []).some(s => s.start === position);
    return { text: has ? 'Remove the section that starts here' : 'Start a section here', run: () => this.toggleSection(position) };
  }

  /** What the + Add button offers: a new step after the selected one, or a section. */
  addMenu(anchor: HTMLElement) {
    this.open(anchor, [
      ...(this.locked ? [{ heading: 'STOP PLAYBACK TO ADD STEPS' }] : [
        { text: 'New blank pattern', run: () => this.insert('new') },
        { text: 'Copy of this pattern (edited on its own)', run: () => this.insert('variation') },
        { text: 'Repeat this pattern (edits are shared)', run: () => this.insert('reuse') },
      ]),
      { heading: 'SECTION' }, this.sectionEntry(this.position),
    ]);
  }

  /** The actions for one step: the same menu from its ⋯ button and from a right-click. */
  private stepMenu(anchor: HTMLElement, index: number) {
    const project = this.project(), loop = project.loop_range, looped = !!loop && loop.start === index && loop.end === index;
    const edit = (run: () => void) => () => { this.select(index); run(); };
    this.open(anchor, [
      ...(this.locked ? [{ heading: 'STOP PLAYBACK TO EDIT STEPS' }] : [
        ...(index > 0 ? [{ text: 'Move up', run: edit(() => this.move(-1)) }] : []),
        ...(index < project.sequence.length - 1 ? [{ text: 'Move down', run: edit(() => this.move(1)) }] : []),
        ...(project.sequence.length > 1 ? [{ text: 'Remove step', run: edit(() => this.remove()) }] : []),
      ]),
      { text: 'Rename pattern', run: () => this.rename(index) },
      { text: looped ? 'Clear the loop' : 'Loop this step', run: () => this.loopStep(index) },
      this.sectionEntry(index),
      ...(this.locked ? [] : [
        { heading: 'PLAY PATTERN' } as MenuEntry,
        ...project.patterns.map((p, i) => ({ text: `P${pad(i)} ${p.name}`, checked: p.id === project.sequence[index], run: () => this.repoint(index, p.id) })),
      ]),
    ]);
  }

  /** Swaps a step's label for a text box that renames its pattern: Enter or a click elsewhere keeps the name, Escape drops it. */
  rename(index: number) {
    const project = this.project(), pattern = project.patterns.find(p => p.id === project.sequence[index]);
    const main = this.parent.querySelector<HTMLElement>(`.sequence-entry[data-position="${index}"] .sequence-main`);
    if (!pattern || !main) return;
    const input = document.createElement('input'); input.className = 'sequence-rename'; input.value = pattern.name; input.maxLength = 60; input.setAttribute('aria-label', `Name of pattern ${pad(project.patterns.indexOf(pattern))}`);
    let done = false;
    const finish = (keep: boolean) => {
      if (done) return; done = true;
      const name = input.value.trim();
      if (keep && name && name !== pattern.name) { pattern.name = name; this.render(); this.selected(false); this.edited(); } else this.render();
    };
    input.onkeydown = event => { event.stopPropagation(); if (event.key === 'Enter') finish(true); else if (event.key === 'Escape') finish(false); };
    input.onblur = () => finish(true);
    main.replaceWith(input); input.focus(); input.select();
  }

  private sectionMenu(anchor: HTMLElement, section: Section) {
    const project = this.project(), end = this.sectionEnd(section) - 1, looped = project.loop_range?.start === section.start && project.loop_range.end === end;
    this.open(anchor, [
      { text: section.collapsed ? 'Expand' : 'Collapse', run: () => this.collapse(section) },
      { text: looped ? 'Clear the loop' : 'Loop this section', run: () => this.loopSection(section) },
      { text: 'Remove section (keeps its steps)', run: () => this.toggleSection(section.start) },
    ]);
  }

  private collapse(section: Section) {
    if (section.collapsed) delete section.collapsed; else section.collapsed = true;
    this.render(); this.edited();
  }

  /** Dragging down the strip at a row's left edge loops the steps it passes over; a click loops that one step. */
  private loopGutter(index: number) {
    const gutter = document.createElement('div'); gutter.className = 'loop-gutter';
    gutter.title = 'Click to loop this step; drag over other steps to loop a range';
    gutter.onpointerdown = down => {
      if (down.button !== 0) return;
      down.preventDefault();
      let end = index, moved = false;
      const move = (event: PointerEvent) => {
        const row = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>('.sequence-entry');
        if (!row || Number(row.dataset.position) === end) return;
        end = Number(row.dataset.position); moved = true;
        this.project().loop_range = { start: Math.min(index, end), end: Math.max(index, end) }; this.render();
      };
      const up = () => {
        document.removeEventListener('pointermove', move, true); document.removeEventListener('pointerup', up, true);
        if (moved) { this.render(); this.edited(); } else this.loopStep(index);
      };
      document.addEventListener('pointermove', move, true); document.addEventListener('pointerup', up, true);
    };
    return gutter;
  }

  render() {
    const project = this.project();
    this.closeMenu(); closeMatrixMenu();
    this.parent.replaceChildren();
    // the sidebar makes room for a cell for each track
    this.parent.parentElement?.style.setProperty('--sequence-cells', this.showCells ? `${project.tracks.length * 17}px` : '0px');
    let hidden = false;
    for (const [index, id] of project.sequence.entries()) {
      const section = project.sections?.find(s => s.start === index);
      if (section) { this.parent.append(this.header(section)); hidden = !!section.collapsed; }
      if (hidden) continue;
      const pattern = project.patterns.find(p => p.id === id)!;
      const number = pad(project.patterns.indexOf(pattern));
      const row = document.createElement('div'); row.className = 'sequence-entry'; row.dataset.position = String(index);
      row.classList.toggle('selected', index === this.position);
      const loop = project.loop_range, inLoop = !!loop && index >= loop.start && index <= loop.end;
      row.classList.toggle('in-loop', inLoop); row.classList.toggle('loop-start', inLoop && index === loop.start); row.classList.toggle('loop-end', inLoop && index === loop.end);
      const overridden = project.tracks.some((_, track) => trackSource(project, index, track));
      const main = document.createElement('button'); main.className = 'sequence-main'; main.setAttribute('aria-label', `Sequence step ${index + 1}: ${pattern.name}`);
      main.title = `Step ${index + 1}, pattern ${number}, used ${project.sequence.filter(ref => ref === id).length} times. Click to edit, drag to reorder, Shift-click to loop from the selected step to this one. Double-click to rename. Right-click for more.`
        + (overridden ? ' Some tracks are muted or play another pattern here.' : '');
      for (const [className, text] of [['step', pad(index + 1)], ['name', pattern.name], ['pattern', `P${number}`]]) { const span = document.createElement('span'); span.className = className; span.textContent = text; main.append(span); }
      if (overridden && !this.showCells) { const flag = document.createElement('span'); flag.className = 'flag'; flag.textContent = '•'; main.append(flag); }
      const menu = document.createElement('button'); menu.className = 'row-menu'; menu.textContent = '⋯'; menu.title = 'Step actions'; menu.setAttribute('aria-label', `Actions for step ${index + 1}`);
      menu.onclick = () => this.stepMenu(menu, index);
      row.onclick = event => { if ((event.target as HTMLElement).closest('.matrix-cell, .row-menu, .loop-gutter')) return; if (event.shiftKey) this.extendLoop(index); else this.select(index); };
      row.ondblclick = event => { if (!(event.target as HTMLElement).closest('.matrix-cell, .row-menu, .loop-gutter')) this.rename(index); };
      row.oncontextmenu = event => { event.preventDefault(); this.stepMenu(menu, index); };
      draggable(main, () => ({ kind: 'step', value: String(index), name: `${pad(index + 1)} ${pattern.name}` }));
      dropTarget(row, item => item.kind === 'step' && !this.locked, item => this.moveTo(Number(item.value), index));
      row.append(this.loopGutter(index), main);
      if (this.showCells) {
        const cells = document.createElement('div'); cells.className = 'sequence-cells';
        for (let track = 0; track < project.tracks.length; track++) cells.append(matrixCell(project, index, track, () => { this.render(); this.selected(false); this.edited(); }));
        row.append(cells);
      }
      row.append(menu); this.parent.append(row);
    }
  }

  private header(section: Section) {
    const size = this.sectionEnd(section) - section.start;
    const element = document.createElement('div'); element.className = 'sequence-section'; element.dataset.start = String(section.start); element.dataset.end = String(section.start + size);
    element.classList.toggle('selected', !!section.collapsed && this.position >= section.start && this.position < section.start + size);
    const caret = document.createElement('button'); caret.textContent = section.collapsed ? '▸' : '▾'; caret.setAttribute('aria-expanded', String(!section.collapsed));
    const verb = section.collapsed ? 'Expand' : 'Collapse'; caret.title = `${verb} ${section.name}`; caret.setAttribute('aria-label', caret.title);
    caret.onclick = () => this.collapse(section);
    const name = document.createElement('input'); name.value = section.name; name.maxLength = 60; name.setAttribute('aria-label', `Name of the section starting at step ${section.start + 1}`);
    name.onchange = () => { section.name = name.value.trim() || 'Section'; this.render(); this.edited(); };
    const count = document.createElement('span'); count.className = 'sequence-count'; count.textContent = String(size); count.title = `${size} step${size === 1 ? '' : 's'}`;
    const menu = document.createElement('button'); menu.className = 'row-menu'; menu.textContent = '⋯'; menu.title = 'Section actions'; menu.setAttribute('aria-label', `Actions for ${section.name}`);
    menu.onclick = () => this.sectionMenu(menu, section);
    element.oncontextmenu = event => { event.preventDefault(); this.sectionMenu(menu, section); };
    element.append(caret, name, count, menu); return element;
  }

  paint(position: number | null, playing: boolean) {
    if (playing !== this.locked) { this.locked = playing; this.parent.parentElement?.classList.toggle('locked', playing); }
    for (const header of this.parent.querySelectorAll<HTMLElement>('.sequence-section')) header.classList.toggle('playing', position !== null && position >= Number(header.dataset.start) && position < Number(header.dataset.end) && header.querySelector('[aria-expanded=false]') !== null);
    for (const row of this.parent.querySelectorAll<HTMLElement>('.sequence-entry')) row.classList.toggle('playing', position !== null && Number(row.dataset.position) === position);
  }
}
