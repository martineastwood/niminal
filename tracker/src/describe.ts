import { commandProblem, trackCommandProblem, type Project, type Track } from './project';

const hex = (text: string) => parseInt(text, 16);
const trim = (n: number) => String(Number(n.toFixed(2)));
const plural = (n: number, word: string) => `${trim(n)} ${word}${n === 1 ? '' : 's'}`;
const levels = [0, 3, 6, 12, 25, 50];

/** What a note command does, in words, for a tooltip or the status line; its problem if it cannot be played. */
export function describeCommand(text: string): string {
  const problem = commandProblem(text);
  if (problem) return problem;
  const value = hex(text.slice(1)), x = value >> 4, y = value & 15;
  const repeat = (what: string) => value === 0 ? `repeats the last ${what} above it in the column` : null;
  switch (text[0]) {
    case 'S': return `starts the sample ${Math.round(value / 256 * 100)}% of the way in`;
    case 'B': return value === 0 ? 'plays the sample backwards' : 'plays the sample forwards';
    case 'G': return repeat('glide') ?? (value === 255 ? 'glides to this note at once' : `glides the held note to this one, ${plural(value / 16, 'semitone')} a tick (an octave in ${plural(16 / value, 'row')})`);
    case 'U': return repeat('slide') ?? `slides the held note up ${plural(value * 0.75, 'semitone')} during the row`;
    case 'D': return repeat('slide') ?? `slides the held note down ${plural(value * 0.75, 'semitone')} during the row`;
    case 'R': {
      const change = x === 0 || x === 8 ? 'at the same level' : x <= 5 ? `each ${levels[x]}% quieter` : x === 6 ? 'each 33% quieter, building up' : x === 7 ? 'each 50% quieter, building up' : x <= 13 ? `each ${levels[x - 8]}% louder` : x === 14 ? 'each 50% louder, building up' : 'each 100% louder, building up';
      return `plays the note ${Math.floor(12 / y)} times in the row (every ${plural(y, 'tick')}), ${change}`;
    }
    case 'C': return `cuts the note ${y} of 12 ticks into the row`;
    case 'Y': return `plays the note ${Math.round(value / 255 * 100)}% of the time`;
    case 'I': return repeat('fade') ?? `fades the held note's level in by ${value} of 128 during the row`;
    default: return repeat('fade') ?? `fades the held note's level out by ${value} of 128 during the row`;
  }
}

/** What a track command does, in words; `strip` says which effect an `xyzz` command reaches. */
export function describeTrackCommand(text: string, project: Project, strip: Track): string {
  const problem = trackCommandProblem(text);
  if (problem) return problem;
  if (text[0] === 'L') {
    const value = hex(text.slice(1));
    return value === 0 ? 'sets the track volume to silence' : `sets the track volume to ${trim(20 * Math.log10(value / 255) + 3)} dB`;
  }
  if (text[0] === 'P') {
    const pan = Math.round((hex(text.slice(1)) - 128) / 127 * 100);
    return pan === 0 ? 'pans the track to the centre' : `pans the track ${Math.min(100, Math.abs(pan))}% ${pan < 0 ? 'left' : 'right'}`;
  }
  if (text[0] === '0') {
    const index = hex(text[1]), value = hex(text.slice(2)), knob = strip.macros?.[index - 1];
    return `sets ${knob ? `macro ${index} (${knob.name || index})` : `macro ${index} (none there yet)`} to ${Math.round(value / 255 * 100)}%`;
  }
  const place = hex(text[0]), parameter = hex(text[1]), value = hex(text.slice(2)), device = strip.fx[place - 1];
  const named = device ? `effect ${place} (${project.definitions.find(d => d.id === device.definition)?.name ?? device.definition})` : `effect ${place} (none there yet)`;
  return `sets parameter ${parameter} of ${named} to ${value} of 255 (${Math.round(value / 255 * 100)}% of its range)`;
}

/** What a command cell does, whichever kind it is: a note's, or the track's. */
export function describeAnyCommand(text: string, project: Project, strip: Track): string {
  return trackCommandProblem(text) === null ? describeTrackCommand(text, project, strip) : describeCommand(text);
}
