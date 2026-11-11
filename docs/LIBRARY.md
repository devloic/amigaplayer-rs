# Using amigaplayer-rs as a library

amigaplayer-rs is a player (`amigaplayer-rs`, SDL3) and a library. The library is the **open
replayers**: Rust ports of Amiga music replay routines that drive an emulated Paula, CIAs and
(for a few formats) a 68000 exactly as the original players do, the PC tracker formats of the
libxmp port, the built-in ProTracker with ProWizard's conversions of packed modules, a
decruncher for packed files, and an AIFF decoder. Bytes in, PCM out; no audio device, no files
read behind your back, deterministic output.

It does **not** include the hosting of the user's original 68k DeliPlayers / Eagleplayers
(`host-68k` feature, on for the application only), and has no SDL dependency.

| consumer | how |
|----------|-----|
| Rust crates | git dependency on this repository, `default-features = false` |
| WebAssembly / JavaScript | `crates/amigaplayer-wasm` (the `dp_*` ABI) |
| C and other languages | `crates/amigaplayer-capi` (`libamigaplayer`, `amigaplayer.h`) |

## Licence

GPL-2.0-or-later, as a whole; files keep the attribution and licence texts of what they are
ported from (`LICENSE-NostalgicPlayer`, `LICENSE-libxmp`, `LICENSE-pt2-clone`, ...; all
GPL-compatible, and the texts have to travel with the code). **Linking the library (statically
or dynamically), or shipping the `.wasm`, makes your program a combined work that has to be
offered under GPL-2.0-or-later compatible terms.** GPL-2.0-only programs cannot link it.

## Rust

```toml
[dependencies]
amigaplayer = { git = "https://fg.lolo.asia/adminlolo/amigaplayer-rs.git", package = "amigaplayer-rs", default-features = false }
```

(The package is `amigaplayer-rs`; the `package =` key lets you name it `amigaplayer` in your own
code: `use amigaplayer::Song;`. Pin a `rev =` or `tag =` for reproducible builds.) With
`default-features = false` Cargo builds the library with no SDL and no 68k player hosting.
The repository's `Cargo.lock` is not used by a dependent; the dependencies are `anyhow`, `log`,
`env_logger`, `m68k` and two path crates of this workspace (`crates/m68k-rc`,
`crates/players-native`), all fetched by Cargo with the git dependency.

```rust
use amigaplayer::{detect, Song};

let bytes = std::fs::read("tune.mod")?;
println!("{:?}", detect(&bytes));                       // Some("ProTracker")

let mut song = Song::open("tune.mod", &bytes, 48000)?;  // name: a hint, some formats look at it
println!("{} [{}]", song.title(), song.format());
let subs = song.subsongs();                              // first / last / current
song.set_subsong(subs.first)?;

let mut pcm = vec![0i16; 2 * 4800];                      // interleaved stereo
while !song.songend() && song.elapsed_secs() < 120.0 {
    song.render_i16(&mut pcm);                           // or render_f32
    // play or write `pcm`
}
```

### API

| item | |
|------|--|
| `detect(&[u8]) -> Option<String>` | which format would play the bytes (after unpacking), without starting it |
| `formats() -> Vec<&'static str>` | the formats the library knows |
| `Song::open(name, bytes, rate)` / `open_with(.., companions, ..)` | start a module. Companions (TFMX `smpl.*`, Mark Cooksey `mcs.*`, IFF-SMUS `Instruments/*`, ...) are `(relative path, bytes)` pairs |
| `render_f32` / `render_i16` | fill interleaved stereo; return frames written |
| `subsongs()` / `set_subsong(n)` | `Subsongs { first, last, current }`; out of range is an `Error` |
| `songend()` | the replayer reported the end (many never do: use a time limit and/or silence detection) |
| `duration_hint()` | seconds, for recordings and PC trackers; `None` for Amiga replayers |
| `elapsed_secs()` / `restart()` / `seek(secs)` | position; `seek` restarts and renders up to the point |
| `title()` / `format()` / `sample_rate()` | metadata |
| `Error` | `NotRecognised`, `Invalid(String)`, `InvalidRate(u32)`, `SubsongOutOfRange{..}`, `Internal(String)`; `#[non_exhaustive]` |

