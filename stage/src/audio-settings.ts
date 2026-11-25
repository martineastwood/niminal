import { $, input } from './layout';
import { AudioState, stage } from './live';

/** The sound card, sample rate and buffer size. Changing them starts the engine again. */
const BUFFERS = [64, 128, 256, 512, 1024];

function fillSelect(select: HTMLSelectElement, options: { value: string; label: string }[], chosen: string) {
  select.replaceChildren(
    ...options.map(({ value, label }) => {
      const option = document.createElement('option');
      option.value = value;
      option.textContent = label;
      return option;
    }),
  );
  if (!options.some(o => o.value === chosen)) {
    const option = document.createElement('option');
    option.value = chosen;
    option.textContent = chosen;
    select.append(option);
  }
  select.value = chosen;
}

export function createAudioSettings(deps: {
  ensureConnected: () => Promise<boolean>;
  ask: (message: string) => Promise<boolean>;
  status: (message: string) => void;
  onApplied: () => void;
}) {
  const panel = document.createElement('section');
  panel.className = 'settings-section';
  panel.innerHTML = `
    <h3>Audio</h3>
    <p class="settings-note">Changing the sound card, sample rate or buffer starts the engine again.</p>
    <div class="settings-row"><label for="audio-output">Output<small>the sound card to play through</small></label><select id="audio-output"></select></div>
    <div class="settings-row"><label for="audio-rate">Sample rate</label><select id="audio-rate"></select></div>
    <div class="settings-row"><label for="audio-buffer">Buffer<small>smaller is quicker to respond, larger is safer against glitches</small></label><select id="audio-buffer"></select></div>
    <div class="settings-row"><div id="audio-note" class="audio-note"></div><button id="audio-apply" title="Start the engine on these settings">Apply</button></div>
    <div class="settings-row" title="Controls bound with osc_in hear OSC sent to this UDP port on this machine">
      <label for="audio-osc">OSC in port<small>the UDP port to listen on; empty is off</small></label>
      <span class="settings-inline"><input id="audio-osc" inputmode="numeric" placeholder="off" /><button id="osc-apply" title="Listen for OSC on this port, or stop if it is empty">Listen</button></span>
    </div>`;

  function showAudio(state: AudioState) {
    const output = panel.querySelector('#audio-output') as HTMLSelectElement;
    const rate = panel.querySelector('#audio-rate') as HTMLSelectElement;
    const buffer = panel.querySelector('#audio-buffer') as HTMLSelectElement;
    fillSelect(
      output,
      [{ value: '', label: 'system default' }, ...state.devices.map(d => ({ value: d.name, label: d.name }))],
      state.choice.output ?? '',
    );
    const chosen = state.devices.find(d => d.name === (state.choice.output ?? ''));
    const rates = chosen ? chosen.rates : [...new Set(state.devices.flatMap(d => d.rates))].sort((a, b) => a - b);
    fillSelect(
      rate,
      [{ value: '', label: "the card's own" }, ...rates.map(r => ({ value: String(r), label: `${r} Hz` }))],
      state.choice.sample_rate ? String(state.choice.sample_rate) : '',
    );
    fillSelect(
      buffer,
      [{ value: '', label: 'default' }, ...BUFFERS.map(b => ({ value: String(b), label: `${b} frames` }))],
      state.choice.buffer_size ? String(state.choice.buffer_size) : '',
    );
    $('audio-note').textContent = `playing at ${state.sample_rate} Hz through ${state.output}`;
    input('audio-osc').value = state.osc_port ? String(state.osc_port) : '';
  }

  /** Show what the engine is playing through now, each time the settings page opens. */
  async function refresh() {
    if (!(await deps.ensureConnected())) return;
    try {
      showAudio(await stage.audioState());
    } catch (error) {
      deps.status(String(error));
    }
  }

  function bind() {
    // choosing another card offers its own sample rates
    panel.querySelector('#audio-output')!.addEventListener('change', async () => {
      try {
        const state = await stage.audioState();
        state.choice.output = (panel.querySelector('#audio-output') as HTMLSelectElement).value || null;
        state.choice.sample_rate = null;
        showAudio(state);
      } catch (error) {
        deps.status(String(error));
      }
    });

    $('osc-apply').addEventListener('click', async () => {
      const port = Number(input('audio-osc').value) || null;
      try {
        await stage.oscListen(port);
        deps.status(port ? `listening for OSC on port ${port}` : 'not listening for OSC');
      } catch (error) {
        deps.status(String(error));
      }
    });

    $('audio-apply').addEventListener('click', async () => {
      const value = (id: string) => (panel.querySelector(id) as HTMLSelectElement).value;
      const choice = {
        output: value('#audio-output') || null,
        sample_rate: Number(value('#audio-rate')) || null,
        buffer_size: Number(value('#audio-buffer')) || null,
      };
      const sure = await deps.ask(
        'Changing the audio settings starts the engine again. What is playing stops and your clips and scenes are cleared, so evaluate your code again afterwards. Continue?',
      );
      if (!sure) return;
      $('audio-note').textContent = 'starting the engine…';
      try {
        const state = await stage.audioApply(choice);
        showAudio(state);
        deps.onApplied();
        deps.status(`playing at ${state.sample_rate} Hz through ${state.output}; evaluate your code to play it again`);
      } catch (error) {
        // the previous settings were put back: show them, with why the new ones did not work
        deps.status(String(error));
        void stage
          .audioState()
          .then(showAudio)
          .catch(() => {})
          .finally(() => {
            $('audio-note').textContent = String(error);
          });
      }
    });
  }

  return { panel, refresh, bind };
}
