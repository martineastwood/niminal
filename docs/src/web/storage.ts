// The project's files, kept in the browser (IndexedDB) so they are still there on the next visit. Where that is not
// allowed (a private window, blocked site data) the store says so and holds them for this visit only.

export type StoredFile = { path: string; bytes: ArrayBuffer; added: number };

export type Store = {
  /** whether what is added will still be here next time */
  lasting: boolean;
  all(): Promise<StoredFile[]>;
  put(file: StoredFile): Promise<void>;
  remove(path: string): Promise<void>;
};

const DB = 'niminal-studio';
const FILES = 'files';

const request = <T>(r: IDBRequest<T>) => new Promise<T>((resolve, reject) => { r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error); });

export async function openStore(): Promise<Store> {
  try {
    const open = indexedDB.open(DB, 1);
    open.onupgradeneeded = () => open.result.createObjectStore(FILES, { keyPath: 'path' });
    const db = await request(open);
    const store = (mode: IDBTransactionMode) => db.transaction(FILES, mode).objectStore(FILES);
    // ask the browser not to clear it when space runs short; it may say no, and nothing depends on the answer
    try { void navigator.storage?.persist?.(); } catch {}
    return {
      lasting: true,
      all: async () => (await request(store('readonly').getAll())) as StoredFile[],
      put: async (file) => { await request(store('readwrite').put(file)); },
      remove: async (path) => { await request(store('readwrite').delete(path)); },
    };
  } catch {
    const held = new Map<string, StoredFile>();
    return {
      lasting: false,
      all: async () => [...held.values()],
      put: async (file) => { held.set(file.path, file); },
      remove: async (path) => { held.delete(path); },
    };
  }
}