Rates from 1000 to 768000 Hz are accepted; 44100 and 48000 are what the output stage is
tuned for. A `Song` is `Send`, not `Sync`; use one per thread. It allocates the emulated machine's
memory when opened (about 6 MB resident for a ProTracker song), so open songs when needed, not in bulk.

Output is deterministic: the same bytes, rate and subsong give the same samples, run after run.
Garbage input gives `Err`, and a replayer bug that would panic is caught and reported as
`Error::Internal` (the song then renders silence); with `panic = "abort"` (the WebAssembly
build) it aborts instead. The repository's tests feed damaged and random input to a sample of
the replayers and all the detectors.

Examples (`cargo run --release --no-default-features --example NAME`): `play_to_wav` (module
file to WAV, no SDL), `list_formats`, `make_demo_mod`.

### Environment variables

The emulation reads a few `DELI_*` variables for debugging and for comparing with the original
players (`DELI_WRITE_LOG`, `DELI_DL_CPU`, ...). Leave them unset in a host; unset, the behaviour
is the documented one.

### Stability

0.x semver. Within `0.y.*` the documented items above keep their meaning; a `0.(y+1)` may change
them. Everything else (`engine`, `machine`, `replay`, `paula`, ...) is `#[doc(hidden)]`: public
only because the application, the WebAssembly and C crates and the tools in `tools/` use it.
The compiled-out `host-68k` code leaves the emulated machine (`machine`, `os`, `deli`, `heap`)
in place, because the open replayers load their modules and companion files through it.

## Command line without SDL: `amigaplayer-render`

```text
cargo build --release --no-default-features --features host-68k --bin amigaplayer-render
amigaplayer-render [--out FILE|-] [--format wav|raw] [--rate HZ] [--subsong N|first|default]
                   [--seconds S] [--silence S] [--no-songend] [--volume V] [--info] MODULE
```

A module to 16-bit stereo WAV or raw PCM, on a file or on standard output (the default), for
servers and pipes: `amigaplayer-render tune.ahx | ffmpeg -i - tune.mp3`. It links only the C
library, reads no configuration directory and needs no data files. Unlike `Song` it hosts 68k
code, so custom modules (`CUST.*`, which carry their own player) play; `--players`,
`--eagleplayers` and `--genies` add original players. The render stops at the player's song
end, after `--silence` seconds of silence (10) or after `--seconds` (600). `--info` prints one
line of JSON (`title`, `format`, `author`, `backend`, `subsong_min`, `subsong_max`,
`subsong_default`, `subsong`) and renders nothing; a render prints the same line on standard
error. Exit status 0 played, 1 not playable (missing, not recognised, no such subsong: nothing
is written to standard output), 2 wrong usage. A WAV on standard output carries the largest
length in its header, as streams do.

## C and other languages

```sh
cargo build --release -p amigaplayer-capi           # target/release/libamigaplayer.{so,a}
cc -O2 -Icrates/amigaplayer-capi/include examples/c/play.c -Ltarget/release -lamigaplayer \
   -Wl,-rpath,target/release -o play
./play tune.mod out.raw 60                          # raw s16le stereo, 44100 Hz
```

The header is `crates/amigaplayer-capi/include/amigaplayer.h` (hand-written, documented):
`ap_open` / `ap_open_with`, `ap_render_s16` / `ap_render_f32`, `ap_subsongs`, `ap_set_subsong`,
`ap_songend`, `ap_elapsed_secs`, `ap_duration_hint`, `ap_restart`, `ap_seek`, `ap_title`,
`ap_format`, `ap_detect`, `ap_close`, `ap_last_error` / `ap_last_error_code`, `ap_version`.
An `ap_song *` is an opaque handle; failures return NULL or a negative number and set a
per-thread error. Linking statically needs `-lpthread -ldl -lm`. Other languages bind to it with
their FFI (Python `ctypes`/`cffi`, C#, Go `cgo`, Zig, ...).

## WebAssembly

`crates/amigaplayer-wasm` builds `amigaplayer_wasm.wasm` with the `dp_*` ABI (decode a whole
tune into memory, copy the PCM out). Protocol, build and a JavaScript example:
[`crates/amigaplayer-wasm/README.md`](../crates/amigaplayer-wasm/README.md). The ABI is used by
a deployed web app and does not change.

## Checking it all

`tools/lib-check.sh` builds and tests the library with default features off, builds the
application, builds the C API and compares a C-rendered file with the Rust example's, builds
the wasm and runs the node smoke test.
