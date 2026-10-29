/** The musical divisions an effect length can be: a name and its length in beats (a quarter note is one). */
export const divisions: { name: string; beats: number }[] = [
  { name: '1/32', beats: 1 / 8 }, { name: '1/32d', beats: 3 / 16 },
  { name: '1/16t', beats: 1 / 6 }, { name: '1/16', beats: 1 / 4 }, { name: '1/16d', beats: 3 / 8 },
  { name: '1/8t', beats: 1 / 3 }, { name: '1/8', beats: 1 / 2 }, { name: '1/8d', beats: 3 / 4 },
  { name: '1/4t', beats: 2 / 3 }, { name: '1/4', beats: 1 }, { name: '1/4d', beats: 3 / 2 },
  { name: '1/2t', beats: 4 / 3 }, { name: '1/2', beats: 2 }, { name: '1/2d', beats: 3 },
  { name: '1/1', beats: 4 },
].sort((a, b) => a.beats - b.beats);

/** The division nearest `beats`, by ratio, as the sound itself snaps it. */
export function nearestDivision(beats: number) {
  const length = Math.max(beats, 1e-6);
  return divisions.reduce((best, d) => Math.abs(Math.log(length / d.beats)) < Math.abs(Math.log(length / best.beats)) ? d : best);
}
