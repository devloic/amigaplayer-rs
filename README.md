# deliplayer

A Linux port of **DeliTracker 2.32** (Delirium Softdesign, AmigaOS, freeware since
2000), written in Rust with SDL3.

DeliTracker itself is closed source, so this is not a recompile. It reimplements
the main program, and runs the original 68k **DeliPlayers** (the plugins that
know the ~150 module formats) on an embedded 68020 emulator, the way UADE does
for EaglePlayers. It runs the **Eagleplayers** of Eagleplayer 2.06 too:

```
  module file ─▶ engine ──▶ native ProTracker ──────────────┐
                   │                                         ▼
                   └─▶ 68k DeliPlayer (m68k crate) ──▶ Paula emulation ──▶ mixer ─▶ SDL3 audio
                          │      ▲                    NoteStruct voices ─┘
                          ▼      │
                   HLE AmigaOS: exec, dos, utility, cia.resource, audio.device
                   DeliTrackerGlobals + dtg_* support functions
```

* `src/deli.rs`: the DeliPlayer interface (`DTP_*` tags, `DeliTrackerGlobals`,
  the `dtg_*` support functions), following the original developer kit.
* `src/eagle.rs`: the Eagleplayer additions: `EP_*` tags, the
  `EaglePlayerGlobals` (a separate block from DeliTracker's, since the two
  differ from `dtg_NotePlayer` on), the `ENPP_*` jump table below it, and the
  amplifier interface (`EP_InitAmplifier`, `ENPP_PokeAdr/Len/Per/Vol`,
  `ENPP_DMAMask`), whose writes go straight to Paula as in UADE's score.
* `src/engine.rs`: module identification and the player call sequence
  (Config → Check → ExtLoad → InitPlayer → Volume/Balance → InitSound →
  Interrupt/StartInt → …), subsongs, song end, mixing and filters.
* `src/machine.rs`: CPU, guest heap, A-line trap stubs, host→68k calls.
* `src/os.rs`: high-level emulation of the AmigaOS calls players make. Files
  map to the host filesystem, and unknown libraries become logged stubs.
* `src/paula.rs`, `src/paulaout.rs`, `src/cia.rs`, `src/bus.rs`: Paula's
  audio DMA state machine and its output stage (resampling and the A500/A1200
  and LED filters), the CIA timers, and the custom-chip address space, all on
  one colour-clock timebase (see Timing and sound).
* `src/notes.rs`: the renderer for `_note` players, which hand DeliTracker a
  NoteStruct instead of programming Paula, and the translation of that
  NoteStruct into the form a NotePlayer genie asks for. The structure is
  undocumented. Its layout was recovered by disassembling `SoundFX13_note` and
  the 4Voc, 8Voc and Headphone NotePlayer genies.
* `src/protracker.rs`: native ProTracker/NoiseTracker/SoundTracker replayer
  (it is built into DeliTracker rather than shipped as a plugin).
* `src/decrunch.rs`: PowerPacker (PP20) decrunching.
* `src/ui.rs`: SDL3 window with transport controls, scopes and module list.
* `crates/m68k-rc`, `crates/players-native`, `tools/recomp`,
  `tools/native-players.py`: the optional 68k → Rust translation of the players
  (see Setup).

The engine is also a library (`src/lib.rs`). With `default-features = false` it
builds without SDL3, which is how other programs embed it:

```toml
deliplayer = { path = "../deliplayerrust", default-features = false }
```

```rust
let mut e = deliplayer::engine::Engine::new(48000.0);
e.load_players(&deliplayer::default_players_dir().unwrap());
e.play_data(path, bytes)?;      // or play_file(path)
e.render(&mut buf);             // interleaved stereo f32
```

## Setup

```sh
cargo build --release
tools/fetch-deliplayers.sh        # downloads DeliTracker232.lha from Aminet,
                                  # installs ~/.config/deliplayer/DeliPlayers
tools/fetch-eagleplayers.sh       # optional: downloads Eagleplayer.lha (2.06),
                                  # installs ~/.config/deliplayer/Eagleplayers
```

