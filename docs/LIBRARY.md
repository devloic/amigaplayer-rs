# Using amigaplayer-rs as a library

amigaplayer-rs is a player (`amigaplayer-rs`, SDL3) and a library. The library is the **open
replayers**: Rust ports of Amiga music replay routines that drive an emulated Paula, CIAs and
(for a few formats) a 68000 exactly as the original players do, the PC tracker formats of the
libxmp port, the built-in ProTracker with ProWizard's conversions of packed modules, a
decruncher for packed files, and an AIFF decoder. Bytes in, PCM out; no audio device, no files
read behind your back, deterministic output.

It does **not** include the hosting of the user's original 68k DeliPlayers / Eagleplayers
(a build-time option of the application, `--features host-68k`), and has no SDL dependency. With the `custom-modules` feature a `Song`
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

The optional song database reader (`crates/songdb`, feature `songdb`) is a port of
audacious-uade's (Matti Tiainen) and keeps its terms, LGPL-2.1-or-later OR GPL-2.0-or-later,
with the credit in each file. The **database itself is not in this repository** and is under
another licence, CC BY-NC-SA 4.0 (attribution, non-commercial, share-alike): see
[Song database](#song-database).

The optional C64 SID player (feature `sid`, off by default) is ported from code under the GNU
GPL **version 2 only**: a build with it is GPL-2.0-only as a whole. See
[C64 SID tunes](#c64-sid-tunes).

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
| `detect_with(name, &[u8], companions) -> Option<String>` | as `detect`, given the file name and companion files; where the bytes pass more than one replayer's check (Infogrames and Rob Hubbard 2 modules; the bank beside them decides) it starts the song to name the one that plays it |
| `formats() -> Vec<&'static str>` | the formats the library knows |
| `Song::open(name, bytes, rate)` / `open_with(.., companions, ..)` | start a module. Companions (TFMX `smpl.*`, Mark Cooksey `mcs.*`, IFF-SMUS `Instruments/*`, ...) are `(relative path, bytes)` pairs |
| `Song::open_opts(name, bytes, companions, rate, &Options)` | as `open_with`, with `Options` (`Options::new().allow_custom(true)`: custom modules, `custom-modules` builds) |
| `detect_custom(&[u8]) -> bool` | the bytes are a custom module (`custom-modules` builds); nothing is run |
| `error() -> Option<&Error>` | why the song stopped by itself and renders silence: `Error::Guest` (a custom module's code failed), `Error::Internal` (a replayer panicked) |
| `missing_calls() -> &[MissingCall]` | the AmigaOS library functions a custom module's code called that the host does not implement, each once, in the order first called; empty for other formats (see [Custom modules](#custom-modules)) |
| `render_f32` / `render_i16` | fill interleaved stereo; return frames written |
| `subsongs()` / `set_subsong(n)` | `Subsongs { first, last, current }`; out of range is an `Error` |
| `songend()` | the replayer reported the end (many never do: use a time limit and/or silence detection) |
| `duration_hint()` | seconds, for recordings and PC trackers; `None` for Amiga replayers |
| `elapsed_secs()` / `restart()` / `seek(secs)` | position; `seek` restarts and renders up to the point |
| `title()` / `format()` / `sample_rate()` | metadata |
| `MissingCall` | `library` (`"exec.library"`), `function` (`Some("AddTask")` where the host knows the name), `lvo` (`-282`), `fatal`; `Display` gives `exec.library/AddTask() (LVO -282)`; `#[non_exhaustive]` |
| `Error` | `NotRecognised`, `MissingCompanion { format, wanted }`, `Invalid(String)`, `InvalidRate(u32)`, `SubsongOutOfRange{..}`, `Internal(String)`, `Guest(String)`; `#[non_exhaustive]` |

`Error::NotRecognised` means that no replayer claims the bytes. A module a replayer does
recognise but cannot set up because files it reads beside the module are not there is
`Error::MissingCompanion { format, wanted }` instead: `format` is the replayer's name (the one
`detect` gives for the bytes alone: "TFMX Pro", "IFF-SMUS"), `wanted` the files it looked for
and did not find, in the order it tried them, as paths relative to the module, the form
`open_with` takes companions in. A host can show them, or fetch them and open again:

```rust
# use amigaplayer_rs::{Error, Song};
# let (name, bytes) = ("mdat.tune", amigaplayer_rs::testdata::minimal_mod());
match Song::open(name, &bytes, 48000) {
    Err(Error::MissingCompanion { format, wanted }) => eprintln!("{format}: needs one of {wanted:?}"),
    Err(e) => eprintln!("{e}"),
    Ok(_song) => {}
}
```

Where a format knows other names for the same file each is listed, and one of them is enough
(a TFMX Pro module `mdat.tune` names `smpl.tune`, then `smpl.set`, the samples of an early
song; `Tune.tfx` also `Tune.smpl` and `Tune.sam`). A format that stops at the first file it
misses names that one, and more may be missing after it (IFF-SMUS stops at its first missing
instrument, `Instruments/Piano.instr`). It is reported only when nothing else plays the module
either: a format whose check another replayer's module also passes still falls through to the
other players first.

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
the file. `Song` plays one only when asked to, per open call, in a build with `custom-modules` (which the application's default build has; `host-68k` implies it):

```toml
amigaplayer = { git = "...", package = "amigaplayer-rs", default-features = false, features = ["custom-modules"] }
```

```rust
use amigaplayer::{detect, detect_custom, Options, Song};

let opts = Options::new().allow_custom(true);
let mut song = Song::open_opts("cust.Rodland", &bytes, &[], 48000, &opts)?;
assert_eq!(song.format(), "Custom");
song.render_i16(&mut pcm);
if let Some(e) = song.error() { /* Error::Guest: the module's code failed; silence from here */ }
for c in song.missing_calls() { eprintln!("warning: the module called {c}"); }
```

**Calls the host does not implement.** The emulated AmigaOS has exec, dos, utility, a few
functions of intuition, the CIA resources, audio.device and timer.device; graphics.library
and every other library are stubs. A function it does not implement answers 0 and does
nothing else. `Song::missing_calls()` lists each one the module called (at most 64): the
library, the function's name (from the libraries' interface files, for exec, dos, graphics,
intuition and the device vectors; otherwise only the offset) and the offset. Most are
warnings: the module plays on, perhaps differently than on an Amiga. `MissingCall::fatal`
marks the few the code cannot go on without (exec's scheduler entries `Schedule`,
`Reschedule`, `Switch`, `Dispatch`, and `Exception`, `ColdReboot`: the caller waits for
something that never happens): such a call stops the song as a fault does, and
`Song::error()` is `Error::Guest` naming it. The list is kept across `set_subsong`; `restart`
and `seek` start it again.

Tasks the code starts with exec `AddTask` (on the stack it set up in the Task structure) run as
dos `CreateNewProc` processes do: at once until they first wait, then whenever they have
received a signal they wait for (after each interrupt, or while the code that started them
waits); `RemTask` ends one. timer.device `TR_ADDREQUEST` sent with `SendIO`/`BeginIO` is
answered when its time has passed on the emulated clock (`UNIT_VBLANK` at a vertical blank),
to a signal or a software-interrupt port.

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

### Song database

With the `songdb` feature (off by default) `amigaplayer::songdb` answers, for a module file:
how long each subsong is, how that length was found, the format, and author, album, publisher
and year where known. The answers come from the song database of
[audacious-uade](https://github.com/mvtiaine/audacious-uade) (some 476,000 files of the
Modland, AMP, UnExoticA, ... collections).

```toml
amigaplayer = { git = "...", package = "amigaplayer-rs", default-features = false, features = ["songdb"] }
```

```rust
use amigaplayer::songdb::{key, SongDb};

let db = SongDb::open("/usr/share/audacious-uade/songdb")?;   // std::io::Result
if let Some(info) = db.lookup(&bytes) {                        // or db.lookup_key(&key(&bytes))
    println!("{} / {} / {} ({})", info.author, info.album, info.publisher, info.year);
    if let Some(s) = info.subsong(song.subsongs().current) {
        println!("{} ms, ended by {}", s.length_ms, s.status);  // "94760 ms, ended by player+volume"
    }
}
```

| item | |
|------|--|
| `SongDb::open(dir) -> io::Result<SongDb>` | loads the four files of `dir` into memory: about 19 MB and 0.2 s for the real database. A missing file is `NotFound`, one that does not parse `InvalidData`; never a panic |
| `lookup(&[u8]) -> Option<Info>` / `lookup_key(&str)` | by the file's bytes, or by its key. `&self`: one database serves all threads (`Send + Sync`) |
| `key(&[u8]) -> String` | the key: XXH32 of the first 256 KiB and the low 16 bits of the length, 12 hex digits. Of the file as it is on disk (packed files are not unpacked first) |
| `Info` | `key`, `subsongs: Vec<SubSongInfo>`, `format`, `channels`, `author`, `album`, `publisher`, `year`; empty strings and 0 where the database knows nothing; `subsong(n)` |
| `SubSongInfo` | `subsong`, `length_ms` (steps of 20 ms), `status: SongEndStatus`, `is_duplicate` (the same audio as an earlier subsong) |
| `SongEndStatus` | how the measured song ended: `Player`, `DetectSilence`, `DetectLoop`, `PlayerPlusVolume`, ...; `as_str()` / `Display` give the database tools' names ("player", "silence", "loop", "player+volume", ...) |

**The data.** The database is the directory `conf/songdb` of audacious-uade (`xxh32idx.tsv`,
`songlengths.tsv`, `modinfos.tsv`, `metadata.tsv`, about 10 MB; its packages install it as
`share/audacious-uade/songdb`). It is licensed CC BY-NC-SA 4.0, which is why it is not
committed here, not compiled in and not downloaded by any build step: you obtain it, you name
its directory at run time, and its terms (attribution, non-commercial use, share-alike) are
yours to meet. The tests use a synthetic database written by
`testdata::write_songdb` (and the example `make_demo_songdb`); nothing in the repository
comes from the real one.

**What a length means.** The lengths were measured by audacious-uade's tools with *their*
replayers (UADE, libopenmpt, ...), as the time to the song end, or to a detected loop, silence
or volume fade. This library's replayers are different programs: its own song end is usually
within a fraction of a second of the database's, not equal to it. A length is right for a
progress bar or a play list; as a place to cut the audio, cut at silence near it (what
`amigaplayer-render --stop-at songdb` does). A subsong whose status is `Error`, `None` or
`NoSound` was not measured usefully. The database's subsong numbers are the format's own, as
`Song::subsongs()` counts for the same replayer; where a replayer here numbers a format
differently the entry is simply not found by `Info::subsong`.

The reader is the crate `crates/songdb` (`amigaplayer-songdb`, no dependencies): a port of
audacious-uade's `src/songdb`, kept under its dual licence, usable by itself. Not built into
the WebAssembly crate.

### C64 SID tunes

With the `sid` feature (off by default) `detect` and `Song` also take C64 SID tunes (`PSID` and
`RSID` files, `.sid`), played on an emulated C64 with a real SID chip model: a port of
NostalgicPlayer's C# ports of libsidplayfp and reSIDfp (`src/sid/`). **Those ports are under
the GNU GPL version 2 and do not say "or any later version": a program built with `sid` is a
GPL-2.0-only work as a whole** (`LICENSE-LibSidPlayFp`, `LICENSE-LibReSidFp`), which is why
the feature is off by default.

```toml
amigaplayer = { git = "...", package = "amigaplayer-rs", default-features = false, features = ["sid"] }
```

`format()` is `"PSID"` or `"RSID"`; the subsongs are the file's songs (0-based, starting at its
start song); a SID tune has no end, so `songend()` stays false (unless the emulated CPU stops on
a halt instruction) and the caller decides how long to play. Stereo at the song's rate, the
same samples on every run and platform. No Commodore ROM is included: an RSID tune that needs
the BASIC ROM is refused (`Error::Invalid`). Not in the C API or WebAssembly builds.

### Environment variables

The emulation reads a few `DELI_*` variables for debugging and for comparing with the original
players (`DELI_WRITE_LOG`, `DELI_DL_CPU`, ...). Leave them unset in a host; unset, the behaviour
is the documented one.

### Stability

0.x semver. Within `0.y.*` the documented items above keep their meaning; a `0.(y+1)` may change
them. Everything else (`engine`, `machine`, `replay`, `paula`, ...) is `#[doc(hidden)]`: public
only because the application, the WebAssembly and C crates and the tools in `tools/` use it.
The compiled-out `host-68k` and `custom-modules` code leaves the emulated machine (`machine`, `os`, `deli`, `heap`)
in place, because the open replayers load their modules and companion files through it.

## Command line without SDL: `amigaplayer-render`

```text
cargo build --release --no-default-features --features "custom-modules songdb sid" --bin amigaplayer-render
# the original 68k players too (--players, --eagleplayers, --genies, --libs, --68k-first): add host-68k
amigaplayer-render [--out FILE|-] [--format wav|raw] [--rate HZ] [--subsong N|first|default]
                   [--seconds S] [--silence S] [--no-songend] [--volume V] [--info]
                   [--songdb DIR] [--songlengths FILE] [--stop-at songdb|songlengths|default] MODULE
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
is written to standard output), 2 wrong usage. A module that no player recognises is
`unknown module format`; one that a replayer recognises but cannot play for want of the files
beside it names the format and what it looked for, e.g. `IFF-SMUS module, but a file it needs
beside it is missing (looked for: Instruments/AcousticGuitar.ss)` (the player window shows the
same text).

In builds with the `archives` feature (the default of the applications) a companion looked for
on disk that is not there as named is also found compressed beside it, `NAME.xz` or `NAME.gz`
(in any case), and read unpacked: archive collections keep every file so (`mdat.tune.xz`
beside `smpl.tune.xz`, `song.smus.xz` beside `Instruments/Piano.instr.xz`). The module itself
may be such a wrapper as before. `Song::open_with` reads no disk: hand it the companions
unpacked. A WAV on standard output carries the largest
length in its header, as streams do.

Built with the `songdb` feature as well (`--features "host-68k songdb"`), `--songdb DIR` looks
the file up in the [song database](#song-database) in `DIR` and adds a member to the JSON
(`null` if the database does not know the file; strings and numbers it does not have are
`null`):

```json
"songdb":{"key":"503a5ac60f27","duration_ms":94760,"songend":"player+volume","author":"Pink",
  "album":"Cruisin'","publisher":"Abyss","year":1997,"format":"AHX v2","channels":null,
  "subsongs":[{"subsong":0,"duration_ms":94760,"songend":"player+volume","duplicate":false}]}
```

`duration_ms` and `songend` are those of the subsong being rendered (`null` if the database
has no entry for it). `--stop-at songdb` uses that length as a place to stop: once it has
passed, the render ends where the output falls digitally silent for half a second, and none
of that silence is written (the output ends at the length, or where the silence began if that
is later). It never stops earlier than the length, and it does not cut a song that keeps
sounding: the other stops (song end, `--silence`, `--seconds`) apply as before. A length of 0
or with the status `error`, `none` or `nosound` is not used. For example `AHX.Cruisin` (the
entry above) renders 94.92 s by the player's own song end and 94.76 s with `--stop-at songdb`.
Loading the database costs each run about 0.2 s and 19 MB. A directory that is not a database
is exit status 1; `--songdb` in a build without the feature is exit status 2 with the reason.

`--songlengths FILE` does the same for C64 tunes (a build with the `sid` feature) with HVSC's
`DOCUMENTS/Songlengths.md5` (not shipped with this program): the file's MD5 is looked up and
the length of the subsong in the list is added to the JSON as `"songlength_ms"` (`null` if the
tune or that subsong is not listed). `--stop-at songlengths` stops the render at that length,
exactly: a SID tune never ends or falls silent by itself, so there is no silence to wait for.
The other stops apply as before, and a tune not listed renders as without the option. All 628
tunes of HVSC Update 85 that start are found; a two-song tune listed `3:17.645 0:03.467`
renders 197.645 s and 3.467 s. The same lookup is in the library as `songlengths::SongLengths`.

## C and other languages

```sh
cargo build --release -p amigaplayer-capi           # target/release/libamigaplayer.{so,a}
cc -O2 -Icrates/amigaplayer-capi/include examples/c/play.c -Ltarget/release -lamigaplayer \
   -Wl,-rpath,target/release -o play
./play tune.mod out.raw 60                          # raw s16le stereo, 44100 Hz
```

The header is `crates/amigaplayer-capi/include/amigaplayer.h` (hand-written, documented):
`ap_open` / `ap_open_with` / `ap_open_opts`, `ap_render_s16` / `ap_render_f32`, `ap_subsongs`,
`ap_set_subsong`, `ap_songend`, `ap_error` / `ap_error_code`, `ap_missing_call` /
`ap_missing_call_fatal`, `ap_elapsed_secs`,
`ap_duration_hint`, `ap_restart`, `ap_seek`, `ap_title`, `ap_format`, `ap_detect`,
`ap_detect_custom`, `ap_close`, `ap_last_error` / `ap_last_error_code` / `ap_last_missing`,
`ap_version`, and the `ap_songdb_*` functions. `Error::MissingCompanion` is
`AP_ERR_MISSING_COMPANION` (9): `ap_last_missing(i)` gives the `i`-th file looked for and not
found (NULL past the last), and `ap_last_error()` names them too.
An `ap_song *` is an opaque handle; failures return NULL or a negative number and set a
per-thread error. Custom modules play through `ap_open_opts(..., AP_OPEN_CUSTOM)` (see
[Custom modules](#custom-modules); `ap_error` reports a module whose code failed); the
library is built with `custom-modules` by default, and `--no-default-features` leaves the 68000
hosting out (the flag then fails with `AP_ERR_UNSUPPORTED`). API 0.2.0 (`ap_version()` 200)
added `ap_open_opts`, `ap_detect_custom`, `ap_error` and `ap_error_code`; nothing changed in
the functions of 0.1.0. API 0.3.0 (`ap_version()` 300) added `AP_ERR_MISSING_COMPANION` and
`ap_last_missing` (a module recognised without its companions was `AP_ERR_INVALID` before).
API 0.4.0 (`ap_version()` 400) added `ap_missing_call(song, i)` (the `i`-th function a custom
module called that the host does not implement, as `"exec.library/AddTask() (LVO -282)"`,
NULL past the last) and `ap_missing_call_fatal(song, i)` (1: it stopped the song and
`ap_error` names it, 0: a warning, -1 past the last), as `Song::missing_calls` of the Rust
API; nothing else changed. Linking statically needs `-lpthread -ldl -lm`. Other languages bind to it with
their FFI (Python `ctypes`/`cffi`, C#, Go `cgo`, Zig, ...).

The [song database](#song-database) is in a library built with the `songdb` feature (off by
default; without it the functions exist and `ap_songdb_open` fails with `AP_ERR_UNSUPPORTED`):

```sh
cargo build --release -p amigaplayer-capi --features songdb
```

```c
ap_songdb *db = ap_songdb_open("/usr/share/audacious-uade/songdb");
ap_songdb_info info;
if (db && ap_songdb_lookup(db, data, len, &info) == 1 && info.subsong_count > 0)
    printf("%s, %u ms (%s)\n", info.author, info.subsongs[0].length_ms,
           ap_songdb_status_name(info.subsongs[0].status));
ap_songdb_free(db);
```

`ap_songdb_open` / `ap_songdb_free`, `ap_songdb_count`, `ap_songdb_lookup` /
`ap_songdb_lookup_key` (1 found, 0 not found, -1 bad argument), `ap_songdb_key`,
`ap_songdb_status_name`. The strings and the subsong array of an `ap_songdb_info` belong to
the database handle and are valid until the next lookup on it; lookups on one handle must not
run concurrently. The data is not part of the library (CC BY-NC-SA 4.0, see above).

## WebAssembly

`crates/amigaplayer-wasm` builds `amigaplayer_wasm.wasm` with the `dp_*` ABI (decode a whole
tune into memory, copy the PCM out). Protocol, build and a JavaScript example:
[`crates/amigaplayer-wasm/README.md`](../crates/amigaplayer-wasm/README.md). The ABI is used by
a deployed web app and does not change.

## Checking it all

`tools/lib-check.sh` builds and tests the library with default features off, with `custom-modules`,
with `host-68k` and with `host-68k songdb` (and the song database crate by itself, and the renderer's
`--songdb` on a synthetic database), builds the application, builds and tests the C API (with
and without its features) and compares a C-rendered file with the Rust example's, builds the
wasm and runs the node smoke test.
