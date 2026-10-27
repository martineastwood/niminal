import { expect, it } from 'vitest';
import { describeAnyCommand, describeCommand, describeTrackCommand } from './describe';
import { addDevice } from './effects';
import { newProject } from './project';

it('says in words what a note command does', () => {
  expect(describeCommand('S80')).toBe('starts the sample 50% of the way in');
  expect(describeCommand('B00')).toBe('plays the sample backwards'); expect(describeCommand('B01')).toBe('plays the sample forwards');
  expect(describeCommand('G10')).toBe('glides the held note to this one, 1 semitone a tick (an octave in 1 row)');
  expect(describeCommand('GFF')).toBe('glides to this note at once'); expect(describeCommand('G00')).toBe('repeats the last glide above it in the column');
  expect(describeCommand('G20')).toBe('glides the held note to this one, 2 semitones a tick (an octave in 0.5 rows)'); expect(describeCommand('U01')).toBe('slides the held note up 0.75 semitones during the row');
  expect(describeCommand('U10')).toBe('slides the held note up 12 semitones during the row'); expect(describeCommand('D04')).toBe('slides the held note down 3 semitones during the row');
  expect(describeCommand('R03')).toBe('plays the note 4 times in the row (every 3 ticks), at the same level');
  expect(describeCommand('R76')).toBe('plays the note 2 times in the row (every 6 ticks), each 50% quieter, building up');
  expect(describeCommand('RF1')).toBe('plays the note 12 times in the row (every 1 tick), each 100% louder, building up');
  expect(describeCommand('C06')).toBe('cuts the note 6 of 12 ticks into the row');
  expect(describeCommand('Y80')).toBe('plays the note 50% of the time');
  expect(describeCommand('I20')).toBe('fades the held note\'s level in by 32 of 128 during the row'); expect(describeCommand('O00')).toBe('repeats the last fade above it in the column');
  expect(describeCommand('Q10')).toContain('Commands are'); // a command that cannot play says why
});

it('says in words what a track command does, naming the effect it reaches', () => {
  const project = newProject(); addDevice(project, 0, 'echo');
  const lead = project.tracks[0];
  expect(describeTrackCommand('L00', project, lead)).toBe('sets the track volume to silence'); expect(describeTrackCommand('LFF', project, lead)).toBe('sets the track volume to 3 dB');
  expect(describeTrackCommand('P80', project, lead)).toBe('pans the track to the centre');
  expect(describeTrackCommand('P00', project, lead)).toBe('pans the track 100% left'); expect(describeTrackCommand('PFF', project, lead)).toBe('pans the track 100% right');
  expect(describeTrackCommand('12FF', project, lead)).toBe('sets parameter 2 of effect 1 (Echo) to 255 of 255 (100% of its range)');
  expect(describeTrackCommand('21FF', project, lead)).toContain('effect 2 (none there yet)');
  expect(describeTrackCommand('01FF', project, lead)).toBe('sets macro 1 (none there yet) to 100%');
  lead.macros = [{ name: 'Open', value: 0, links: [] }];
  expect(describeTrackCommand('0180', project, lead)).toBe('sets macro 1 (Open) to 50%');
  expect(describeTrackCommand('Q80', project, lead)).toContain('not a track command');
});

it('describes either kind of command for an fx cell', () => {
  const project = newProject(), lead = project.tracks[0];
  expect(describeAnyCommand('S80', project, lead)).toBe('starts the sample 50% of the way in');
  expect(describeAnyCommand('L80', project, lead)).toContain('sets the track volume');
  expect(describeAnyCommand('12FF', project, lead)).toContain('effect 1 (none there yet)');
});
