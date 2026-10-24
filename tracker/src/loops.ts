/** Loop points are shares of the whole sample, captured when a note starts. */
export interface SampleLoop { mode: 'none' | 'forward' | 'bidirectional'; start: number; end: number }
const binding = /^(\w+\s*=\s*sample\("[^"\n]+"\)[^\n]*?)(?=[ \t]*\/\/|$)/m;
const modeCall = /\.loop\(\s*(none|false|forward|true|bidirectional|pingpong)\s*\)/;

export function readLoop(source: string): SampleLoop {
  const setting = modeCall.exec(binding.exec(source)?.[0] ?? '')?.[1];
  const mode = setting === 'forward' || setting === 'true' ? 'forward' : setting === 'bidirectional' || setting === 'pingpong' ? 'bidirectional' : 'none';
  const start = Number(/loop_start: 0\.\.1 = ([\d.]+)/.exec(source)?.[1] ?? 0);
  const end = Number(/loop_end: 0\.\.1 = ([\d.]+)/.exec(source)?.[1] ?? 1);
  return { mode, start, end };
}

export function writeLoop(source: string, loop: SampleLoop): string {
  if (!binding.test(source) || !Number.isFinite(loop.start) || !Number.isFinite(loop.end) || loop.start < 0 || loop.start >= loop.end || loop.end > 1) return source;
  source = source.replace(binding, text => modeCall.test(text) ? text.replace(modeCall, `.loop(${loop.mode})`) : `${text}.loop(${loop.mode})`);
  if (!/loop_start: 0\.\.1 = /.test(source)) {
    source = source.replace('begin: 0..1 = 0', 'begin: 0..1 = 0, loop_start: 0..1 = 0, loop_end: 0..1 = 1')
      .replaceAll('begin: begin', 'begin: begin, loop_start: loop_start, loop_end: loop_end');
  }
  source = source.replace(/loop_start: 0\.\.1 = [\d.]+/, `loop_start: 0..1 = ${Number(loop.start.toFixed(9))}`)
    .replace(/loop_end: 0\.\.1 = [\d.]+/, `loop_end: 0..1 = ${Number(loop.end.toFixed(9))}`);
  if (loop.mode !== 'none') source = source.replace('gate: 0..1 = 0', 'gate: 0..1 = 1');
  return source;
}
