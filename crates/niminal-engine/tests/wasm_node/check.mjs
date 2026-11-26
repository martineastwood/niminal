// Runs the wasm modules the emitter produced in V8 and compares with the Rust interpreter's output.
// node check.mjs dump.json
import { readFileSync } from "node:fs";

const cases = JSON.parse(readFileSync(process.argv[2], "utf8"));
const f64 = new Float64Array(1), u64 = new BigUint64Array(f64.buffer);
const f32 = new Float32Array(1), u32 = new Uint32Array(f32.buffer);
const fromBits = (h) => { u64[0] = BigInt("0x" + h); return f64[0]; };
const fromBits32 = (h) => { u32[0] = Number("0x" + h); return f32[0]; };
const bits32 = (x) => { f32[0] = x; return u32[0]; };
// the engine's `rem`: x - y * floor(x / y), zero for a zero divisor
const rem = (x, y) => (y === 0 ? 0 : x - y * Math.floor(x / y));
const BIN = [(a, b) => a + b, (a, b) => a - b, (a, b) => a * b, (a, b) => (b === 0 ? 0 : a / b),
  null, null, null, null, rem];

let bad = 0;
for (const c of cases) {
  const bytes = Uint8Array.from(c.wasm.match(/../g).map((h) => parseInt(h, 16)));
  const imports = { env: {
    bin: (id, a, b) => { const f = BIN[id]; if (!f) throw new Error("bin " + id); return f(a, b); },
    f1: (id) => { throw new Error("f1 " + id); },
    wrapx: (x, n) => { if (!Number.isFinite(x)) return 0; const r = ((Math.floor(x) % n) + n) % n; return r % n; },
  } };
  const { instance } = await WebAssembly.instantiate(bytes, imports);
  const mem = new DataView(instance.exports.mem.buffer);
  const put = (off, vals) => vals.forEach((h, i) => mem.setFloat64(off + i * 8, fromBits(h), true));
  const L = c.layout;
  put(L.regs, c.regs); put(L.state, c.state);
  mem.setFloat64(L.sr, 48000, true);
  c.tables.forEach((t, i) => put(L.tables[i], t));
  c.buffers.forEach((t, i) => put(L.buffers[i], t));
  c.inputs.forEach((p, port) => p.forEach((h, i) => mem.setFloat32(L.inputs + (port * L.block + i) * 4, fromBits32(h), true)));
  instance.exports.render(c.frames);
  let mismatch = null;
  c.expected.forEach((reg, r) => reg.forEach((h, i) => {
    const got = mem.getFloat32(L.outputs + (r * L.block + i) * 4, true);
    const want = fromBits32(h);
    const same = bits32(got) === bits32(want) || (Number.isNaN(got) && Number.isNaN(want));
    if (!same && !mismatch) mismatch = `reg ${r} sample ${i}: wasm ${got} expected ${want}`;
  }));
  if (mismatch) { bad++; console.log(`seed ${c.seed} : ${mismatch}`); }
}
console.log(`${cases.length} kernels in ${process.version}'s V8 ${bad} mismatches`);
process.exit(bad ? 1 : 0);
