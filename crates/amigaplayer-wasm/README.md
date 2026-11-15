# amigaplayer-wasm

The open replayers of amigaplayer-rs for the browser (or Node, Deno, any WebAssembly host):
Amiga modules and the PC tracker formats in, 16-bit stereo PCM out, behind a small C-style ABI
(`dp_*`, "decode and play") that a few lines of JavaScript can drive. No imports are needed
(an empty import object), no threads, no SDL, none of the 68k player files: the module is the
library of the repository's root built with `default-features = false` plus `archives` and
`custom-modules`. Custom modules (`cust.*`, a hunk executable carrying the module's own 68k replay
code) run in the engine's 68k host, as natively; no player of anyone else's is in the `.wasm`.

Licence: GPL-2.0-or-later (see `../../LICENSE` and the `LICENSE-*` files). A page that ships
this `.wasm` ships a GPL-2.0-or-later combined work: offer the source.

## Build

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --manifest-path crates/amigaplayer-wasm/Cargo.toml --target wasm32-unknown-unknown
wasm-strip target/wasm32-unknown-unknown/release/amigaplayer_wasm.wasm      # optional, from wabt
```

The crate is outside the root workspace on purpose: its `[profile.release]` (LTO, `panic =
"abort"`, `opt-level = 3`) and its own `Cargo.lock` only apply to a crate that is not a member.
`tools/lib-check.sh` builds it and runs the smoke test below.

## Model

`dp_open` decodes the **whole tune into memory** (up to a byte limit) and returns; the page then
copies the PCM out of the module's linear memory and plays it with Web Audio. A tune ends where
its replayer says so, after 10 s of silence, or at the byte limit (then `dp_truncated()` is 1):
many Amiga players loop for ever. The state is one global: one decode at a time per instance.

The PCM is interleaved stereo signed 16-bit little-endian at the rate given to `dp_open`.

## The `dp_*` protocol

| export | meaning |
|--------|---------|
| `dp_alloc(n) -> ptr` | `n` bytes of module memory to copy a file into |
| `dp_free(ptr, n)` | free a block from `dp_alloc` |
| `dp_detect(ptr, n) -> 0/1` | 1 if an open replayer claims the module, or it is a custom module |
| `dp_add_file(path_ptr, path_len, ptr, n)` | a companion file for the next `dp_open`, at `path` relative to the module (`smpl.tune` for TFMX, `Instruments/Piano` for IFF-SMUS); `..` components are dropped. Consumed by `dp_open` |
| `dp_open(name_ptr, name_len, ptr, n, rate, max_bytes, subsong) -> i32` | decodes the module (`name` is its file name, UTF-8; `/` and `\` in it become `_`) at `rate` Hz, at most `max_bytes` of PCM, from `subsong` (-1: the first). Returns **0** ok, **-2** not claimed by any open replayer (the page should use another player), **-1** failed (the reason is in the info string) |
| `dp_info_ptr() / dp_info_len()` | UTF-8 string `title\tformat\tsubsong\tfirst\tlast` after a successful open; after -1 the failure reason |
| `dp_pcm_ptr() / dp_pcm_len()` | the PCM (length in **bytes**) |
| `dp_truncated() -> 0/1` | the byte limit cut the tune short |
| `dp_close()` | drop the PCM and info |

`subsong`, `first`, `last` are in the format's own numbering. The info string, the PCM and
`dp_alloc` pointers are only valid until the next call that allocates (the linear memory can
grow, which invalidates `ArrayBuffer` views: create the views after the call).

## Minimal JavaScript

```js
const { instance } = await WebAssembly.instantiate(await (await fetch('amigaplayer_wasm.wasm')).arrayBuffer(), {});
const x = instance.exports;
const put = (bytes) => { const p = x.dp_alloc(bytes.length); new Uint8Array(x.memory.buffer, p, bytes.length).set(bytes); return p; };

function decode(bytes, name, rate = 48000, maxSeconds = 600) {
  const pm = put(bytes), pn = put(new TextEncoder().encode(name));
  const nameLen = new TextEncoder().encode(name).length;
  if (!x.dp_detect(pm, bytes.length)) return null;                    // not an open-replayer format
  const r = x.dp_open(pn, nameLen, pm, bytes.length, rate, maxSeconds * rate * 4, -1);
  const info = new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.dp_info_ptr(), x.dp_info_len()));
  if (r !== 0) throw new Error(info);                                  // -1: failed, with the reason
  const [title, format, subsong, first, last] = info.split('\t');
  const s16 = new Int16Array(x.memory.buffer.slice(x.dp_pcm_ptr(), x.dp_pcm_ptr() + x.dp_pcm_len()));
  const truncated = x.dp_truncated() === 1;
  x.dp_close();
  return { title, format, subsong: +subsong, first: +first, last: +last, s16, rate, truncated };
}
```

Feed `s16` (interleaved L/R) to an `AudioBuffer` (divide by 32768) or an `AudioWorklet`.
For companion files call `x.dp_add_file(...)` before `dp_open`.

`amigaplayer.mjs` of the archive web app (`createAmigaPlayer(wasmBytes)`, `A.decode({ bytes,
name, companions, rate, limitS })`) is a ready-made wrapper of exactly this protocol; the ABI
is unchanged since it was written and must stay so.

## Smoke test

`test/smoke.mjs` drives the raw exports without any glue and prints a checksum of the PCM:

```sh
node crates/amigaplayer-wasm/test/smoke.mjs target/wasm32-unknown-unknown/release/amigaplayer_wasm.wasm [module]
```

Without a module it builds a minimal ProTracker module itself.
