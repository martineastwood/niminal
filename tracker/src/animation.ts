/** A display clock anchored to backend time; recording keeps its own unbounded timeline. */
export class DisplayClock {
  private seconds = 0;
  private receivedAt = 0;
  private age = 0;

  anchor(seconds: number, atMs: number) {
    this.seconds = seconds;
    this.age = Math.max(0, (Date.now() - atMs) / 1000);
    this.receivedAt = performance.now();
  }

  read() {
    return this.seconds + Math.min(0.25, this.age + Math.max(0, (performance.now() - this.receivedAt) / 1000));
  }
}
