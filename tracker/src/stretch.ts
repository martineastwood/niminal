/** The ways a sampler's code is given its time-stretch settings. Samplers the tracker wrote before stretching existed lack them. */

/** `source` with the `stretch` and `stretch_mode` parameters of a sampler, passed on to its sample. */
export function withStretch(source: string): string {
  source = source.replace(/stretch_mode: 0\.\.1 = ([01])/, 'stretch_mode: 0..2 = $1');
  if (!/stretch: 0\.25\.\.4 = /.test(source)) {
    source = source.replace('rate: -4..4 = 1', 'rate: -4..4 = 1, stretch: 0.25..4 = 1').replaceAll('rate: rate)', 'rate: rate, stretch: stretch)');
  }
  if (!/stretch_mode: 0\.\.2 = /.test(source)) {
    source = source.replace(/stretch: 0\.25\.\.4 = [\d.]+/, '$&, stretch_mode: 0..2 = 0').replaceAll('stretch: stretch)', 'stretch: stretch, stretch_mode: stretch_mode)');
  }
  return source;
}

/** The tempo, in beats a minute, the sample was recorded at, as the code sets it; 0 when it is not fitted to the song's tempo. */
export function readFit(source: string): number {
  return Number(/from_bpm: 0\.\.400 = ([\d.]+)/.exec(source)?.[1] ?? 0) || 0;
}

/** `source` with its `from_bpm` set (0 turns the fit off), adding the parameters it needs to a sampler that lacks them. */
export function withFit(source: string, bpm: number): string {
  source = withStretch(source);
  if (!/from_bpm: 0\.\.400 = /.test(source)) source = source.replace(/stretch_mode: 0\.\.2 = [012]/, '$&, from_bpm: 0..400 = 0');
  return source.replace(/from_bpm: 0\.\.400 = [\d.]+/, `from_bpm: 0..400 = ${bpm}`);
}

/** The tempo a sample of `seconds` is at if it lasts `beats` beats, to two places, or null when that is outside what a tempo can be. */
export function sourceTempo(beats: number, seconds: number): number | null {
  const bpm = Math.round(beats * 60 / seconds * 100) / 100;
  return Number.isFinite(bpm) && bpm > 0 && bpm <= 400 ? bpm : null;
}

/** Suggest a whole-sample beat count from onset shares. This assumes a trimmed loop;
 * hit density alone cannot distinguish beats from subdivisions or half/double time. */
export function estimateBeats(hits: number[], seconds: number): { beats: number; bpm: number } | null {
  if (!Number.isFinite(seconds) || seconds <= 0) return null;
  const points = [...new Set(hits.filter(hit => Number.isFinite(hit) && hit >= 0 && hit < 1))].sort((a, b) => a - b);
  if (points.length < 4) return null;
  let best: { beats: number; bpm: number; score: number; fit: number } | null = null;
  // Whole beats over the file, in a useful initial tempo range. Quarter-beat
  // subdivisions allow hats and syncopation; missing hits may span several beats.
  for (let beats = 1; beats <= 256; beats++) {
    const bpm = beats * 60 / seconds;
    if (bpm < 60 || bpm > 200) continue;
    let fit = 0, count = 0, complexity = 0;
    for (let i = 0; i < points.length; i++) {
      for (let j = i + 1; j < Math.min(points.length, i + 5); j++) {
        const interval = (points[j] - points[i]) * beats;
        const quarters = Math.round(interval * 4);
        if (quarters === 0) { count++; continue; }
        const error = interval - quarters / 4;
        fit += Math.exp(-0.5 * (error / 0.05) ** 2);
        count++;
        if (j === i + 1) complexity += quarters % 4 === 0 ? 0 : quarters % 2 === 0 ? 0.04 : 0.08;
      }
    }
    fit /= count;
    const score = fit - complexity / (points.length - 1) - 0.06 * Math.abs(Math.log2(bpm / 120));
    if (!best || score > best.score) best = { beats, bpm, score, fit };
  }
  if (!best || best.fit < 0.75) return null;
  return { beats: best.beats, bpm: Math.round(best.bpm * 100) / 100 };
}