`fetch-deliplayers.sh` also installs the DeliGenies of the archive in
`~/.config/deliplayer/DeliGenies`.

The player binaries are not part of this repository. They are copyrighted
freeware and are fetched from the official Aminet archives
(`mus/play/DeliTracker232.lha`, `mus/play/Eagleplayer.lha`). Needs `curl` and
`lhasa`, plus the SDL3 development libraries for building.

### Native translation (optional)

The players can also be machine-translated from 68k to Rust, so their calls run
as native code instead of on the interpreter:

```sh
tools/recomp/build.sh                      # builds m68k-recomp into target-recomp/
python3 -I tools/native-players.py \
    ~/.config/deliplayer/DeliPlayers ~/.config/deliplayer/Eagleplayers
cargo build --release
```

`tools/recomp` is a static 68000/68020 → Rust recompiler (from the
m68k-recomp project). `tools/native-players.py` runs it over every hunk file it
finds, each at its own fixed address in `$800000-$AE0000`, and writes
`crates/players-native/gen` (derived from the player binaries, so it is
.gitignored). Players that need a 68020 are translated in 68020 mode, and those
the recompiler cannot represent are left to the interpreter. `--only NAME,...`
and `--max-cpu 68000` narrow it down.

At load time a player whose file hash matches a translated one is placed at its
translated address and run through the generated code. The interpreter
(`crates/m68k-rc`) covers whatever the recompiler did not reach, and A-line traps
go to the HLE as before. The translation is used only while the translated
instruction bytes are unchanged, and a player that modifies its own code falls
back to the interpreter. Without `gen/` the build is the plain interpreter.

`DELI_NATIVE=0` keeps the native memory layout but interprets everything, which
is how the translation is checked: renders with and without it must be
byte-identical. `--wav` prints the share of instructions run natively.
`DELI_WRITE_LOG=FILE` logs every custom-chip and CIA write of a `--wav` render
(`ADDRESS VALUE COLOUR-CLOCK`, the format of the comparison log of the UADE
build below). `DELI_UADE=1` departs from the Amiga where UADE does, to compare
renders with it (see Timing and sound).

## Use

```sh
deliplayer [FILES/DIRS...]                       # player window
deliplayer --wav out.wav --seconds 60 MODULE     # render to WAV, headless
deliplayer --players DIR ...                     # other DeliPlayers dir
deliplayer --eagleplayers DIR ...                # other Eagleplayers dir
deliplayer --player NAME ...                     # only try this player
deliplayer --genies DIR ...                      # other DeliGenies dir
deliplayer --noteplayer 8Voices-NotePlayer ...   # another genie plays _note players
deliplayer --noteplayer builtin ...              # the built-in renderer plays them
deliplayer --genie NAME ...                      # run any other genie
```

Players are looked up in `--players DIR`, `$DELIPLAYERS`,
`~/.config/deliplayer/DeliPlayers`, then `./DeliPlayers`; Eagleplayers in
`--eagleplayers DIR`, `$EAGLEPLAYERS`, `~/.config/deliplayer/Eagleplayers`, then
`./Eagleplayers`. DeliPlayers are asked first, Eagleplayers last (after the
SoundTracker heuristic), so installing them only adds formats.
`--players /nonexistent` leaves the DeliPlayers out.

Genies are looked up in `--genies DIR`, `$DELIGENIES`,
`~/.config/deliplayer/DeliGenies`, then `./DeliGenies`, and are named by their
internal name (an unknown name lists them). `_note` players are played by
4Voices-NotePlayer, DeliTracker's default, when it is installed, and otherwise
by the built-in renderer (`--noteplayer builtin` chooses that one). A genie
plays them on Paula the way it would on an Amiga. 4Voices plays four voices, so
by default players with more go to the built-in renderer; `--noteplayer
8Voices-NotePlayer` or `32Voices-NotePlayer` mix them on the 68000 instead,
which takes seconds to start and a lot of emulated CPU time. The 4Voices, 8Voices, 14Bit,
32Voices and Headphone NotePlayers work. A genie that fails is dropped, and the
built-in renderer takes over.

