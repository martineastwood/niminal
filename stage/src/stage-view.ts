import { arrangementCommand, arrangementLabel, arrangementProgress } from './arrangements';
import { $, setText } from './layout';
import { Arrangement, EvalOutcome, Problem, Queued, Scene, Snapshot, beatsUntil, queuedStates, stage } from './live';
import { sceneCommand, sceneStates } from './scenes';
import { mod } from './shortcuts';

export function createStageView(deps: {
  ensureConnected: () => Promise<boolean>;
  showOutcome: (outcome: EvalOutcome) => boolean;
  refresh: () => Promise<void>;
  status: (message: string) => void;
  showBanner: (list: Problem[], heading?: string) => void;
  problem: (message: string) => Problem;
  flash: (kind: 'landed' | 'sent' | 'refused') => void;
  cancelPending: (id: number) => Promise<void>;
  quantize: () => string;
}) {
  const scenes = $('scenes');
  const arrangements = $('arrangements');
  const pending = $('pending');
  const notices = $('notices');
  const stageHint = $('stage-hint');

  let lastSceneSignature = '';
  let lastArrangementSignature = '';
  let sceneList: Scene[] = [];
  let pendingSignature = '';

  function sceneButton(scene: Scene, index: number, shown: ReturnType<typeof sceneStates>, queued?: Queued) {
    const button = document.createElement('button');
    button.className = 'scene';
    const { state, within } = shown.get(scene.name) ?? { state: 'off' as const };
    const active = state === 'playing';
    button.classList.toggle('active', active);
    button.classList.toggle('included', state === 'included');
    button.classList.toggle('partial', state === 'partial');
    button.title = {
      playing: `${scene.name} is playing; click to stop it`,
      included: `Part of ${within}, which is playing; click to switch down to just ${scene.name}`,
      partial: `Some of ${scene.name} is playing; click to switch to all of it`,
      off: `Switch to ${scene.name}: the other scenes' clips stop as it starts`,
    }[state];
    const icon = document.createElement('span');
    icon.className = 'scene-icon';
    icon.textContent = active ? '■' : state === 'included' ? '⊂' : '▶';
    const name = document.createElement('span');
    name.className = 'scene-name';
    name.textContent = scene.name;
    const count = document.createElement('span');
    count.className = 'scene-count';
    count.textContent = `${scene.running}/${scene.clips.length}`;
    button.append(icon, name, count);
    if (queued) {
      button.classList.add(`queued-${queued.kind}`);
      const eta = document.createElement('span');
      eta.className = 'queued-eta';
      eta.dataset.lands = String(queued.landsAt);
      button.append(eta);
      button.title = `${scene.name} ${queued.kind === 'start' ? 'starts' : 'stops'} at the next boundary`;
    }
    if (index < 9) {
      const key = document.createElement('span');
      key.className = 'scene-key';
      key.textContent = `${mod}${index + 1}`;
      button.append(key);
    }
    button.addEventListener('click', () => void launch(scene));
    return button;
  }

  async function launch(scene: Scene) {
    if (!(await deps.ensureConnected())) return;
    try {
      deps.showOutcome(await stage.evaluate(sceneCommand(scene, sceneList), deps.quantize(), null, 0));
      await deps.refresh();
    } catch (error) {
      deps.status(String(error));
    }
  }

  function updateScenes(snapshot: Snapshot) {
    const list = snapshot.scenes;
    sceneList = list;
    const queued = queuedStates(snapshot);
    const signature = JSON.stringify(list.map(scene => [scene, queued.get(scene.name)]));
    if (signature === lastSceneSignature) return;
    lastSceneSignature = signature;
    const shown = sceneStates(list);
    scenes.replaceChildren(...list.map((scene, index) => sceneButton(scene, index, shown, queued.get(scene.name))));
    stageHint.textContent = list.length === 0 ? 'no scenes yet' : `${list.length} scene${list.length === 1 ? '' : 's'}`;
  }

  /** One arrangement: press it to play it through, or to stop it. It shows the section it is in and how far it has got. */
  function arrangementRow(arrangement: Arrangement) {
    const button = document.createElement('button');
    button.className = 'arrangement';
    button.classList.toggle('active', arrangement.running);
    button.title = arrangement.running
      ? `${arrangement.name} is playing; click to stop it and let go of what is sounding`
      : `Play ${arrangement.name} from the start`;
    const icon = document.createElement('span');
    icon.className = 'scene-icon';
    icon.textContent = arrangement.running ? '■' : '▶';
    const name = document.createElement('span');
    name.className = 'scene-name';
    name.textContent = arrangement.name;
    const label = document.createElement('span');
    label.className = 'arrangement-label';
    label.textContent = arrangementLabel(arrangement);
    const progress = document.createElement('span');
    progress.className = 'arrangement-progress';
    progress.style.width = `${(arrangementProgress(arrangement) * 100).toFixed(1)}%`;
    button.append(icon, name, label, progress);
    button.addEventListener('click', () => void playArrangement(arrangement));
    return button;
  }

  async function playArrangement(arrangement: Arrangement) {
    if (!(await deps.ensureConnected())) return;
    try {
      // stopping is for now; starting lands where the land box says, as a scene does
      const when = arrangement.running ? 'now' : deps.quantize();
      deps.showOutcome(await stage.evaluate(arrangementCommand(arrangement), when, null, 0));
      await deps.refresh();
    } catch (error) {
      deps.status(String(error));
    }
  }

  function updateArrangements(snapshot: Snapshot) {
    const list = snapshot.arrangements ?? [];
    // the section and the position change as it plays, so they are part of what is compared
    const signature = JSON.stringify(list.map(a => [a.name, a.running, a.section, Math.round((a.elapsed ?? 0) * 2), a.length]));
    if (signature === lastArrangementSignature) return;
    lastArrangementSignature = signature;
    arrangements.replaceChildren(...list.map(arrangementRow));
  }

  /** How long until the next change lands, counted in beats so it is felt rather than read. */
  function updateLanding(snapshot: Snapshot, elapsed = 0) {
    const landing = $('landing');
    const on = snapshot.transport !== null && snapshot.sample_rate > 0 && snapshot.pending.length > 0;
    const next = Math.min(...snapshot.pending.map(p => p.lands_at));
    setText(landing, on ? `lands in ${beatsUntil(snapshot, next, elapsed).toFixed(1)} beats` : '');
    landing.classList.toggle('on', on);
  }

  /** What lands next, with a countdown in beats that is updated in place, so its buttons can be clicked. */
  function updatePending(snapshot: Snapshot) {
    updateLanding(snapshot);
    if (!snapshot.pending.length) {
      pending.replaceChildren();
      pending.classList.add('empty');
      pendingSignature = '';
      return;
    }
    pending.classList.remove('empty');
    const signature = JSON.stringify(snapshot.pending.map(change => [change.id, change.changes, change.lands_at]));
    if (signature !== pendingSignature) {
      pendingSignature = signature;
      pending.replaceChildren();
      const title = document.createElement('div');
      title.className = 'pending-title';
      title.textContent = 'next';
      pending.append(title);
      for (const change of snapshot.pending) {
        const row = document.createElement('div');
        row.className = 'pending-row';
        row.dataset.lands = String(change.lands_at);
        const when = document.createElement('span');
        when.className = 'pending-when';
        const text = document.createElement('span');
        text.className = 'pending-text';
        text.textContent = change.changes.join(' · ') || 'a change';
        const cancel = document.createElement('button');
        cancel.className = 'pending-cancel';
        cancel.textContent = '×';
        cancel.title = 'Cancel this change';
        cancel.addEventListener('click', () => void deps.cancelPending(change.id));
        row.append(when, text, cancel);
        pending.append(row);
      }
    }
    paintCountdowns(snapshot, 0);
  }

  function paintCountdowns(snapshot: Snapshot, elapsed: number) {
    updateLanding(snapshot, elapsed);
    for (const eta of document.querySelectorAll<HTMLElement>('.scene .queued-eta')) {
      setText(eta, `in ${beatsUntil(snapshot, Number(eta.dataset.lands), elapsed).toFixed(1)}`);
    }
    for (const row of pending.querySelectorAll<HTMLElement>('.pending-row')) {
      const when = row.querySelector('.pending-when')!;
      setText(when as HTMLElement, `in ${beatsUntil(snapshot, Number(row.dataset.lands), elapsed).toFixed(1)} beats`);
    }
  }

  /** The latest thing the engine said, or the latest change that landed. */
  function updateNotices(snapshot: Snapshot) {
    let latest: string | undefined;
    for (const notice of snapshot.notices) {
      latest = notice;
      deps.status(notice);
      // the engine can turn down a change that compiled, when it lands: that must not pass unseen
      if (/was refused by the engine/.test(notice)) {
        deps.showBanner([deps.problem(notice)], 'not applied');
        deps.flash('refused');
      }
    }
    const landed = snapshot.landed.at(-1);
    if (landed) {
      latest = `landed: ${landed.changes.join(' · ')}`;
      deps.flash('landed');
    }
    if (latest === undefined) return;
    const row = document.createElement('div');
    row.className = 'notice';
    row.textContent = latest;
    notices.replaceChildren(row);
  }

  function sceneAt(index: number) {
    return sceneList[index];
  }

  function resetSignatures() {
    lastSceneSignature = '';
  }

  return {
    updateScenes,
    updateArrangements,
    updatePending,
    paintCountdowns,
    updateNotices,
    launch,
    sceneAt,
    resetSignatures,
  };
}
