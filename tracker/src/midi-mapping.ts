import { laneParameter, targetKey } from './automation';
import { nearestDivision } from './divisions';
import { openMenu } from './menu';
import { readLfos } from './modulation';
import type { EffectParameters, MidiTarget, Project, Track } from './project';
import type { MidiMessage } from './midi';

const controls = new WeakMap<HTMLElement, { strip: Track; target: MidiTarget }>();
/** Register the actual value controls, excluding record/LFO buttons and macro-link selectors. */
export function midiControl(element: HTMLElement, strip: Track, target: MidiTarget) {
  const inputs = element.matches('input, select, [role=slider]') ? [element] : element.querySelectorAll<HTMLElement>('input[type=range], input.parameter-value, select.division-pick');
  for (const input of inputs) controls.set(input, { strip, target });
}
const strips = (project: Project) => [...project.tracks, ...project.returns, project.master];
interface Options {
  project(): Project;
  parameters(): EffectParameters;
  changed(group?: string): void;
  record(strip: Track, target: MidiTarget, value: number): void;
  refresh(): void;
  feedback(text: string): void;
}

/** Learn mode and project-owned absolute 7-bit CC mappings. */
export class MidiLearn {
  private armed = false;
  private pending: { strip: Track; target: MidiTarget } | null = null;
  private timer = 0;
  private closeMenu: (() => void) | undefined;
  private button = document.createElement('button');
  private list = document.createElement('button');
  constructor(parent: HTMLElement, private options: Options) {
    this.button.textContent = 'MIDI Learn'; this.button.setAttribute('aria-pressed', 'false');
    this.button.title = 'Choose a control, then move a hardware knob. Escape cancels.';
    this.button.onclick = () => {
      if (this.armed) this.cancel();
      else { this.armed = true; this.button.setAttribute('aria-pressed', 'true'); options.feedback('MIDI Learn: click a volume, pan, instrument or effect control, then move a hardware knob.'); }
    };
    this.list.textContent = 'Mappings'; this.list.onclick = () => this.showMappings();
    parent.append(this.button, this.list);
    window.addEventListener('pointerdown', this.choose, true);
    window.addEventListener('click', this.choose, true);
    window.addEventListener('keydown', this.key, true);
  }
  cancel = () => { this.armed = false; this.pending = null; this.button.setAttribute('aria-pressed', 'false'); };
  dispose() {
    this.cancel(); this.closeMenu?.(); clearTimeout(this.timer);
    window.removeEventListener('pointerdown', this.choose, true); window.removeEventListener('click', this.choose, true); window.removeEventListener('keydown', this.key, true);
  }
  private choose = (event: Event) => {
    if (!this.armed) return;
    let input = event.target as HTMLElement | null;
    while (input && !controls.has(input)) input = input.parentElement;
    const chosen = input && controls.get(input);
    if (!chosen || (input as HTMLInputElement).disabled) return;
    event.preventDefault(); event.stopImmediatePropagation();
    this.pending = chosen;
    this.options.feedback('Move the MIDI knob or fader to bind this control. Escape cancels.');
  };
  private key = (event: KeyboardEvent) => {
    if (!this.armed) return;
    if (event.key === 'Escape') { event.preventDefault(); event.stopImmediatePropagation(); this.cancel(); this.options.feedback('MIDI Learn cancelled.'); }
    else if (event.key === 'Enter') this.choose(event);
  };
  receive(message: Extract<MidiMessage, { kind: 'control' }>) {
    if (![message.channel, message.controller, message.value].every(Number.isInteger) || message.channel < 0 || message.channel > 15 || message.controller < 0 || message.controller >= 120 || message.controller === 64 || message.value < 0 || message.value > 127) return;
    const project = this.options.project(), all = strips(project);
    if (this.armed) {
      const chosen = this.pending;
      if (!chosen || !all.includes(chosen.strip)) return;
      const info = laneParameter(chosen.target, all.indexOf(chosen.strip), project, this.options.parameters());
      if (!info || !Number.isFinite(info.min) || !Number.isFinite(info.max) || info.max <= info.min) { this.options.feedback('Apply this instrument or effect before learning its control.'); return; }
      const kept = chosen.strip.midi?.filter(b => !(b.channel === message.channel && b.controller === message.controller) && targetKey(b.target) !== targetKey(chosen.target)).length ?? 0;
      if (kept >= 128) { this.options.feedback('This strip has 128 MIDI mappings; remove one before learning another.'); return; }
      for (const strip of all) {
        strip.midi = strip.midi?.filter(b => !(b.channel === message.channel && b.controller === message.controller) && !(strip === chosen.strip && targetKey(b.target) === targetKey(chosen.target)));
        if (!strip.midi?.length) delete strip.midi;
      }
      (chosen.strip.midi ??= []).push({ channel: message.channel, controller: message.controller, target: { ...chosen.target } });
      this.cancel(); this.options.changed();
      this.options.feedback(`Mapped channel ${message.channel + 1} CC${message.controller} to ${chosen.strip.name} → ${info.label}.`);
      return; // Learning binds without jumping the control to the knob's current position.
    }
    for (const strip of all) {
      const binding = strip.midi?.find(b => b.channel === message.channel && b.controller === message.controller);
      if (!binding) continue;
      const target = binding.target, info = laneParameter(target, all.indexOf(strip), project, this.options.parameters());
      if (!info || !Number.isFinite(info.min) || !Number.isFinite(info.max) || info.max <= info.min) return;
      const device = target.kind === 'effect' ? strip.fx.find(d => d.id === target.device) : undefined;
      if (device && (device.modulations[target.kind === 'effect' ? target.parameter : '']?.enabled || strip.macros?.some(m => m.links.some(l => target.kind === 'effect' && l.device === device.id && l.parameter === target.parameter)))) {
        this.options.feedback('Disable the LFO or macro driving this parameter before controlling it with MIDI.'); return;
      }
      if (target.kind === 'instrument' && readLfos(project.definitions.find(d => d.id === target.instrument)?.source ?? '')[target.parameter]) {
        this.options.feedback('Disable this parameter’s LFO before controlling it with MIDI.'); return;
      }
      let value = target.kind === 'pan' ? message.value <= 64 ? (message.value - 64) / 64 : (message.value - 64) / 63 : info.min + (info.max - info.min) * message.value / 127;
      if (info.division) value = Math.min(info.max, Math.max(info.min, nearestDivision(value).beats));
      if (!Number.isFinite(value)) return;
      if (target.kind === 'volume') strip.volume_db = value;
      else if (target.kind === 'pan') strip.pan = value;
      else if (target.kind === 'effect' && device) device.values[target.parameter] = value;
      else if (target.kind === 'instrument') {
        const definition = project.definitions.find(d => d.id === target.instrument && d.kind === 'instr');
        if (!definition || !project.tracks.includes(strip)) return;
        (definition.values ??= {})[target.parameter] = value;
      } else return;
      this.options.record(strip, target, value);
      this.options.changed(`midi:${message.channel}:${message.controller}`);
      if (!this.timer) this.timer = window.setTimeout(() => { this.timer = 0; this.options.refresh(); }, 50);
      return;
    }
  }
  private showMappings() {
    this.cancel(); this.closeMenu?.();
    const project = this.options.project(), all = strips(project);
    const entries = all.flatMap((strip, column) => (strip.midi ?? []).map(binding => ({
      text: `Remove Ch ${binding.channel + 1} CC${binding.controller} → ${strip.name} / ${laneParameter(binding.target, column, project, this.options.parameters())?.label ?? 'Unavailable control'}`,
      run: () => {
        if (this.options.project() !== project) return;
        strip.midi = strip.midi?.filter(b => b !== binding); if (!strip.midi?.length) delete strip.midi;
        this.options.changed(); this.options.refresh();
      },
    })));
    this.closeMenu = openMenu(this.list, entries.length ? [{ heading: 'MIDI mappings — select one to remove' }, ...entries] : [{ heading: 'No MIDI mappings yet' }]);
  }
}
