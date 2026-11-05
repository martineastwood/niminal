import { look } from './look';

/** One animation-frame loop for the whole window. It runs only while something is moving, so an idle window
 *  costs nothing between polls. Each callback returns whether it still has work for the next frame. */
const callbacks: (() => boolean)[] = [];
let running = false;

let lastRun = 0;

function tick(now: number) {
  // 30 frames a second is plenty for a playhead and a lit note, and half the work of 60; the look can ask for more
  if (now - lastRun < (look.fps === 60 ? 14 : 30)) {
    requestAnimationFrame(tick);
    return;
  }
  lastRun = now;
  let busy = false;
  for (const callback of callbacks) {
    try {
      if (callback()) busy = true;
    } catch (error) {
      console.error('a frame callback failed', error);
    }
  }
  if (busy) requestAnimationFrame(tick);
  else running = false;
}

/** Call `callback` every frame while the loop runs. */
export function onFrame(callback: () => boolean) {
  callbacks.push(callback);
  wake();
}

/** Something changed (a new snapshot arrived): make sure the loop is running. */
export function wake() {
  if (running) return;
  running = true;
  requestAnimationFrame(tick);
}
