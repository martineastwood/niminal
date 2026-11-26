// Plays the audio a worker makes: chunks arrive over a MessagePort, are queued, and are copied out a render
// quantum (128 frames) at a time. It reports how much it has played so the worker knows when to make more, and
// says so when it runs dry.
class Player extends AudioWorkletProcessor {
  constructor() {
    super();
    this.queue = [];
    this.offset = 0; // frames of queue[0] already played
    this.played = 0; // frames played or dropped since the start
    this.sinceReport = 0;
    this.dry = false;
    this.running = false;
    this.waiting = true; // nothing has arrived since the start or a restart, so a dry queue is expected
    this.port.onmessage = (event) => {
      if (!event.data.link) return;
      this.link = event.data.link;
      this.link.onmessage = (message) => {
        if ('running' in message.data) this.running = message.data.running;
        if (message.data.chunk) { this.queue.push(message.data.chunk); this.waiting = false; }
        // a restart or a stop: what is queued is dropped, and counted as played so the worker's books balance
        if (message.data.clear) {
          for (const chunk of this.queue) this.played += chunk[0].length;
          this.played -= this.offset;
          this.queue = [];
          this.waiting = true;
          this.offset = 0;
          this.report();
        }
      };
    };
  }

  report(underrun = false) {
    this.link?.postMessage({ played: this.played, underrun });
    this.sinceReport = 0;
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    const frames = out[0].length;
    let at = 0;
    while (at < frames && this.queue.length > 0) {
      const chunk = this.queue[0];
      const take = Math.min(frames - at, chunk[0].length - this.offset);
      for (let c = 0; c < out.length; c++) {
        // one channel plays on both sides; more than two are not mixed down, only the first two are heard
        const source = chunk[Math.min(c, chunk.length - 1)];
        out[c].set(source.subarray(this.offset, this.offset + take), at);
      }
      at += take;
      this.offset += take;
      this.played += take;
      if (this.offset >= chunk[0].length) {
        this.queue.shift();
        this.offset = 0;
      }
    }
    if (at < frames) {
      for (const channel of out) channel.fill(0, at);
      // only once per dry spell: a stopped engine is not an underrun
      if (!this.dry && this.queue.length === 0 && this.running && !this.waiting) {
        this.dry = true;
        this.report(true);
      }
    } else {
      this.dry = false;
    }
    if (++this.sinceReport >= 8) this.report();
    return true;
  }
}

registerProcessor("niminal-player", Player);