You can drop files or folders onto the window, or use **Add** / **Dir**.

| Key | Action |
| --- | --- |
| Space | play / pause |
| Enter, double-click | play the selected module |
| Up / Down, PgUp / PgDn, mouse wheel | move through the list |
| Left / Right | previous / next subsong |
| P / N | previous / next module |
| S | stop |
| + / - | volume |
| R | random order |
| F | A500 / A1200 filter model |
| M | mono |
| O | open file dialog |
| Del | remove the selected entry |
| Q / Esc | quit |

**Next@End** moves on when a player signals song end. The **LED** button
honours the Amiga's LED filter, which players switch. **Sep-/Sep+** adjust the
stereo separation. **Mono** mixes both channels to mono.

`RUST_LOG=info` shows which player took a module. It also logs, once each, any
AmigaOS call that isn't implemented, which is where to look when a player
misbehaves.

## Status

Verified by rendering and comparing spectrograms against xmp:

* ProTracker modules (native replayer)
* FastTracker 2 XM and ScreamTracker 3 S3M (the original 68k players)
* AHX (the original 68k `_note` player)

All 85 players in the archive load, configure, and run their check routines
against every test file without crashing the host.

Not yet verified, for lack of test modules: most of the other formats (Future
Composer, SoundMon, TFMX, JamCracker, MED, …). Also unverified are players that
install their own CIA or audio interrupts instead of using DeliTracker's timer.

Eagleplayer 2.06: 112 of its 118 player files load (the four `AYPlayers` are
Z80 emulators for PlayAY, and BFB Master and Datatype have no module check).
Rendered over the test modules with no DeliPlayers loaded, 46 play audible
music, and where a DeliPlayer covers the same format most renders correlate at
0.9 or better with the DeliPlayer's. Eagleplayers that need a software mixer
(no `EPAMB_Direct` in their amplifier flags: Fasttracker II, PS3M, OctaMED, 8
voice Oktalyzer) or AHI (DigiBoosterPRO) are recognised but stay silent, and
those that run as a process of their own (PlayAY, MusiclineEditor) do not
work. TME's check accepts almost any large file, and Eagleplayers are asked in
file name order, so with no DeliPlayers installed it takes some modules that
later Eagleplayers would play.

Players that run as a process of their own (`DTP_Process`: IFF-8SVX, AIFF,
RIFF-WAV) work. The process is a guest task with its own stack that the host
suspends whenever it `Wait`s for a signal it has not received. DeliTracker's
calls reach it as DeliMessages on the port `DTP_MsgPort` names, and it resumes
when an `audio.device` write it waits for has played. `audio.device` queues
writes per channel and replies to each after `ioa_Cycles` passes through its
sample, following Paula's audio interrupts. It honours `ADIOF_PERVOL`,
`ADIOF_WRITEMESSAGE`, `ADIOF_SYNCCYCLE`, `CMD_STOP`/`CMD_START`,
`ADCMD_WAITCYCLE`, and allocation precedence: a higher precedence steals
channels, and the loser's request fails with `ADIOERR_CHANNELSTOLEN`. Processes
run in priority order. These sample players stream from the module file, which
they open through `dtg_PathArrayPtr`, so that holds the module's path when
`InitPlayer` runs.

NotePlayer genies are processes too. DeliTracker does not give them the
player's NoteStruct but one of its own, built from the genie's
`DTP_NotePlayer` record (voices per side, NSTF flags, maximum frequency). It
puts each channel on a side, converts periods to frequencies in Hz and word
lengths to byte lengths, and refills that NoteStruct before every
`DTP_NoteSignal`.

## Timing and sound

The aim is the sound of a real PAL Amiga 500. Everything runs on one clock in
colour clocks (3,546,895 Hz):

