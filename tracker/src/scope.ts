/** Draws a waveform (samples from -1 to 1) on a canvas, one line across its width. */
export function drawScope(canvas: HTMLCanvasElement, frames: number[] | undefined, colour = '#d9903f') {
  const ratio = window.devicePixelRatio || 1;
  const width = Math.max(1, Math.round(canvas.clientWidth * ratio)), height = Math.max(1, Math.round(canvas.clientHeight * ratio));
  if (canvas.width !== width) canvas.width = width;
  if (canvas.height !== height) canvas.height = height;
  const context = canvas.getContext('2d');
  if (!context) return;
  context.clearRect(0, 0, width, height);
  const middle = height / 2;
  context.strokeStyle = 'rgba(255,255,255,.08)'; context.lineWidth = 1;
  context.beginPath(); context.moveTo(0, middle + .5); context.lineTo(width, middle + .5); context.stroke();
  context.strokeStyle = colour; context.lineWidth = Math.max(1, ratio); context.beginPath();
  if (!frames?.length) { context.moveTo(0, middle); context.lineTo(width, middle); }
  else frames.forEach((value, i) => {
    const x = i / Math.max(1, frames.length - 1) * width, y = middle - Math.max(-1, Math.min(1, value)) * (middle - 1);
    if (i === 0) context.moveTo(x, y); else context.lineTo(x, y);
  });
  context.stroke();
}

/** A level from 0 to 1 for a meter, from the loudest sample, on a scale that runs from -60 dB to +6 dB. */
export function meterLevel(frames: number[] | undefined): number {
  if (!frames?.length) return 0;
  const peak = frames.reduce((p, v) => Math.max(p, Math.abs(v)), 0);
  return peak <= 0 ? 0 : Math.max(0, Math.min(1, (20 * Math.log10(peak) + 60) / 66));
}
