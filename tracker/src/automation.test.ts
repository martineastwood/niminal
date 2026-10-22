import { expect, it } from 'vitest';
import { formatValue, parseValue, laneParameter } from './automation';
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
it('addresses track FX by name and retains missing targets for repair', () => {
  const project = newProject();
  project.definitions.push({ id: 'filter', kind: 'fx', name: 'Filter', source: '' });
  project.tracks[0].fx = [{ id: 'd1', definition: 'filter', bypass: false, values: {}, modulations: {} }];
  const target = { kind: 'effect' as const, device: 'd1', parameter: 'cutoff' };
  const parameters = { filter: [{ name: 'cutoff', min: 20, max: 20000, default: 2400, unit: 'hz' }] };
  expect(laneParameter(target, 0, project, parameters)?.name).toBe('cutoff');
  expect(laneParameter(target, 0, project, { filter: [] })).toBeNull();
  expect(target.parameter).toBe('cutoff');
});
