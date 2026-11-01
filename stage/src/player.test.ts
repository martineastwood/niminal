import { afterEach, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { SamplePlayer } from './player';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
afterEach(() => { vi.unstubAllGlobals(); vi.resetAllMocks(); });

it('cancels pending loads when stopped and lets only the latest preview play', async () => {
  const loads: ((bytes: ArrayBuffer) => void)[] = [];
  vi.mocked(invoke).mockImplementation(() => new Promise(resolve => loads.push(resolve as (bytes: ArrayBuffer) => void)));
  const sources: { start: ReturnType<typeof vi.fn>; stop: ReturnType<typeof vi.fn>; onended?: () => void }[] = [];
  vi.stubGlobal('AudioContext', class {
    destination = {};
    async decodeAudioData() { return {}; }
    createBufferSource() {
      const source = { start: vi.fn(), stop: vi.fn(), connect: vi.fn(), onended: undefined };
      sources.push(source);
      return source;
    }
  });
  const player = new SamplePlayer();
  const started = vi.fn(), ended = vi.fn(), failed = vi.fn();
  const first = player.play('first.wav', started, ended, failed);
  player.stop();
  loads[0](new ArrayBuffer(0));
  await first;
  expect(sources).toHaveLength(0);

  const second = player.play('second.wav', started, ended, failed);
  const third = player.play('third.wav', started, ended, failed);
  loads[2](new ArrayBuffer(0));
  await third;
  loads[1](new ArrayBuffer(0));
  await second;
  expect(started).toHaveBeenCalledTimes(1);
  expect(sources).toHaveLength(1);
  player.stop();
  sources[0].onended?.();
  expect(sources[0].stop).toHaveBeenCalledTimes(1);
  expect(ended).not.toHaveBeenCalled();
  expect(failed).not.toHaveBeenCalled();
});
