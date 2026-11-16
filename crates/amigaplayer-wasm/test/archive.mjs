// Archive test of the dp_* ABI: node archive.mjs AMIGAPLAYER_WASM ARCHIVE_FILE [ENTRY_INDEX]
// Lists the archive's entries, plays ENTRY_INDEX (default: the first claimed one); exits 1
// on a failure.
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

const [wasmFile, arcFile, entryArg] = process.argv.slice(2);
const { instance } = await WebAssembly.instantiate(readFileSync(wasmFile), {});
const x = instance.exports;
const info = () => new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.dp_info_ptr(), x.dp_info_len()));
function put(b) { const p = x.dp_alloc(b.length); new Uint8Array(x.memory.buffer, p, b.length).set(b); return p; }
const bytes = new Uint8Array(readFileSync(arcFile));
const name = new TextEncoder().encode(arcFile.split('/').pop());
const pm = put(bytes), pn = put(name);
const n = x.dp_archive(pn, name.length, pm, bytes.length);
x.dp_free(pm, bytes.length);
console.log('archive', n);
if (n <= 0) { console.log(info()); process.exit(1); }
const list = info().trim().split('\n').map(l => l.split('\t'));
for (const [i, path, size, claimed] of list) console.log(' ', i, path, size, claimed);
const pick = entryArg !== undefined ? list.find(e => e[0] === entryArg) : list.find(e => e[3] === '1');
if (!pick) { console.log('no entry claimed'); process.exit(1); }
const r = x.dp_open_entry(+pick[0], 48000, 48000 * 4 * 5, -1);
console.log('open_entry', pick[0], r, JSON.stringify(info()));
if (r !== 0) process.exit(1);
const pcm = new Uint8Array(x.memory.buffer, x.dp_pcm_ptr(), x.dp_pcm_len());
console.log('pcm bytes', pcm.length, 'sha256', createHash('sha256').update(pcm).digest('hex'));
x.dp_close(); x.dp_archive_close();
if (pcm.length === 0) process.exit(1);
