// The project's files, kept in the browser (IndexedDB) so they are still there on the next visit. Where that is not
// allowed (a private window, blocked site data) the store says so and holds them for this visit only.

export type StoredFile = { path: string; bytes: ArrayBuffer; added: number };

/** A piece of the user's: `piece:<id>` is one they named and saved, `draft:<path>` is the edits they have made to an example. */
export type PieceRecord = { key: string; name: string; text: string; updated: number };

export type Store = {
  pieces: {
    all(): Promise<PieceRecord[]>;
    get(key: string): Promise<PieceRecord | undefined>;
    put(record: PieceRecord): Promise<void>;
    remove(key: string): Promise<void>;
  };
  /** whether what is added will still be here next time */
  lasting: boolean;
  all(): Promise<StoredFile[]>;
  put(file: StoredFile): Promise<void>;
  remove(path: string): Promise<void>;
};

const DB = 'niminal-studio';
const FILES = 'files';
const PIECES = 'pieces';

const request = <T>(r: IDBRequest<T>) => new Promise<T>((resolve, reject) => { r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error); });

export async function openStore(): Promise<Store> {
  try {
    const open = indexedDB.open(DB, 2);
    open.onupgradeneeded = () => {
      const names = open.result.objectStoreNames;
      if (!names.contains(FILES)) open.result.createObjectStore(FILES, { keyPath: 'path' });
      if (!names.contains(PIECES)) open.result.createObjectStore(PIECES, { keyPath: 'key' });
    };
    const db = await request(open);
    // another tab that upgrades or clears the database must not be kept waiting by this one
    db.onversionchange = () => db.close();
    const store = (mode: IDBTransactionMode) => db.transaction(FILES, mode).objectStore(FILES);
    const pieceStore = (mode: IDBTransactionMode) => db.transaction(PIECES, mode).objectStore(PIECES);
    // ask the browser not to clear it when space runs short; it may say no, and nothing depends on the answer
    try { void navigator.storage?.persist?.(); } catch {}
    return {
      pieces: {
        all: async () => (await request(pieceStore('readonly').getAll())) as PieceRecord[],
        get: async (key) => (await request(pieceStore('readonly').get(key))) as PieceRecord | undefined,
        put: async (record) => { await request(pieceStore('readwrite').put(record)); },
        remove: async (key) => { await request(pieceStore('readwrite').delete(key)); },
      },
      lasting: true,
      all: async () => (await request(store('readonly').getAll())) as StoredFile[],
      put: async (file) => { await request(store('readwrite').put(file)); },
      remove: async (path) => { await request(store('readwrite').delete(path)); },
    };
  } catch {
    const held = new Map<string, StoredFile>();
    const pieces = new Map<string, PieceRecord>();
    return {
      pieces: {
        all: async () => [...pieces.values()],
        get: async (key) => pieces.get(key),
        put: async (record) => { pieces.set(record.key, record); },
        remove: async (key) => { pieces.delete(key); },
      },
      lasting: false,
      all: async () => [...held.values()],
      put: async (file) => { held.set(file.path, file); },
      remove: async (path) => { held.delete(path); },
    };
  }
}