* Guest code takes the time a 68000 at 7.09 MHz takes: every instruction adds
  its cycles (68000 User's Manual tables, `crates/m68k-rc/src/cycles.rs`) and a
  custom-chip access happens at the time it is made. The generated code
  (`m68k-recomp --cycles`) and both interpreters count with the same table, so
  native and interpreted renders stay byte-identical. Chip RAM contention is
  not modelled. The time a player takes to get ready (AHX precalculates for
  about 10 s on a 68000) passes before the song starts, not as silence in it.
* Paula is UADE's audio state machine: DMA starts at the next line, one DMA
  word per channel per line, the length reload and its interrupt, attached
  channels, and the interrupt a channel without DMA raises. Fed the register
  writes and DMA fetches of a UADE run, it produces UADE's samples to the bit.
  It departs from UADE where UADE departs from the hardware: volume 64 is full
  volume (UADE plays 63), and periods below 16 are played as written (UADE
  raises them to 16).
* The output stage is UADE's default one: each channel's output averaged over
  the colour clocks of an output sample, hard left/right, then the A500 filter
  (fixed 6.2 kHz RC plus 20 kHz) or the A1200 one (none), and the LED filter
  (CIA-A PRA bit 1) on top. It matches UADE's `--write-audio` dumps to the bit.
  Formats from the PC (XM, S3M, 669, WAV, AIFF, MP3, TakeTracker) skip the
  filters, since their music was not made for them. **F** switches between the
  A500 and A1200 models.
* Frames are 313 lines (49.92 Hz vertical blank; UADE uses 312), the CIA
  timers count E-clock ticks (colour clock / 5), DeliTracker's timer
  interrupt comes every timer value + 1 E-clock ticks, and `dtg_WaitAudioDMA`
  (and `ENPP_DMAMask` switching DMA on) waits for 10 new raster lines, as
  UADE's score does (DeliTracker's own wait is not known). The built-in
  ProTracker waits 7 lines between switching a channel's DMA off and on.
* `_note` players go through a NotePlayer genie (4Voices by default) and so
  through Paula like everything else; the built-in renderer, used without one,
  keeps its own filters.

Compared with UADE (`DELI_UADE=1`: 312 lines, UADE's volume and period
quirks, hard panning) on the same player binaries, the tick timing matches to
the colour clock and the register writes of a tick land within a few dozen
colour clocks of UADE's. Renders still differ by a few samples of phase per
channel, because UADE's CPU is a 68020 at `cpu_speed=max` with UAE's cycle
tables, not an A500's 68000.

`src/paula.rs` and `src/paulaout.rs` follow UADE's `audio.c` and are derived
from GPL-2 code.

## Open replayers

`src/replay/` holds Rust ports of NostalgicPlayer's players (MIT, Thomas
Neumann: `LICENSE-NostalgicPlayer`), open-source stand-ins for the 68k players,
whose licences forbid modifying them or shipping them in another project.
`--open-replayers` plays the formats they know with them instead of the 68k
players. They write Paula's registers as the original players do, and are
checked against the Eagleplayers, not against NostalgicPlayer:

```sh
DELI_WRITE_LOG=ep.log deliplayer --player 'Pumatracker 1.0' --wav ep.wav MODULE
DELI_WRITE_LOG=rs.log deliplayer --open-replayers --wav rs.wav MODULE
python3 -I tools/replay-cmp.py ep.log rs.log   # tick by tick; first difference
```

`tools/replay-batch.py` does this for a list of modules and sorts them into
identical, differing and refused.

The write log's first line says where the module was loaded, so sample
addresses compare as offsets in the module. Within a tick the ports write all
at once where 68k code takes about a millisecond, so renders differ in phase
while the register writes are the same.

