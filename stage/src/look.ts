/** How the window looks while it plays: how much the lit notes glow, how long they linger, whether the instrument
 *  they play lights up too, and how often the window redraws. The presets trade looks against CPU. */
export type Look = {
  /** Scales the size of the glow around a lit note; 0 is none. */
  glow: number;
  /** How long a note stays lit after it starts, in seconds. */
  trail: number;
  /** Light the name of the instrument a clip plays in its definition whenever it sounds a note. */
  instruments: boolean;
  /** The most frames a second the lit notes and playheads are redrawn at. */
  fps: 30 | 60;
  /** Bar, beat and tempo in the top bar at a size that reads on a screen recording. */
  largeTransport: boolean;
};

export const PRESETS = {
  calm: { glow: 0.5, trail: 0.35, instruments: false, fps: 30, largeTransport: false },
  standard: { glow: 1, trail: 0.45, instruments: true, fps: 30, largeTransport: false },
  video: { glow: 1.5, trail: 0.7, instruments: true, fps: 60, largeTransport: true },
} satisfies Record<string, Look>;

export type PresetName = keyof typeof PRESETS;

const KEY = 'niminal-stage-look';

/** The look now in force; read it where it is used rather than copying it. */
export const look: Look = { ...PRESETS.standard };

const listeners: (() => void)[] = [];

/** Called after the look changes, so the parts that keep their own copy can redraw. */
export function onLook(listener: () => void) {
  listeners.push(listener);
}

/** The preset the look matches, or `custom` once a single setting has been moved off one. */
export function presetOf(value: Look = look): PresetName | 'custom' {
  const same = (name: PresetName) => (Object.keys(value) as (keyof Look)[]).every(k => value[k] === PRESETS[name][k]);
  return (Object.keys(PRESETS) as PresetName[]).find(same) ?? 'custom';
}

function clamp(value: unknown, low: number, high: number, fallback: number) {
  const n = Number(value);
  return Number.isFinite(n) ? Math.min(Math.max(n, low), high) : fallback;
}

/** Change some settings, draw with them, and remember them. */
export function setLook(change: Partial<Look>) {
  const next = { ...look, ...change };
  look.glow = clamp(next.glow, 0, 2.5, PRESETS.standard.glow);
  look.trail = clamp(next.trail, 0.2, 1.5, PRESETS.standard.trail);
  look.instruments = !!next.instruments;
  look.fps = next.fps === 60 ? 60 : 30;
  look.largeTransport = !!next.largeTransport;
  document.body.classList.toggle('large-transport', look.largeTransport);
  document.documentElement.style.setProperty('--glow', String(look.glow));
  try {
    localStorage.setItem(KEY, JSON.stringify(look));
  } catch {
    // private windows and cleared site data can refuse storage; the look still applies for this run
  }
  for (const listener of listeners) listener();
}

/** Use what was kept from last time, or the standard look. */
export function loadLook() {
  let saved: Partial<Look> = {};
  try {
    saved = JSON.parse(localStorage.getItem(KEY) ?? '{}');
  } catch {
    // nothing kept, or unreadable: the standard look
  }
  setLook({ ...PRESETS.standard, ...saved });
}

/** The Look section of the settings page: a preset, and the settings it is made of. */
export function lookSection(): HTMLElement {
  const section = document.createElement('section');
  section.className = 'settings-section';
  section.innerHTML = `
    <h3>Look</h3>
    <p class="settings-note">How lit notes glow while a clip plays, and what it costs. Pick a preset, or change a single setting.</p>
    <div class="settings-row">
      <label for="look-preset">Preset<small>calm is light on the CPU; video is the strongest for screen recordings</small></label>
      <select id="look-preset">
        <option value="calm">calm</option>
        <option value="standard">standard</option>
        <option value="video">video</option>
        <option value="custom" hidden>custom</option>
      </select>
    </div>
    <div class="settings-row">
      <label for="look-glow">Glow<small>how far the light around a playing note spreads</small></label>
      <input id="look-glow" type="range" min="0" max="2.5" step="0.1" />
    </div>
    <div class="settings-row">
      <label for="look-trail">Trail<small>how long a note stays lit after it starts</small></label>
      <input id="look-trail" type="range" min="0.2" max="1.5" step="0.05" />
    </div>
    <div class="settings-row">
      <label for="look-instruments">Light instrument names<small>light the name where an instrument is defined whenever a clip plays a note on it</small></label>
      <input id="look-instruments" type="checkbox" />
    </div>
    <div class="settings-row">
      <label for="look-fps">Frame rate<small>the most times a second lit notes and playheads redraw; 60 is smoother and uses more CPU</small></label>
      <select id="look-fps"><option value="30">30</option><option value="60">60</option></select>
    </div>
    <div class="settings-row">
      <label for="look-large">Large transport<small>bar, beat and tempo in the top bar at a size that reads on a recording</small></label>
      <input id="look-large" type="checkbox" />
    </div>`;
  const get = <T extends HTMLElement>(id: string) => section.querySelector<T>(`#${id}`)!;
  const preset = get<HTMLSelectElement>('look-preset');
  const glow = get<HTMLInputElement>('look-glow');
  const trail = get<HTMLInputElement>('look-trail');
  const instruments = get<HTMLInputElement>('look-instruments');
  const fps = get<HTMLSelectElement>('look-fps');
  const large = get<HTMLInputElement>('look-large');
  const show = () => {
    preset.value = presetOf();
    glow.value = String(look.glow);
    trail.value = String(look.trail);
    instruments.checked = look.instruments;
    fps.value = String(look.fps);
    large.checked = look.largeTransport;
  };
  preset.addEventListener('change', () => {
    if (preset.value in PRESETS) setLook(PRESETS[preset.value as PresetName]);
    show();
  });
  glow.addEventListener('input', () => setLook({ glow: Number(glow.value) }));
  trail.addEventListener('input', () => setLook({ trail: Number(trail.value) }));
  instruments.addEventListener('change', () => setLook({ instruments: instruments.checked }));
  fps.addEventListener('change', () => setLook({ fps: Number(fps.value) === 60 ? 60 : 30 }));
  large.addEventListener('change', () => setLook({ largeTransport: large.checked }));
  onLook(show);
  show();
  return section;
}
