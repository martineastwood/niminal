import { invoke } from '@tauri-apps/api/core';

/** One preview at a time. Stopping also cancels a preview that is still loading. */
export class SamplePlayer {
  private context: AudioContext | null = null;
  private source: AudioBufferSourceNode | null = null;
  private request = 0;

  stop() {
    this.request++;
    const source = this.source;
    this.source = null;
    source?.stop();
  }

  async play(path: string, started: (buffer: AudioBuffer) => void, ended: () => void, failed: (error: unknown) => void) {
    this.stop();
    const request = this.request;
    try {
      this.context ??= new AudioContext();
      const bytes = await invoke<ArrayBuffer>('read_sample', { path });
      const buffer = await this.context.decodeAudioData(bytes);
      if (request !== this.request) return;
      const source = this.context.createBufferSource();
      source.buffer = buffer;
      source.connect(this.context.destination);
      source.onended = () => {
        if (this.source !== source) return;
        this.source = null;
        ended();
      };
      this.source = source;
      source.start();
      started(buffer);
    } catch (error) {
      if (request === this.request) failed(error);
    }
  }
}
