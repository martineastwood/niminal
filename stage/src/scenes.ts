import { Scene } from './live';

/**
 * What a scene button shows. A scene whose clips are all running is *playing*, unless a bigger scene that
 * contains it is also fully running: then it is only *included* in that one, because the bigger scene is what
 * was launched. `verse` (bass, lead) is included while `chorus` (bass, lead, shimmer) plays.
 */
export type SceneState = 'playing' | 'included' | 'partial' | 'off';

export function sceneStates(scenes: Scene[]) {
  const full = (scene: Scene) => scene.clips.length > 0 && scene.running === scene.clips.length;
  const inside = (small: Scene, big: Scene) =>
    big.clips.length > small.clips.length && small.clips.every(name => big.clips.includes(name));
  const states = new Map<string, { state: SceneState; within?: string }>();
  for (const scene of scenes) {
    if (!full(scene)) {
      states.set(scene.name, { state: scene.running > 0 ? 'partial' : 'off' });
      continue;
    }
    const bigger = scenes.find(other => full(other) && inside(scene, other));
    states.set(scene.name, bigger ? { state: 'included', within: bigger.name } : { state: 'playing' });
  }
  return states;
}

/**
 * What pressing a scene sends: stop it if it is playing, otherwise start it. The engine does the rest:
 * starting a scene stops the clips of the other scenes that it does not use, all landing together.
 */
export function sceneCommand(scene: Scene, scenes: Scene[]) {
  const playing = sceneStates(scenes).get(scene.name)?.state === 'playing';
  return `${scene.name}.${playing ? 'stop' : 'start'}`;
}