| Format | Port of | Checked against | Result |
|---|---|---|---|
| PumaTracker | NostalgicPlayer PumaTracker | Pumatracker 1.0 (Eagleplayer 2.06) | 60 of 61 modules identical for 20 s; `puma.toki7` plays a note past the period table, where the original reads its own variables (period `$5060`, silent) and the port writes 0 (silent) |
| SidMon 2.0 | NostalgicPlayer SidMon 2.0 | SIDMon 2.0 (Sidmon-II Eagleplayer V2.3, 22 Aug 97) | 109 of 109 modules identical for 20 s (the 110th file of the list is an XM); `Flipper - Title` plays note 80, past the period table, where the original reads its own pointers to the sample list, which the port keeps too; renders of 12 modules within a median 1.0 dB (specdist) of the reference |
| JamCracker | NostalgicPlayer JamCracker | JamCracker V1.33 (Eagleplayer; the DeliPlayer JamCracker_note has the same name, so run with `DELIPLAYERS` pointing elsewhere) | 63 of 63 modules identical for 20 s (12 more of the list are other formats) |
| FutureComposer1.3 | NostalgicPlayer Future Composer and its 1.3 converter | FutureComposer1.3 (Eagleplayer 2.02) | 148 of 152 modules identical for 20 s; 2 more are identical but for a sample address past the module's end, which `replay-cmp.py` takes for player memory; `pow2` (one tick, volume 0) and `Thr Demo Disk 17` read the player's sample table past its periods or its samples, whose contents depend on where the player was loaded |
| FutureComposer1.4 | NostalgicPlayer Future Composer | FutureComposer1.4 (Eagleplayer 2.02) | 113 of 114 modules identical for 20 s; `Tristar - Scoopex` plays sample 230 of 90 (at volume 0), which the original reads past its data hunk and the port as empty |
| SoundMon 2.0 | NostalgicPlayer SoundMon (its "1.1", mark `V.2`) | SoundMon (Eagleplayer V5.02) | 216 of 219 modules identical for 20 s; the two Green Berets are identical but for an empty sample at the module's end, which `replay-cmp.py` takes for player memory; `NR.1` plays an instrument past the 15, whose sample address the original reads past its table and the port takes as its empty sample (same lengths and volumes, here 0) |
| SoundMon 2.2 | NostalgicPlayer SoundMon (mark `V.3`) | SoundMon 2.2 (Eagleplayer V2.1) | 70 of 72 modules identical for 20 s; `Green Beret` as for 2.0; in `test3` a looped sample makes the original call into its own code (an ENPP call through the wrong register), which writes `$33` into the unused top byte of a sample address |
| Delta Music 2.0 | NostalgicPlayer Delta Music 2.0 | DeltaMusic 2.0 (Eagleplayer, V2.2) | 103 of 103 modules identical for 20 s and for 90 s; it reads the tables and the saved editor state from the module, as the original does, and spaces its writes as the original does (specdist mean 1.8 dB) |
| Fred Editor | NostalgicPlayer Fred Editor and its Fred Editor (Final) converter | Fred (Eagleplayer V2.3, 25 Aug 97), which plays only the final modules (the editor's replayer with the song), with a replayer of its own | 54 of 54 modules identical for 20 s and for 90 s, and on 9 later subsongs of 5 modules; it works on the replayer's variables and the voices' synth waveforms in the module, as the original does, and spaces its writes as the original does (specdist median 0.77 dB over the 54, 2.68 before); of the list, 2 Fuzzball modules have a replayer the Eagleplayer does not recognise, 4 are editor files (`Fred Editor `), which it does not play either |

Not implemented:

* the Eagleplayer mixing amplifier (AudioStructs without `EPAMB_Direct`), and
  AudioStruct interrupt handlers (`EPAMB_AudioInts`)
* most `ENPP_*` functions besides the DeliTracker ones, the amplifier pokes,
  `LoadExecutable` and `AllocSampleStruct` (the rest log once and return)
* the windows of GUI genies (scopes, analyzers, ModuleInfo): they load and run,
  but intuition and graphics are stubs, so they show nothing
* `ModSave-NotePlayer`, which saves the notes as a module file, fails in its
  process startup
* XPK decrunching
* the DataType player
