import { describe, expect, it } from 'vitest';
import { sceneCommand, sceneStates } from './scenes';
import { Scene } from './live';

const scene = (name: string, clips: string[], running: number): Scene => ({ name, clips, running });

describe('what a scene button shows', () => {
  const verse = (running: number) => scene('verse', ['bass', 'lead'], running);
  const chorus = (running: number) => scene('chorus', ['bass', 'lead', 'shimmer'], running);
  const states = (...list: Scene[]) => Object.fromEntries([...sceneStates(list)].map(([name, s]) => [name, s.state]));

  it('lights the scene that was launched, and not the smaller one inside it', () => {
    expect(states(verse(2), chorus(3))).toEqual({ verse: 'included', chorus: 'playing' });
  });

  it('lights the smaller scene when only it is playing', () => {
    expect(states(verse(2), chorus(2))).toEqual({ verse: 'playing', chorus: 'partial' });
  });

  it('says which scene it is part of', () => {
    expect(sceneStates([verse(2), chorus(3)]).get('verse')).toEqual({ state: 'included', within: 'chorus' });
  });

  it('lights both when they are different sets that both play', () => {
    const outro = scene('outro', ['pad'], 1);
    expect(states(verse(2), outro)).toEqual({ verse: 'playing', outro: 'playing' });
  });

  it('shows an empty scene as off', () => {
    expect(states(scene('nothing', [], 0))).toEqual({ nothing: 'off' });
  });
});

describe('pressing a scene', () => {
  const verse = scene('verse', ['bass', 'lead'], 2);
  const chorus = scene('chorus', ['bass', 'lead', 'shimmer'], 3);

  it('stops the scene that is playing', () => {
    expect(sceneCommand(chorus, [verse, chorus])).toBe('chorus.stop');
  });

  it('starts a scene that is only included in the playing one, which switches down to it', () => {
    expect(sceneCommand(verse, [verse, chorus])).toBe('verse.start');
  });

  it('starts a scene that is not playing', () => {
    expect(sceneCommand(scene('outro', ['pad'], 0), [verse, scene('outro', ['pad'], 0)])).toBe('outro.start');
  });
});
