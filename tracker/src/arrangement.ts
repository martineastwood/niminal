import { makePattern, tidyMatrix, type Pattern, type Project, type Section } from './project';

export class Arrangement {
  position = 0;
  private patternId = '';

  constructor(private parent: HTMLElement, private project: () => Project, private selected: (manual: boolean) => void, private edited: () => void) {}

  get pattern(): Pattern { return this.project().patterns.find(p => p.id === this.patternId) ?? this.project().patterns.find(p => p.id === this.project().sequence[this.position])!; }
  get number() { return this.project().patterns.indexOf(this.pattern).toString().padStart(2, '0'); }

  reset() { this.position = 0; this.patternId = this.project().sequence[0]; this.render(); }

  select(position: number, manual = true) {
    this.position = position; this.patternId = this.project().sequence[position];
    this.render(); this.selected(manual);
  }

  choose(id: string) {
    if (!this.project().patterns.some(p => p.id === id)) return;
    this.patternId = id; this.render(); this.selected(true);
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

  /** Points the selected step at the previous or next pattern in the pool. */
  shiftPattern(delta: number) {
    const project = this.project(), index = project.patterns.findIndex(p => p.id === project.sequence[this.position]) + delta;
    if (index < 0 || index >= project.patterns.length) return;
    project.sequence[this.position] = project.patterns[index].id; this.select(this.position); this.edited();
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
  toggleSection() {
    const project = this.project(), sections = project.sections ??= [];
    const existing = sections.findIndex(s => s.start === this.position);
    if (existing >= 0) sections.splice(existing, 1);
    else { sections.push({ start: this.position, name: `Section ${sections.length + 1}` }); sections.sort((a, b) => a.start - b.start); }
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

  /** Loops from the selected step to `position`, in either direction. */
  extendLoop(position: number) {
    this.project().loop_range = { start: Math.min(this.position, position), end: Math.max(this.position, position) };
    this.render(); this.edited();
  }

  move(delta: number) {
    const sequence = this.project().sequence, next = this.position + delta;
    if (next < 0 || next >= sequence.length) return;
    [sequence[this.position], sequence[next]] = [sequence[next], sequence[this.position]];
    // A step's mutes and redirects move with it.
    const project = this.project(), matrix = project.matrix;
    if (matrix) { while (matrix.length <= Math.max(this.position, next)) matrix.push([]); [matrix[this.position], matrix[next]] = [matrix[next], matrix[this.position]]; tidyMatrix(project); }
    this.select(next); this.edited();
  }

  remove() {
    const sequence = this.project().sequence;
    if (sequence.length <= 1) return;
    this.removeFromSections();
    const project = this.project();
    if (project.matrix && this.position < project.matrix.length) { project.matrix.splice(this.position, 1); tidyMatrix(project); }
    sequence.splice(this.position, 1);
    const loop = this.project().loop_range;
    if (loop && this.position < loop.start) { loop.start--; loop.end--; } else if (loop && this.position <= loop.end) { if (loop.start === loop.end) delete this.project().loop_range; else loop.end--; }
    this.select(Math.min(this.position, sequence.length - 1)); this.edited();
  }

  render() {
    const project = this.project();
    this.parent.replaceChildren();
    let hidden = false;
    for (const [index, id] of project.sequence.entries()) {
      const section = project.sections?.find(s => s.start === index);
      if (section) { this.parent.append(this.header(section)); hidden = !!section.collapsed; }
      if (hidden) continue;
      const pattern = project.patterns.find(p => p.id === id)!;
      const number = project.patterns.indexOf(pattern).toString().padStart(2, '0');
      const row = document.createElement('div'); row.className = 'sequence-entry'; row.dataset.position = String(index);
      row.classList.toggle('selected', index === this.position);
      const loop = project.loop_range, inLoop = !!loop && index >= loop.start && index <= loop.end;
      row.classList.toggle('in-loop', inLoop); row.classList.toggle('loop-start', inLoop && index === loop.start); row.classList.toggle('loop-end', inLoop && index === loop.end);
      const button = document.createElement('button'); button.textContent = String(index + 1).padStart(2, '0'); button.setAttribute('aria-label', `Sequence step ${index + 1}: ${pattern.name}`);
      button.title = `Edit pattern ${number}. Used ${project.sequence.filter(ref => ref === id).length} times in the sequence. Shift-click loops from the selected step to this one.`;
      button.onclick = event => { if (event.shiftKey) this.extendLoop(index); else this.select(index); };
      const select = document.createElement('select'); select.setAttribute('aria-label', `Pattern at sequence step ${index + 1}`); select.dataset.structure = '';
      for (const [i, p] of project.patterns.entries()) select.add(new Option(String(i).padStart(2, '0'), p.id));
      select.value = id;
      const name = document.createElement('button'); name.className = 'sequence-name'; name.textContent = pattern.name; name.title = pattern.name; name.onclick = () => this.select(index);
      select.title = `Pattern ${number}: ${pattern.name}`;
      select.onchange = () => { project.sequence[index] = select.value; this.select(index); this.edited(); };
      row.append(button, select, name); this.parent.append(row);
    }
  }

  private header(section: Section) {
    const project = this.project(), size = this.sectionEnd(section) - section.start;
    const element = document.createElement('div'); element.className = 'sequence-section'; element.dataset.start = String(section.start); element.dataset.end = String(section.start + size);
    element.classList.toggle('selected', !!section.collapsed && this.position >= section.start && this.position < section.start + size);
    const caret = document.createElement('button'); caret.textContent = section.collapsed ? '▸' : '▾'; caret.setAttribute('aria-expanded', String(!section.collapsed));
    const verb = section.collapsed ? 'Expand' : 'Collapse'; caret.title = `${verb} ${section.name}`; caret.setAttribute('aria-label', caret.title);
    caret.onclick = () => { if (section.collapsed) delete section.collapsed; else section.collapsed = true; this.render(); this.edited(); };
    const name = document.createElement('input'); name.value = section.name; name.maxLength = 60; name.setAttribute('aria-label', `Name of the section starting at step ${section.start + 1}`);
    name.onchange = () => { section.name = name.value.trim() || 'Section'; this.render(); this.edited(); };
    const count = document.createElement('span'); count.className = 'sequence-count'; count.textContent = String(size); count.title = `${size} step${size === 1 ? '' : 's'}`;
    const loop = document.createElement('button'); loop.textContent = '⟲'; loop.title = `Loop ${section.name}`; loop.setAttribute('aria-label', loop.title);
    loop.setAttribute('aria-pressed', String(project.loop_range?.start === section.start && project.loop_range.end === section.start + size - 1)); loop.onclick = () => this.loopSection(section);
    element.append(caret, name, count, loop); return element;
  }

  paint(position: number | null, playing: boolean) {
    for (const header of this.parent.querySelectorAll<HTMLElement>('.sequence-section')) header.classList.toggle('playing', position !== null && position >= Number(header.dataset.start) && position < Number(header.dataset.end) && header.querySelector('[aria-expanded=false]') !== null);
    for (const row of this.parent.querySelectorAll<HTMLElement>('.sequence-entry')) row.classList.toggle('playing', position !== null && Number(row.dataset.position) === position);
    for (const select of this.parent.querySelectorAll<HTMLSelectElement>('select')) select.disabled = playing;
  }
}
