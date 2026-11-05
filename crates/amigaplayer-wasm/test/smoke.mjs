// Smoke test of the dp_* ABI with no glue code: node smoke.mjs AMIGAPLAYER_WASM [MODULE_FILE]
// Without a module file a minimal ProTracker module is built here. Prints the module's
// info string and a checksum of the PCM; exits 1 on a failure.
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

const [wasmFile, modFile] = process.argv.slice(2);

function minimalMod() {
  const d = new Uint8Array(1084 + 1024 + 256);
  d.set(new TextEncoder().encode('smoke'), 0);
  d.set(new TextEncoder().encode('square'), 20);
  d[20 + 23] = 128; d[20 + 25] = 64; d[20 + 29] = 128;     // 128 words, volume 64, loop whole
  d[950] = 1; d[951] = 127;
  d.set(new TextEncoder().encode('M.K.'), 1080);
  const notes = [428, 339, 285, 214];
  for (let row = 0; row < 64; row += 16) notes.forEach((p, ch) => {
    if ((row / 16 + ch) % 2 === 0) { const o = 1084 + (row * 4 + ch) * 4; d[o] = p >> 8; d[o + 1] = p & 255; d[o + 2] = 0x10; }
  });
  for (let i = 0; i < 256; i++) d[1084 + 1024 + i] = i < 128 ? 100 : 156;
  return d;
}

const { instance } = await WebAssembly.instantiate(readFileSync(wasmFile), {});
const x = instance.exports;
const bytes = modFile ? new Uint8Array(readFileSync(modFile)) : minimalMod();
const name = new TextEncoder().encode(modFile ? modFile.split('/').pop() : 'smoke.mod');

function put(b) { const p = x.dp_alloc(b.length); new Uint8Array(x.memory.buffer, p, b.length).set(b); return p; }
const pm = put(bytes), pn = put(name);
console.log('detect', x.dp_detect(pm, bytes.length));
const r = x.dp_open(pn, name.length, pm, bytes.length, 48000, 48000 * 4 * 5, -1);
const info = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.dp_info_ptr(), x.dp_info_len()));
console.log('open', r, JSON.stringify(info));
if (r !== 0) process.exit(1);
const pcm = new Uint8Array(x.memory.buffer, x.dp_pcm_ptr(), x.dp_pcm_len());
console.log('pcm bytes', pcm.length, 'truncated', x.dp_truncated());
console.log('sha256', createHash('sha256').update(pcm).digest('hex'));
if (pcm.length === 0) process.exit(1);
x.dp_close();
