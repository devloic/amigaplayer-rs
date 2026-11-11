# Using amigaplayer-rs as a library

amigaplayer-rs is a player (`amigaplayer-rs`, SDL3) and a library. The library is the **open
replayers**: Rust ports of Amiga music replay routines that drive an emulated Paula, CIAs and
(for a few formats) a 68000 exactly as the original players do, the PC tracker formats of the
libxmp port, the built-in ProTracker with ProWizard's conversions of packed modules, a
decruncher for packed files, and an AIFF decoder. Bytes in, PCM out; no audio device, no files
read behind your back, deterministic output.

It does **not** include the hosting of the user's original 68k DeliPlayers / Eagleplayers
(the application's business), and has no SDL dependency. With the `host-68k` feature a `Song`
can also play **custom modules**, which carry their own 68k replay routine, when the caller
asks for it: see [Custom modules](#custom-modules).

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
| `Song::open_opts(name, bytes, companions, rate, &Options)` | as `open_with`, with `Options` (`Options::new().allow_custom(true)`: custom modules, `host-68k` builds) |
| `detect_custom(&[u8]) -> bool` | the bytes are a custom module (`host-68k` builds); nothing is run |
| `error() -> Option<&Error>` | why the song stopped by itself and renders silence: `Error::Guest` (a custom module's code failed), `Error::Internal` (a replayer panicked) |
| `render_f32` / `render_i16` | fill interleaved stereo; return frames written |
| `subsongs()` / `set_subsong(n)` | `Subsongs { first, last, current }`; out of range is an `Error` |
| `songend()` | the replayer reported the end (many never do: use a time limit and/or silence detection) |
| `duration_hint()` | seconds, for recordings and PC trackers; `None` for Amiga replayers |
| `elapsed_secs()` / `restart()` / `seek(secs)` | position; `seek` restarts and renders up to the point |
| `title()` / `format()` / `sample_rate()` | metadata |
| `Error` | `NotRecognised`, `Invalid(String)`, `InvalidRate(u32)`, `SubsongOutOfRange{..}`, `Internal(String)`, `Guest(String)`; `#[non_exhaustive]` |

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

### Custom modules

A custom module (`CUST.*`, `cust.*`: mostly game music ripped together with its player) is an
AmigaOS executable with a DeliTracker player header: the replay routine is 68k code inside
the file. `Song` plays one only when asked to, per open call, in a build with `host-68k`:

```toml
amigaplayer = { git = "...", package = "amigaplayer-rs", default-features = false, features = ["host-68k"] }
```

```rust
use amigaplayer::{detect, detect_custom, Options, Song};

let opts = Options::new().allow_custom(true);
let mut song = Song::open_opts("cust.Rodland", &bytes, &[], 48000, &opts)?;
assert_eq!(song.format(), "Custom");
song.render_i16(&mut pcm);
if let Some(e) = song.error() { /* Error::Guest: the module's code failed; silence from here */ }
```

`detect_custom(&bytes)` tells whether a file is one (the hunks are loaded and the tags read;
nothing is run). An open replayer that claims the file still plays it (`detect` names it);
`Song::open` and `open_with` keep refusing custom modules (`Error::NotRecognised`). Title,
format (`"Custom"`), subsongs, `set_subsong`, `songend`, `restart` and `seek` work as for any
song. Only the module's own code runs: a `Song` loads no DeliPlayer, Eagleplayer or DeliGenie,
and a file that needs one is not played. Not available in the WebAssembly crate.

**What bounds that code.** It runs on the interpreted 68000 of the emulated machine, and
reaches the host only through the emulated hardware and the emulated AmigaOS calls:

* *Time.* Every call from the host into guest code (InitPlayer, InitSound, each interrupt,
  each subsong change) has an instruction budget, counted over the call and what it nests
  (`src/machine.rs`: `CALL_BUDGET`, 200 million, checked in `run_until_return`). A `Song`
  lowers it to 20 million (`open::CUSTOM_CALL_BUDGET`, `src/open.rs`; minutes of a real
  68000's time, half a second to a second of the host's). A call that uses it up ends with
  "guest code did not return (PC=$...)". A module that loops for ever therefore costs one
  budget, once: `render` returns with all its frames.
* *Faults.* Every exception vector of the machine leads to a host stub (`Machine::new`), so
  an address error, a division by zero, a privilege violation or a `trap` ends the call with
  "CPU exception ..."; an illegal, A-line or F-line opcode ends it with "unhandled ..."; a
  `stop`, a return with the wrong stack pointer, guest calls nested more than 64 deep or a
  guest stack overflow end it likewise (`src/machine.rs`, `call`, `run_until_return`,
  `trap_inner`).
* *What happens then.* During `open_opts` the open fails with `Error::Invalid(reason)`. While
  playing, the engine stops the player (`Engine::fail`, `src/engine.rs`: audio DMA off, no
  further guest call) and `render_*` fills the rest of the buffer, and every later one, with
  silence; `Song::error()` is `Some(Error::Guest(reason))`, `songend()` stays false,
  `set_subsong` returns the error, `title`/`format`/`subsongs` keep answering, and
  `restart()` starts the song again.
* *Memory.* The machine has 11 MB of RAM (`bus::RAM_SIZE`), allocated once per `Song`; the
  guest's AllocMem takes from a heap inside it and returns 0 when it is full. Addresses are
  24 bits; outside the RAM, the CIAs and the custom chips, reads give 0 and writes are
  dropped (`src/bus.rs`): that is not a fault. The host side does not grow with what the
  guest asks for: hunk sizes beyond the RAM are refused (`src/hunk.rs`), dos Read/Write
  lengths are cut to the RAM size, the guest console is capped at 1 MB and an interrupt chain
  at 256 servers (`src/os.rs`).
* *Files.* With the option set the emulated dos.library sees the companion files handed to
  `open_opts` and nothing else (`OsState::no_host_files`, `src/os.rs`): no file or directory
  of the host is opened, looked up or created, whatever path the guest names; opening for
  writing fails. There is no network, clock or environment access in the emulated OS.
* *Panics.* A Rust panic anywhere under `open_opts`, `render_*` or `set_subsong` (an
  emulation bug reached by odd guest behaviour) is caught as for every other format:
  `Error::Internal`, silence from then on, `Song::error()` says so. Not with
  `panic = "abort"`.

What is *not* bounded: a module may be slow without failing (an interrupt routine that uses
most of its budget on every tick renders slower than real time), so a streaming host should
give rendering a deadline of its own; and the bounds above are those of an emulator written
for playing music, tested with damaged and random code (`tests/custom.rs`), not a security
boundary audited against hostile code. For files from untrusted uploads, render in a child
process (`amigaplayer-render`) with the operating system's limits.

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
library, reads no configuration directory and needs no data files. It hosts 68k code, so
custom modules (`CUST.*`, which carry their own player) play without being asked for, with
the engine's full instruction budget and the host's files visible to them; `--players`,
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
`ap_open` / `ap_open_with` / `ap_open_opts`, `ap_render_s16` / `ap_render_f32`, `ap_subsongs`,
`ap_set_subsong`, `ap_songend`, `ap_error` / `ap_error_code`, `ap_elapsed_secs`,
`ap_duration_hint`, `ap_restart`, `ap_seek`, `ap_title`, `ap_format`, `ap_detect`,
`ap_detect_custom`, `ap_close`, `ap_last_error` / `ap_last_error_code`, `ap_version`.
An `ap_song *` is an opaque handle; failures return NULL or a negative number and set a
per-thread error. Custom modules play through `ap_open_opts(..., AP_OPEN_CUSTOM)` (see
[Custom modules](#custom-modules); `ap_error` reports a module whose code failed); the
library is built with `host-68k` by default, and `--no-default-features` leaves the 68000
hosting out (the flag then fails with `AP_ERR_UNSUPPORTED`). API 0.2.0 (`ap_version()` 200)
added `ap_open_opts`, `ap_detect_custom`, `ap_error` and `ap_error_code`; nothing changed in
the functions of 0.1.0. Linking statically needs `-lpthread -ldl -lm`. Other languages bind to it with
their FFI (Python `ctypes`/`cffi`, C#, Go `cgo`, Zig, ...).

## WebAssembly

`crates/amigaplayer-wasm` builds `amigaplayer_wasm.wasm` with the `dp_*` ABI (decode a whole
tune into memory, copy the PCM out). Protocol, build and a JavaScript example:
[`crates/amigaplayer-wasm/README.md`](../crates/amigaplayer-wasm/README.md). The ABI is used by
a deployed web app and does not change.

## Checking it all

`tools/lib-check.sh` builds and tests the library with default features off and with
`host-68k`, builds the application, builds and tests the C API (with and without its
features) and compares a C-rendered file with the Rust example's, builds the wasm and runs
the node smoke test.
