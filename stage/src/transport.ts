import { positionAt } from './animation';
import { onFrame } from './frames';
import { HealthWatch } from './health';
import { $, setText } from './layout';
import { Problem, Snapshot, stage } from './live';
import { Bar, loudest } from './meters';

/** The MIDI clock choices: the ports there are, listed when a menu opens, and the one chosen kept. */
const NEW_CLOCK_PORT = 'niminal clock';

export function createTransport(deps: {
  status: (message: string) => void;
  showBanner: (list: Problem[], heading?: string) => void;
  problem: (message: string) => Problem;
  writeLearned: (control: string, source: string) => void;
}) {
  const bar = $('bar');
  const beat = $('beat');
  const bpm = $('bpm');
  const meter = $('meter');
  const voices = $('voices');
  const waiting = $('waiting');

  /** The output meter: one bar per channel (up to two), stepped and drawn every frame. */
  const masterBars: Bar[] = [];
  let masterAt = performance.now();
  function masterFrame(): boolean {
    const now = performance.now();
    const passed = Math.min((now - masterAt) / 1000, 0.25);
    masterAt = now;
    for (const b of masterBars) {
      b.step(passed);
      b.draw();
    }
    return masterBars.some(b => !b.settled);
  }
  onFrame(masterFrame);

  /** The MIDI chip, and the controls that learned a control change written back into the code that asked. */
  let lastMidi: string | null = null;
  function updateMidi(snapshot: Snapshot) {
    const chip = $('midi');
    const { inputs, last } = snapshot.midi;
    // always shown, so a controller plugged in (or a bridge started) after the stage can be found with a click
    $('midi-readout').hidden = false;
    chip.textContent = inputs.length ? (last ?? `${inputs.length} in`) : 'none';
    chip.title = inputs.length
      ? `${inputs.join(', ')}${last ? ` · last: ${last}` : ''}. Click to look for inputs again.`
      : 'No MIDI inputs. Plug one in and click to look again.';
    chip.classList.toggle('active', inputs.length > 0);
    if (last !== lastMidi) {
      lastMidi = last;
      chip.classList.remove('blink');
      void chip.offsetWidth;
      chip.classList.add('blink');
    }
    for (const { control, source } of snapshot.learned) deps.writeLearned(control, source);
  }

  const healthWatch = new HealthWatch();
  function updateHealth(snapshot: Snapshot) {
    const chip = $('health');
    const { ok, detail, known, load } = healthWatch.update(snapshot.health, performance.now());
    chip.classList.toggle('ok', ok);
    chip.classList.toggle('trouble', !ok);
    chip.textContent = ok ? 'ok' : 'dropouts';
    $('health-readout').hidden = ok;
    chip.title = ok
      ? known
        ? 'The engine is keeping up'
        : 'This engine does not report its health'
      : load
        ? `The audio glitched: ${detail}. Fewer voices or lighter effects will help.`
        : `The audio glitched: ${detail}, which reached the engine after the moment they were for.`;
  }

  function updateMaster(snapshot: Snapshot) {
    const count = Math.min(Math.max(snapshot.channels, 1), 2);
    const host = $('master');
    if (masterBars.length !== count) {
      masterBars.length = 0;
      for (let i = 0; i < count; i++) masterBars.push(new Bar());
      host.replaceChildren(...masterBars.map(b => b.element));
    }
    // more than two channels: the second bar is the loudest of the rest
    masterBars.forEach((b, i) =>
      b.push(i === count - 1 && snapshot.master.length > count ? loudest(snapshot.master.slice(i)) : (snapshot.master[i] ?? 0)),
    );
  }

  /** A replay (called a take inside the code) is a stretch of the engine's clock: the engine's session log and the evaluations are cut to it. */
  let take: { from: number; to: number | null } | null = null;
  let clock = { sample: 0, rate: 48000 };

  function minutes(samples: number) {
    const seconds = Math.max(0, Math.floor(samples / clock.rate));
    return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
  }

  function drawTake() {
    const box = $('take');
    const rec = $('rec');
    rec.classList.toggle('recording', take !== null && take.to === null);
    if (!take) {
      box.hidden = true;
      return;
    }
    box.hidden = false;
    const { from, to } = take;
    if (to === null) {
      box.textContent = `● replay ${minutes(clock.sample - from)}`;
      return;
    }
    box.replaceChildren();
    const label = document.createElement('span');
    label.textContent = `replay · ${minutes(to - from)}`;
    const button = (name: string, title: string, run: () => Promise<string | null>) => {
      const node = document.createElement('button');
      node.textContent = name;
      node.title = title;
      node.addEventListener('click', async () => {
        try {
          const done = await run();
          if (done) deps.status(done);
        } catch (error) {
          deps.status(String(error));
          deps.showBanner([deps.problem(String(error))], 'replay');
        }
      });
      return node;
    };
    const close = button('×', 'Forget this replay', async () => {
      take = null;
      drawTake();
      return null;
    });
    box.append(
      label,
      button('Render WAV', "Play the engine's session log over this replay to a WAV file", () => stage.renderTake(from, to)),
      button('Save script', 'Everything you evaluated during this replay, in order, with when', () => stage.saveTranscript(from, to)),
      close,
    );
  }

  function toggleRecording() {
    if (!clock.sample && !take) return;
    if (!take || take.to !== null) take = { from: clock.sample, to: null };
    else take.to = clock.sample;
    drawTake();
  }

  let litDot = -1;
  function paintPosition(snapshot: Snapshot, elapsed: number) {
    const position = positionAt(snapshot, elapsed);
    if (!position) return;
    setText(bar, String(position.bar));
    // whole beats only: a finer readout changes every frame and repaints the window 60 times a second
    setText(beat, String(Math.floor(position.beat)));
    // one dot per beat as written (6/8 has six), while the engine counts quarter notes
    const [top, unit] = snapshot.meter;
    const count = Math.max(1, top || 4);
    const dots = $('beats');
    if (dots.children.length !== count) {
      dots.replaceChildren(...Array.from({ length: count }, () => document.createElement('i')));
      litDot = -1;
    }
    const lit = Math.floor(((position.beat - 1) * unit) / 4 + 1e-9);
    if (litDot !== lit) {
      litDot = lit;
      Array.from(dots.children).forEach((dot, i) => dot.classList.toggle('on', i === lit));
    }
  }

  function updateTransport(snapshot: Snapshot) {
    if (snapshot.transport) clock = { sample: snapshot.transport.sample, rate: snapshot.sample_rate || clock.rate };
    if (take && take.to === null) drawTake();
    const t = snapshot.transport;
    if (!t) {
      bar.textContent = '1';
      beat.textContent = '1.1';
      bpm.textContent = '—';
      voices.textContent = '0';
      waiting.textContent = '0';
      $('waiting-readout').hidden = true;
      meter.textContent = snapshot.meter.join('/');
      $('beats').replaceChildren();
      return;
    }
    paintPosition(snapshot, 0);
    bpm.textContent = t.bpm.toFixed(1);
    meter.textContent = snapshot.meter.join('/');
    voices.textContent = String(t.voices);
    waiting.textContent = String(snapshot.pending.length);
    $('waiting-readout').hidden = snapshot.pending.length === 0;
  }

  function clockMenu(id: 'clock-in' | 'clock-out', set: (name: string | null) => Promise<void>) {
    const select = $(id) as HTMLSelectElement;
    const off = select.options[0].textContent ?? '';
    const refill = async () => {
      const ports = await stage.midiPorts().catch(() => ({ inputs: [], outputs: [] }));
      const names = id === 'clock-in' ? ports.inputs : ports.outputs;
      const chosen = select.value;
      const options = [{ value: '', label: off }];
      if (id === 'clock-out' && !names.includes(NEW_CLOCK_PORT)) {
        options.push({ value: NEW_CLOCK_PORT, label: `${NEW_CLOCK_PORT} (new port)` });
      }
      options.push(...names.map(name => ({ value: name, label: name })));
      if (chosen && !options.some(o => o.value === chosen)) options.push({ value: chosen, label: chosen });
      select.replaceChildren(
        ...options.map(({ value, label }) => {
          const option = document.createElement('option');
          option.value = value;
          option.textContent = label;
          return option;
        }),
      );
      select.value = chosen;
    };
    select.addEventListener('mousedown', () => void refill());
    select.addEventListener('change', async () => {
      try {
        await set(select.value || null);
        select.classList.toggle('active', select.value !== '');
        deps.status(select.value ? `${id === 'clock-in' ? 'following' : 'sending clock to'} ${select.value}` : 'clock off');
      } catch (error) {
        select.value = '';
        select.classList.remove('active');
        deps.status(String(error));
      }
    });
  }

  $('rec').addEventListener('click', toggleRecording);
  $('midi').addEventListener('click', () => {
    stage.openMidi('').then(
      names => deps.status(names.length ? `listening to ${names.join(', ')}` : 'no MIDI inputs found'),
      error => deps.status(String(error)),
    );
  });
  clockMenu('clock-in', name => stage.clockIn(name));
  clockMenu('clock-out', name => stage.clockOut(name));

  return { updateTransport, paintPosition, updateMaster, updateHealth, updateMidi };
}
