// A project as a zip file: the piece and the files it uses, at the paths the piece names them by, so that the folder it
// unpacks to opens in Stage unchanged. Written without compression (audio hardly compresses, and the pieces are small);
// read with or without, using the browser's own deflate.

export type Entry = { path: string; bytes: Uint8Array };

const crcTable = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(bytes: Uint8Array) {
  let c = 0xffffffff;
  for (let i = 0; i < bytes.length; i++) c = crcTable[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

/** The MS-DOS date and time a zip entry carries. */
function dosTime(date: Date) {
  return {
    time: (date.getHours() << 11) | (date.getMinutes() << 5) | (date.getSeconds() >> 1),
    day: ((Math.max(1980, date.getFullYear()) - 1980) << 9) | ((date.getMonth() + 1) << 5) | date.getDate(),
  };
}

/** A zip file holding `entries`. Paths use `/` and are relative. */
export function makeZip(entries: Entry[], now = new Date()): Blob {
  const parts: BlobPart[] = [];
  const central: Uint8Array[] = [];
  const { time, day } = dosTime(now);
  const encoder = new TextEncoder();
  let offset = 0;
  for (const entry of entries) {
    const name = encoder.encode(entry.path);
    const crc = crc32(entry.bytes);
    const local = new DataView(new ArrayBuffer(30));
    local.setUint32(0, 0x04034b50, true);
    local.setUint16(4, 20, true); // version needed
    local.setUint16(6, 0x0800, true); // the name is UTF-8
    local.setUint16(8, 0, true); // stored
    local.setUint16(10, time, true);
    local.setUint16(12, day, true);
    local.setUint32(14, crc, true);
    local.setUint32(18, entry.bytes.length, true);
    local.setUint32(22, entry.bytes.length, true);
    local.setUint16(26, name.length, true);
    local.setUint16(28, 0, true);
    parts.push(local.buffer, name, entry.bytes as BlobPart);

    const head = new DataView(new ArrayBuffer(46));
    head.setUint32(0, 0x02014b50, true);
    head.setUint16(4, 20, true); // version made by
    head.setUint16(6, 20, true);
    head.setUint16(8, 0x0800, true);
    head.setUint16(10, 0, true);
    head.setUint16(12, time, true);
    head.setUint16(14, day, true);
    head.setUint32(16, crc, true);
    head.setUint32(20, entry.bytes.length, true);
    head.setUint32(24, entry.bytes.length, true);
    head.setUint16(28, name.length, true);
    head.setUint32(42, offset, true);
    const record = new Uint8Array(46 + name.length);
    record.set(new Uint8Array(head.buffer), 0);
    record.set(name, 46);
    central.push(record);
    offset += 30 + name.length + entry.bytes.length;
  }
  const size = central.reduce((n, r) => n + r.length, 0);
  const end = new DataView(new ArrayBuffer(22));
  end.setUint32(0, 0x06054b50, true);
  end.setUint16(8, entries.length, true);
  end.setUint16(10, entries.length, true);
  end.setUint32(12, size, true);
  end.setUint32(16, offset, true);
  return new Blob([...parts, ...(central as BlobPart[]), end.buffer], { type: 'application/zip' });
}

async function inflate(bytes: Uint8Array) {
  const stream = new Blob([bytes as BlobPart]).stream().pipeThrough(new DecompressionStream('deflate-raw'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/** What is in a zip file, or a reason it cannot be read. Folders, the debris that macOS leaves, and paths that would
 *  leave the project are not returned; a single folder that holds everything (as a zipped folder makes) is taken off. */
export async function readZip(buffer: ArrayBuffer): Promise<Entry[]> {
  const view = new DataView(buffer);
  const bytes = new Uint8Array(buffer);
  // the end record is the last 22+ bytes, before an optional comment
  let end = -1;
  for (let i = buffer.byteLength - 22; i >= Math.max(0, buffer.byteLength - 22 - 65535); i--) {
    if (view.getUint32(i, true) === 0x06054b50) { end = i; break; }
  }
  if (end < 0) throw new Error('That is not a zip file.');
  const count = view.getUint16(end + 10, true);
  let at = view.getUint32(end + 16, true);
  const decoder = new TextDecoder();
  const found: Entry[] = [];
  for (let n = 0; n < count; n++) {
    if (view.getUint32(at, true) !== 0x02014b50) throw new Error('The zip file is damaged.');
    const method = view.getUint16(at + 10, true);
    const compressed = view.getUint32(at + 20, true);
    const nameLength = view.getUint16(at + 28, true);
    const extraLength = view.getUint16(at + 30, true);
    const commentLength = view.getUint16(at + 32, true);
    const local = view.getUint32(at + 42, true);
    const path = decoder.decode(bytes.subarray(at + 46, at + 46 + nameLength));
    at += 46 + nameLength + extraLength + commentLength;
    if (path.endsWith('/')) continue;
    const dataAt = local + 30 + view.getUint16(local + 26, true) + view.getUint16(local + 28, true);
    const raw = bytes.subarray(dataAt, dataAt + compressed);
    if (method !== 0 && method !== 8) throw new Error(`${path} is packed in a way this page cannot read.`);
    found.push({ path, bytes: method === 0 ? raw.slice() : await inflate(raw) });
  }
  const wanted = found.filter((e) => {
    const parts = e.path.split('/');
    return !parts.includes('__MACOSX') && !parts.includes('..') && !e.path.startsWith('/') && parts[parts.length - 1] !== '.DS_Store';
  });
  const roots = new Set(wanted.map((e) => e.path.split('/')[0]));
  const folded = roots.size === 1 && wanted.every((e) => e.path.includes('/'));
  return wanted.map((e) => ({ path: folded ? e.path.slice(e.path.indexOf('/') + 1) : e.path, bytes: e.bytes }));
}
