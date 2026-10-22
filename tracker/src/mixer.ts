import type { Project } from './project';
import { trackColour } from './grid';

/** The device strip follows the track selected in the pattern editor. */
export function renderMixer(parent: HTMLElement, project: Project, activeTrack: number, selectTrack: (track: number) => void, edited: () => void, editEffect: (id: string) => void) {
  parent.replaceChildren();
  const track = project.tracks[activeTrack];
  const output = document.createElement('section'); output.className = 'device-card track-output';
  output.style.setProperty('--track-colour', trackColour(activeTrack));
  const heading = document.createElement('div'); heading.className = 'device-heading';
  const picker = document.createElement('select'); picker.setAttribute('aria-label', 'Effects track');
  project.tracks.forEach((t, i) => picker.add(new Option(`${String(i + 1).padStart(2, '0')} ${t.name}`, String(i))));
  picker.value = String(activeTrack); picker.onchange = () => selectTrack(Number(picker.value));
  const mute = document.createElement('button'); mute.textContent = 'Mute'; mute.setAttribute('aria-label', `Mute ${track.name}`);
  mute.setAttribute('aria-pressed', String(track.muted));
  mute.onclick = () => { track.muted = !track.muted; mute.setAttribute('aria-pressed', String(track.muted)); edited(); };
  heading.append(picker, mute);
  const label = document.createElement('label'); label.textContent = 'Output';
  const volume = document.createElement('input'); volume.type = 'range'; volume.min = '-60'; volume.max = '6'; volume.step = '1'; volume.value = String(track.volume_db); volume.setAttribute('aria-label', `${track.name} volume in dB`);
  const value = document.createElement('span'); value.className = 'volume-value'; value.textContent = `${track.volume_db} dB`;
  volume.oninput = () => { track.volume_db = Number(volume.value); value.textContent = `${track.volume_db} dB`; edited(); };
  label.append(volume, value);
  const routing = document.createElement('p'); routing.className = 'device-routing'; routing.textContent = `${track.name} → ${track.effect ? project.definitions.find(d => d.id === track.effect)?.name : 'Direct'} → Master`;
  output.append(heading, label, routing);
  const effect = document.createElement('section'); effect.className = 'device-card effect-device';
  const title = document.createElement('div'); title.className = 'device-heading'; title.textContent = '01 / TRACK EFFECT';
  const select = document.createElement('select'); select.setAttribute('aria-label', `${track.name} effect`); select.add(new Option('No effect · direct to master', ''));
  for (const definition of project.definitions.filter(d => d.kind === 'fx')) select.add(new Option(definition.name, definition.id));
  select.value = track.effect ?? '';
  select.onchange = () => { track.effect = select.value || null; edited(); renderMixer(parent, project, activeTrack, selectTrack, edited, editEffect); };
  const edit = document.createElement('button'); edit.textContent = 'Edit effect code'; edit.disabled = !track.effect; edit.setAttribute('aria-label', `Edit ${track.name} effect`);
  edit.onclick = () => { if (track.effect) editEffect(track.effect); };
  const description = document.createElement('p'); description.textContent = track.effect ? 'Applied to every note on this track.' : 'Choose a saved effect for this track.';
  effect.append(title, select, description, edit);
  parent.append(output, effect);
}
