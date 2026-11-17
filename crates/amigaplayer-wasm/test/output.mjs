// Test of dp_set_output / dp_get_output / dp_default_output: node output.mjs AMIGAPLAYER_WASM
// Checks the defaults, the range errors, and that each setting changes the PCM of a module
// (a minimal ProTracker module is built here); prints the checksums. Exits 1 on a failure.
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

const wasmFile = process.argv[2];
const { instance } = await WebAssembly.instantiate(readFileSync(wasmFile), {});
const x = instance.exports;
let failed = false;
function check(ok, what) { console.log(ok ? 'ok  ' : 'FAIL', what); if (!ok) failed = true; }

const d = new Uint8Array(1084 + 1024 + 256);
d.set(new TextEncoder().encode('out'), 0);
d.set(new TextEncoder().encode('square'), 20);
d[20 + 23] = 128; d[20 + 25] = 64; d[20 + 29] = 128;
d[950] = 1; d[951] = 127;
d.set(new TextEncoder().encode('M.K.'), 1080);
[428, 339, 285, 214].forEach((p, ch) => { const o = 1084 + ch * 4; d[o] = p >> 8; d[o + 1] = p & 255; d[o + 2] = 0x10; });
for (let i = 0; i < 256; i++) d[1084 + 1024 + i] = i < 128 ? 100 : 156;
const name = new TextEncoder().encode('out.mod');
function put(b) { const p = x.dp_alloc(b.length); new Uint8Array(x.memory.buffer, p, b.length).set(b); return p; }
const pm = put(d), pn = put(name);

function render() {
  const r = x.dp_open(pn, name.length, pm, d.length, 48000, 48000 * 4 * 3, -1);
  if (r !== 0) { console.log('open failed', r); process.exit(1); }
  const pcm = new Uint8Array(x.memory.buffer, x.dp_pcm_ptr(), x.dp_pcm_len());
  const out = Uint8Array.from(pcm);
  x.dp_close();
  return out;
}
const sha = (b) => createHash('sha256').update(b).digest('hex').slice(0, 16);
function withOutput(f, r, s) { check(x.dp_set_output(f, r, s) === 0, `dp_set_output(${f}, ${r}, ${s})`); return render(); }

// defaults: 1 (A1200), 1 (Sinc), 300; read back, and equal to dp_default_output
check([0, 1, 2].every((i) => x.dp_get_output(i) === [1, 1, 300][i]), 'defaults are 1, 1, 300');
check([0, 1, 2].every((i) => x.dp_default_output(i) === x.dp_get_output(i)), 'dp_default_output = dp_get_output at the start');
check(x.dp_get_output(3) === -1 && x.dp_get_output(-1) === -1 && x.dp_default_output(3) === -1, 'what out of range is -1');
const byDefault = render();
const explicitDefault = withOutput(1, 1, 300);
check(sha(byDefault) === sha(explicitDefault), `explicit defaults = no call (${sha(byDefault)})`);

// out of range: -1, and nothing changes
for (const [f, r, s] of [[3, 1, 300], [-1, 1, 300], [1, 2, 300], [1, -1, 300], [1, 1, 1001], [1, 1, -1]]) {
  check(x.dp_set_output(f, r, s) === -1, `dp_set_output(${f}, ${r}, ${s}) is -1`);
}
check([0, 1, 2].every((i) => x.dp_get_output(i) === [1, 1, 300][i]), 'a refused call changes nothing');
check(sha(render()) === sha(byDefault), 'and the PCM is the same');

// each setting changes the PCM
const old = withOutput(0, 0, 700);
const sums = new Map([['default', sha(byDefault)], ['old output (A500, default resampler, 700)', sha(old)]]);
check(sha(old) !== sha(byDefault), 'the old output differs from the default');
for (const [label, f, r, s] of [['A500', 0, 1, 300], ['no filter', 2, 1, 300], ['default resampler', 1, 0, 300],
                                 ['separation 1000', 1, 1, 1000], ['separation 0', 1, 1, 0]]) {
  const p = withOutput(f, r, s);
  sums.set(label, sha(p));
  check(sha(p) !== sha(byDefault), `${label} changes the PCM`);
}
check(sha(withOutput(0, 0, 700)) === sha(old), 'the same setting gives the same PCM again');
check(x.dp_get_output(0) === 0 && x.dp_get_output(1) === 0 && x.dp_get_output(2) === 700, 'dp_get_output reads the setting back');
// separation 0 is mono: both channels equal
withOutput(1, 1, 0);
const mono = Int16Array.from(new Int16Array(render().buffer));
let same = true;
for (let i = 0; i < mono.length; i += 2) if (mono[i] !== mono[i + 1]) { same = false; break; }
check(same, 'separation 0 gives equal left and right');
for (const [k, v] of sums) console.log(' ', k, v);
process.exit(failed ? 1 : 0);
