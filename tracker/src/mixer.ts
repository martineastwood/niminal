import type { Project } from './project';

export function renderMixer(parent: HTMLElement, project: Project, edited: () => void, editEffect: (id: string) => void) {
  parent.replaceChildren();
  for (const [index, track] of project.tracks.entries()) {
    const strip = document.createElement('section'); strip.className = 'mixer-strip';
    const header = document.createElement('div'); header.className = 'mixer-heading';
    const name = document.createElement('strong'); name.textContent = `${index + 1} / ${track.name}`;
    const mute = document.createElement('button'); mute.textContent = 'Mute'; mute.setAttribute('aria-label', `Mute ${track.name}`);
    mute.setAttribute('aria-pressed', String(track.muted)); mute.classList.toggle('active', track.muted);
    mute.onclick = () => { track.muted = !track.muted; mute.setAttribute('aria-pressed', String(track.muted)); mute.classList.toggle('active', track.muted); edited(); };
    header.append(name, mute);
    const volume = document.createElement('input'); volume.type = 'range'; volume.min = '-60'; volume.max = '6'; volume.step = '1'; volume.value = String(track.volume_db); volume.setAttribute('aria-label', `${track.name} volume in dB`);
    const value = document.createElement('span'); value.className = 'volume-value'; value.textContent = `${track.volume_db} dB`;
    volume.oninput = () => { track.volume_db = Number(volume.value); value.textContent = `${track.volume_db} dB`; edited(); };
    const select = document.createElement('select'); select.setAttribute('aria-label', `${track.name} effect`); select.add(new Option('No effect', ''));
    for (const definition of project.definitions.filter(d => d.kind === 'fx')) select.add(new Option(definition.name, definition.id));
    select.value = track.effect ?? '';
    const edit = document.createElement('button'); edit.textContent = 'Edit FX'; edit.disabled = !track.effect; edit.setAttribute('aria-label', `Edit ${track.name} effect`);
    select.onchange = () => { track.effect = select.value || null; edit.disabled = !track.effect; if (track.effect) editEffect(track.effect); edited(); };
    edit.onclick = () => { if (track.effect) editEffect(track.effect); };
    const effects = document.createElement('div'); effects.className = 'mixer-effect'; effects.append(select, edit);
    strip.append(header, volume, value, effects); parent.append(strip);
  }
}
