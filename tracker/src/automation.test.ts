import { expect, it } from 'vitest';
import { formatValue, parseValue, remapEffectLanes } from './automation';
import { newProject } from './project';
it('reads real units, preserves precision, and rejects wrong units and out-of-range values', () => {
  const frequency = { name: 'cutoff', min: 20, max: 20000, default: 2400, unit: 'hz' };
  expect(parseValue('2.3456789 kHz', frequency)).toBe('2345.6789');
  expect(formatValue('2345.6789', frequency)).toBe('2.34568 kHz');
  expect(parseValue('35%', frequency)).toBeNull(); expect(parseValue('21 kHz', frequency)).toBeNull();
  const time = { ...frequency, min: 0.001, max: 2, unit: 'sec' };
  expect(parseValue('250', time)).toBe('0.25'); expect(parseValue('0.25 sec', time)).toBe('0.25');
  const blend = { ...frequency, min: 0, max: 1, unit: null };
  expect(parseValue('35', blend)).toBe('0.35'); expect(formatValue('0.35', blend)).toBe('35%');
});
it('keeps lanes attached to reordered inserts and clears lanes for removed inserts', () => {
  const project = newProject();
  const lane = project.patterns[0].fx[0][0]; lane.target = { kind: 'effect', device: 0, parameter: 'cutoff' }; lane.values[4] = '2400';
  remapEffectLanes(project, 0, d => d === 0 ? 1 : 0); expect(lane.target.device).toBe(1); expect(lane.values[4]).toBe('2400');
  remapEffectLanes(project, 0, () => null); expect(lane.target).toBeNull(); expect(lane.values.every(v => v === null)).toBe(true);
});
