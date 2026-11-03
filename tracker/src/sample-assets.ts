import type { Definition } from './project';

export interface SampleAsset {
  file: string; frames: number | null; rate: number | null; channels: number | null; duplicate_of: string | null; error: string | null;
}
export const sampleReferences = (source: string) => [...source.replace(/\/\/[^\n]*/g, '').matchAll(/\bsample\("([^"\n]+)"\)/g)].map(m => m[1]);
export function assetUsers(definitions: Definition[], file: string) {
  const folder = file.slice(0, file.lastIndexOf('/'));
  return definitions.filter(d => sampleReferences(d.source).includes(file) || d.source.includes(`kit("${folder}")`));
}
export function missingAssets(definitions: Definition[], assets: SampleAsset[]): SampleAsset[] {
  const present = new Set(assets.map(a => a.file));
  return [...new Set(definitions.flatMap(d => sampleReferences(d.source)))].filter(file => !present.has(file)).map(file => ({ file, frames: null, rate: null, channels: null, duplicate_of: null, error: 'Missing file' }));
}

export async function showSampleAssets(actions: {
  list(): Promise<SampleAsset[]>;
  definitions(): Definition[];
  replace(file: string): Promise<boolean>;
  preview(file: string): void;
  clean(): Promise<void>;
  stop(): void;
}) {
  const dialog = document.createElement('dialog'); dialog.className = 'sample-assets'; dialog.setAttribute('aria-label', 'Project samples');
  const heading = document.createElement('h2'); heading.textContent = 'Project samples';
  const tools = document.createElement('div'); tools.className = 'sample-toolbar';
  const filter = document.createElement('input'); filter.type = 'search'; filter.placeholder = 'Filter files or instruments'; filter.setAttribute('aria-label', 'Filter project samples');
  const clean = document.createElement('button'); clean.textContent = 'Review unused samples…';
  const close = document.createElement('button'); close.textContent = 'Close'; close.onclick = () => dialog.close(); tools.append(filter, clean, close);
  const message = document.createElement('p'), list = document.createElement('div'); list.className = 'sample-asset-list';
  message.textContent = 'Reading project audio…';
  dialog.append(heading, tools, message, list); document.body.append(dialog); dialog.showModal();
  dialog.onclose = () => { actions.stop(); dialog.remove(); };
  let assets: SampleAsset[] = [];
  function render() {
    const definitions = actions.definitions(), query = filter.value.toLowerCase();
    const rows = [...missingAssets(definitions, assets), ...assets];
    message.textContent = `${assets.length} files · ${rows.filter(a => a.error).length} unavailable. Replacement preserves instrument settings. Undo restores earlier references.`;
    list.replaceChildren();
    for (const asset of rows) {
      const users = assetUsers(definitions, asset.file);
      if (query && !`${asset.file} ${users.map(d => d.name).join(' ')}`.toLowerCase().includes(query)) continue;
      const row = document.createElement('div'); row.className = 'sample-asset';
      const info = document.createElement('div'), name = document.createElement('strong'), detail = document.createElement('div'), usage = document.createElement('div');
      name.textContent = asset.file;
      detail.textContent = asset.error ?? `${((asset.frames ?? 0) / (asset.rate ?? 48000)).toFixed(2)} s · ${asset.rate} Hz · ${asset.channels} ch · ${asset.file.split('.').at(-1)?.toUpperCase()}`;
      usage.textContent = users.length ? `Used by ${users.map(d => d.name).join(', ')}` : 'No active instrument uses this file';
      if (asset.duplicate_of) { const duplicate = document.createElement('div'); duplicate.textContent = `Identical to ${asset.duplicate_of}`; info.append(duplicate); }
      info.prepend(name, detail, usage);
      const hear = document.createElement('button'); hear.textContent = 'Hear'; hear.disabled = !!asset.error; hear.onclick = () => actions.preview(asset.file);
      const replace = document.createElement('button'); replace.textContent = asset.error ? 'Relink…' : 'Replace…'; replace.disabled = !users.length;
      replace.onclick = async () => { replace.disabled = true; try { if (await actions.replace(asset.file)) await refresh(); } catch (e) { message.textContent = String(e); } finally { replace.disabled = !users.length; } };
      row.append(info, hear, replace); list.append(row);
    }
  }
  async function refresh() { try { assets = await actions.list(); if (dialog.isConnected) render(); } catch (e) { message.textContent = `Could not list samples: ${e}`; } }
  clean.onclick = async () => { try { await actions.clean(); await refresh(); } catch (e) { message.textContent = String(e); } };
  filter.oninput = render;
  await refresh();
}
