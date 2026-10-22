import { makePattern, type Pattern, type Project } from './project';

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
    this.select(this.position + 1); this.edited();
  }

  move(delta: number) {
    const sequence = this.project().sequence, next = this.position + delta;
    if (next < 0 || next >= sequence.length) return;
    [sequence[this.position], sequence[next]] = [sequence[next], sequence[this.position]];
    this.select(next); this.edited();
  }

  remove() {
    const sequence = this.project().sequence;
    if (sequence.length <= 1) return;
    sequence.splice(this.position, 1);
    this.select(Math.min(this.position, sequence.length - 1)); this.edited();
  }

  render() {
    const project = this.project();
    this.parent.replaceChildren();
    for (const [index, id] of project.sequence.entries()) {
      const pattern = project.patterns.find(p => p.id === id)!;
      const number = project.patterns.indexOf(pattern).toString().padStart(2, '0');
      const row = document.createElement('div'); row.className = 'sequence-entry'; row.dataset.position = String(index);
      row.classList.toggle('selected', index === this.position);
      const button = document.createElement('button'); button.textContent = String(index + 1).padStart(2, '0'); button.setAttribute('aria-label', `Sequence step ${index + 1}: ${pattern.name}`);
      button.title = `Edit pattern ${number}. Used ${project.sequence.filter(ref => ref === id).length} times in the sequence.`;
      button.onclick = () => this.select(index);
      const select = document.createElement('select'); select.setAttribute('aria-label', `Pattern at sequence step ${index + 1}`); select.dataset.structure = '';
      for (const [i, p] of project.patterns.entries()) select.add(new Option(String(i).padStart(2, '0'), p.id));
      select.value = id;
      const name = document.createElement('button'); name.className = 'sequence-name'; name.textContent = pattern.name; name.title = pattern.name; name.onclick = () => this.select(index);
      select.title = `Pattern ${number}: ${pattern.name}`;
      select.onchange = () => { project.sequence[index] = select.value; this.select(index); this.edited(); };
      row.append(button, select, name); this.parent.append(row);
    }
  }

  paint(position: number | null, playing: boolean) {
    for (const row of this.parent.querySelectorAll<HTMLElement>('.sequence-entry')) row.classList.toggle('playing', position !== null && Number(row.dataset.position) === position);
    for (const select of this.parent.querySelectorAll<HTMLSelectElement>('select')) select.disabled = playing;
  }
}
