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
  (it is built into DeliTracker rather than shipped as a plugin), a translation
  of ProTracker 2.3D's replay routine after 8bitbubsy's pt2-clone and PT2.3F
  (BSD 3-Clause: `LICENSE-pt2-clone`; see Built-in ProTracker).
* `src/decrunch.rs`: PowerPacker (PP20) decrunching.
* `src/prowizard/`: packed ProTracker modules converted back to ProTracker
  (see ProWizard).
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
0.9 or better with the DeliPlayer's. Those that ask for the Eagleplayer
amplifier with more than 4 voices or without `EPAMB_Direct` (Fasttracker II,
PS3M with its S3M, MTM and 5-32 channel MODs, OctaMED, Oktalyzer, Digibooster,
8-voice Art of Noise) play all their voices through the host's amplifier
(`src/amplifier.rs`), a clean-room mixer "compatible with the 8 Bit
Amplifier": it keeps up to 32 virtual voices (Paula's semantics when poked,
the AudioStructs otherwise), mixes them in integers into two 8-bit buffers in
chip memory, and plays those on Paula channels 0 (left) and 1 (right) by DMA,
double-buffered, the next one mixed on channel 0's audio interrupt. Its
settings are the 8 Bit Amplifier's factory defaults: mix rate 28000 Hz less
500 for every voice past 2 (24978 Hz for 8), LRRL sides where the player sets
none, boost 1.0, 1.1 from 5 voices, 1.2 from 9 and so on; so these play quieter than a
4-voice module, and with 8-bit output. With `DELI_WRITE_LOG` the voices'
pokes are logged before mixing, at `0xD00000 | voice << 4 | register`
(register 0/2 LOC, 4 LEN, 6 PER, 8 VOL, `0xA` DMA, `0x8000` on), and
`tools/replay-cmp.py` compares them as `VLOC0`.. `VDMA31`; the amplifier's own
Paula writes are not logged. Players with 4 Direct voices or fewer still poke
Paula itself, as before. DigiBoosterPRO needs AHI and stays silent, and PlayAY,
which runs as a process of its own, does not work. MusiclineEditor (a process
player too) plays: its InitPlayer branches on the flags `dtg_AudioAlloc`
returns, which the host sets as a `moveq #0,d0` would.

TME (V1.1) claims many files of other formats and plays them silent: its
`EP_Check5` looks for 256 zero bytes after the module's tables, or, in a file
too short for that, at the words past the file's end, and never compares what
it computes with `dtg_ChkSize`, as the later TME player UADE ships does. That
is the player's own check, not the host's: in the coverage sample (79 files of
498) no other Eagleplayer 2.06 takes those files when TME is removed, so they
would not play either way. Some formats have an Eagleplayer 2.06 that does not
know the variant the archive holds: OctaMED and MED refuse MMD3 modules in
mix mode (`flags2` bit 7, all of OctaMED SoundStudio's `oss.*`; the open replayers
`OctaMED SoundStudio` and `OctaMED` play them), MusicMaker8
the single-file `FORM MMV8` modules, and Mark Cooksey, SoundMaster, Sound
Images, Jesper Olsen and Sean Conran the replayer variants of the files they
refuse (their checks match code or data patterns of other variants; UADE plays
them with later versions of these players).

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
  ProTracker waits as ProTracker 2.3D does, a DBRA loop of 300 (1515 colour
  clocks, 6.7 lines on a 7 MHz 68000), before switching the new notes' DMA on,
  and again before setting the repeats.
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

## Built-in ProTracker

`src/protracker.rs` translates ProTracker 2.3D's replay routine, after
8bitbubsy's pt2-clone (its C port of the 2.3D replayer) and his PT2.3F
re-source of 2.3D (BSD 3-Clause, `LICENSE-pt2-clone`). The engine calls it
from DeliTracker's timer interrupt, and it writes Paula's registers through the
bus as the 68k routine does, in its order:

* a row: per voice the period when the previous note long was empty, DMA off,
  length, pointer and period of a new note, the period or volume its effect
  sets, its volume; then 2.3D's DMA wait (a DBRA loop of 300, 1515 colour
  clocks), DMA on for the new notes, the wait again, and the repeat pointer and
  length of all four voices, the last voice first. The first row comes on the
  sixth interrupt, as in ProTracker;
* between rows, each voice's effect writes and volume;
* the writes spaced as the 68000 takes from one to the next (estimated from
  the cycle counts of the 2.3 code paths, not measured);
* periods from ProTracker's tables (16 finetunes, and what an arpeggio on
  finetune -1 reads past them), the LED filter on at the start (as an Amiga
  starts and ProTracker leaves it; E0x switches it), a new tempo from the next
  tick (the CIA latch), and 2.3D's quirks: 9xx added twice, B after D on one
  row clearing the break row, E6x and EEx on delayed rows, the tremolo ramp
  testing the vibrato position, the 12-bit wrap of portamentos, EFx inverting
  the sample in memory, F00 ending the song. E8x does nothing (pt2-clone's
  default, PT2.3F). An empty sample's note takes the replay's null-pointer path
  (length 1, no repeat), as in the 2.3D editor, where it has no memory.

Checked against UADE's PTK-Prowiz (a ProTracker 3.0b replay): in uade123 for
plain modules (its check wants the module type UADE's uade.library passes;
A500 filter, LED forced on unless the module uses E0x, as UADE starts it off),
in this engine for the ProWizard families (see ProWizard); 20 s each, 300
random archive MODs, NostalgicPlayer's ProTracker and NoiseTracker test files
and 25 or 15 of each SoundTracker folder's, 638 played (59 the reference
refuses, 20 other players take):

| | State identical | The same writes in the same order, but the repeats' order | specdist median, to 2 kHz |
| --- | --- | --- | --- |
| before | 392 | 0 | 5.20 dB, 2.01 |
| now | 480 | 329 | 2.43 dB, 2.06 |

(7 more files, StarTrekker modules with no tick to compare, count as identical
in both.) Strict order never matches whole files: PTK-Prowiz sets the repeats
first voice first, 2.3D last voice first. It also plays SoundTracker modules in
a NoiseTracker-like mode (the period in every effects tick), so they match in
state only. Of the periods that still differ first, most are PTK-Prowiz
starting a portamento a tick later than 2.3D does; others are 0-length
samples, which PTK-Prowiz does not play. Its DMA waits are 10 raster lines,
2.3D's 1515 colour clocks.

8-channel MODs (`CD81`, `8CHN`, `FLT8`) are not ProTracker's: DeliTracker's
TakeTracker and PS3M players take them and mix in software, so the built-in
replayer leaves them alone.

## Open replayers

`src/replay/` holds Rust ports of NostalgicPlayer's players (MIT, Thomas
Neumann: `LICENSE-NostalgicPlayer`; the AHX one is a port of 8bitbubsy's
ahx2play, BSD 3-Clause: `LICENSE-ahx2play`; the HivelyTracker one follows Pete Gordon's
hvl_replay, BSD 3-Clause: `LICENSE-hivelytracker`), open-source stand-ins for the 68k players.
DeliTracker's DeliPlayers and genies may not be modified or shipped in another
project; Eagleplayer's main program and engines are GPL since 2.04, but its
external Eagleplayers are not ("The external Eagleplayers are not released under
the GPL", Eagleplayer 2.04 sources), and most of them hold the composers' own
replay routines.
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
| SoundFX1.3 | NostalgicPlayer SoundFX and its 1.x converter (mark `SONG`) | SoundFX 1.3 (Eagleplayer V2.1, 20 Dec 96) | 327 of 327 modules identical for 20 s; renders of 12 modules within a median 0.96 dB (specdist) of the reference (47.7 dB with no time between the writes); where a DMA off to on gap is shorter than a word at the voice's period, the restart can differ (the reference's ticks start later, by its init time), as in `AA Demo 1` (3.8 dB) |
| SoundFX2.0 | NostalgicPlayer SoundFX (mark `SO31`) | SoundFX 2.0 (Eagleplayer V2.1, 20 Dec 96) | 89 of 89 modules identical for 20 s; renders of 12 modules within a median 1.35 dB (specdist) of the reference (25.9 dB with no time between the writes). The original tests the flags `dtg_GetListData` returns, which the host now sets as UADE's does |
| Delta Music 1.0 | NostalgicPlayer Delta Music 1.0 | DeltaMusic 1.0 (Eagleplayer V2.1) | 9 modules (1 more file of the list is another format): 1 identical for 20 s; the other 8 differ only in the period, at volume 0, of a voice that has not played a note yet, whose arpeggio the original reads from its code hunk's pointer to its data hunk, so from where it was loaded. With the reference's pointer in that place all 9 are identical for 20 s and for 90 s. The variables live in guest memory in the original's layout, and the writes come within a few colour clocks of the original's; specdist mean, median of the 9: 0.30 dB (0.03 with the reference's pointer, 8.4 without the waits) |
| Sonic Arranger | NostalgicPlayer Sonic Arranger and its "final" converter | SonicArranger (Eagleplayer V4.11; the DeliPlayer SAplayer_note has the same name, so run with `DELIPLAYERS` pointing elsewhere) | 263 of 263 modules identical for 20 s ("SOARV1.0", offsets-only and with the replayer in front; the 43 others of the list are lh-packed `@OARV1.0`, Lionheart's, or other formats, which the original refuses too), subsongs too; writes spaced by the original's code-path costs fitted to its logs; renders of 10 modules within a median 0.94 dB (specdist; 1.44 dB with flat gaps). The song end comes when the last position is over, as the later SonicArranger Eagleplayer (UADE's) signals it; the original signals it on entering the last position, so a one-position song (`sa.199`, Marvin's Marvellous Adventure) ended after 0.1 s, and now plays its 7.8 s; the 263 stay identical for 20 s played on past either side's song end. The Lionheart offsets-only files with 2, 3 or 5 subsongs (`sa.level_6`, `sa.endgegner34.SA`) or an empty last part (`sa.sfx`) the original refuses (its check takes a subsong table only when its size's low byte has no bits but 2 and 3, so one subsong), and the port takes them as UADE's newer player and NostalgicPlayer's converter do (no such test): 29 modules of the 40 `sa.*`/`*.sa` files of the archive that the port refused before (the others are 8 lh-packed, 2 XMs and `sa.hole2`, not Sonic Arranger). Their master volume starts at 64 and a note starts with period 1, the interrupt cleared and DMA off, as UADE's log shows; of the 29, 12 write the same audio registers as UADE for 20 s (times and sample addresses aside) and 17 differ (a voice's DMACON is written with 0 for a voice that is off, and a row starts a tick later than the port's, in ways not worked out); the modules taken before stay identical |
| SidMon 1.0 | NostalgicPlayer SidMon 1.0 | SIDMon 1.0 (Eagleplayer V4.01, adapted by DEFECT; the DeliPlayer Sidmon1.0_note plays these first, so run with `DELIPLAYERS` pointing elsewhere) | 37 of 52 modules identical for 20 s; the other 15 read periods past the table (mostly instrument 0, 32 bytes before the instruments, with a stray finetune), where the original reads past its code hunk and the port writes no period; with those masked all 52 are identical. It spaces its writes as the original does; renders of 12 modules within a median 1.8 dB (specdist), 1.0 dB for the 7 of them that are identical |
| TFMX Pro | NostalgicPlayer TFMX (its engine for all three TFMX kinds), following the original's replayer | TFMX_Pro (Eagleplayer, "TFMX_Pro player module V1.4") | 173 of 174 modules identical for 20 s (`mdat.NAME` with `smpl.NAME` beside it, old fixed and offset-table layouts, sound effects modules), subsongs too; 73 of 73 more whose text is the editor's "(Empty) " (as the Eagleplayer plays them with the text changed: unchanged, it takes only four it knows and leaves the others to TFMX_1.5, which refuses them); `Masterblazer ingame` differs from tick 578 in a random play macro, whose random numbers the original takes from the beam position (`VHPOSR`), so from the exact time of its code. The voice, track and global variables are kept in the original's byte layout; macro DMA ons and the periods go out at the end of the tick after its 9-line wait, a DMA off with a second byte at the start of the next one, as the Eagleplayer's default mode does; writes spaced by code-path costs fitted to its logs; renders of 10 modules within a median 0.96 dB (specdist) |
| TFMX 1.5 | NostalgicPlayer TFMX, following the original's replayer | TFMX_1.5 (Eagleplayer, "TFMX 1.6 player module V1.91"), an older replayer of its own (26 macro commands, fixed tables, Paula through `ENPP_Poke*`) | 18 of 18 modules identical for 20 s (Rock'n'Roll), subsongs too; the single-file TFHD form the Eagleplayer also takes is not supported; writes spaced by code-path costs fitted to its logs; renders of 10 modules within a median 1.83 dB (specdist); also the early `TFMX-SONG ` modules of the fixed tables with no `smpl.NAME` beside them but one `smpl.set` (Einmal Kanzler sein, 90 `mdat.*`), which the Eagleplayer refuses, as UADE's TFMX 1.5 player plays them (no fade to $3F, the detune added to SetNote, DMA on at the macro's command): 89 of 90 write the same audio registers as UADE for 20 s (times aside), `ingame_22` differs from the tenth tick (a voice's note starts later); the 90 are the ones this build refused before |
| TFMX 7V | NostalgicPlayer TFMX, following the original's replayer | TFMX_7V (Eagleplayer, "TFMX 7V Eagleplayer V2.35"), the Pro replayer with voices 4-7 mixed in software into a buffer that channel 3 plays, its tick run from channel 3's audio interrupt | 16 modules, none identical for 20 s: 15 differ only at start-up as below, `TRSI` as further on (compared with `--tick-gap 8000`: a tick's writes come in two groups, the next buffer's and, after the mixing, the tick's). The original's voice variables start with what was left in its data hunk when it was saved, which the port does not carry: it starts them at 0. So every tick writes the volume and period of a Paula channel that has not played a note yet as 0 where the original writes those leftovers (DMA off, silent): in all 15 playing modules every differing write is one of these, from the first ticks to as long as the channel stays unused (to the end of the 20 s in 5 of them); the register writes are otherwise the same tick by tick. Channel 3's DMA bytes, the mixed buffers, are identical to the original's for 14 of 15; in `Turrican 2 - Title` the first SetPrevNote of a mixed voice reads note 0, not the leftover, and three buffers (489-491, about 8 s in) differ until its next note. Later subsongs: 2 of 26 identical, the others the same start-up difference, and in subsong 1 of `Turrican 2 - Title` the first SetPrevNote of two Paula voices gives period $78 for two ticks each. `TRSI` (NostalgicPlayer's set) asks for a mixing rate of 0, for which the original mixes 65536 bytes into its 480-byte buffer, over the memory past it, and writes nothing more; the port mixes none. The single-file TFHD form the Eagleplayer also takes is not supported; the 276 other TFMX modules of the list it refuses, and so does the port. Writes spaced by code-path costs fitted to its logs (the mixing about 141 colour clocks a byte); renders of 12 modules within a median 0.39 dB (specdist) |
| IFF-SMUS | NostalgicPlayer IFF-SMUS | IFF-SMUS (Sonix player module V2.2 Eagleplayer, adapted by Delirium & DEFECT; the DeliPlayer of the same name plays these first, so run with `DELIPLAYERS` pointing elsewhere) | 269 of 270 songs identical for 20 s (with their `Instruments` folders; 29 more of the list are duplicates); `Amazing 3` names an instrument `How?` that its folder has as `How!`, which both refuse. It loads the instruments (synthesis, sampled sounds and their `.ss` files, IFF 8SVX) into guest memory through the original's allocations, and spaces its writes and its start as the original does (code-path costs fitted to the logs, rms 17 colour clocks); renders of 10 songs within a median 1.8 dB (specdist; the synthesis voices are as sensitive to a few clocks: the port against itself started 2 clocks later is 1.1 dB off on `smus.DELTA`) |
| Ben Daglish | NostalgicPlayer Ben Daglish | Benn Daglish (Eagleplayer V1.0), which patches the module's own replayer and runs it, with an audio interrupt handler of its own | 50 of 55 modules identical for 20 s (every replayer version of the list: the two oldest, the "C0" and "F0" ones, Motörhead's; 1 more file of the list is a ProTracker module, 9 are not on disk), and 21 of 22 later subsongs of 22 modules. `Butcher Hill ingame_1` is the same write for write, but an audio interrupt comes 20022 colour clocks after a tick's last write in the reference and 19924 in the port, either side of `replay-cmp.py`'s tick gap; in the other 4 (Motörhead's 3, `FoFT ingame_1`) two voices' interrupts a few hundred colour clocks apart are taken by one call of the handler in one and by two in the other. Writes spaced by the original's code-path costs fitted to its logs (median error 7 colour clocks); renders of 12 modules within a median 1.35 dB (specdist) |
| Hippel COSO | NostalgicPlayer Hippel (its COSO modules) | Hippel-COSO (Eagleplayer V2.12, with a replayer of its own) | 106 of 133 modules identical for 20 s; 26 more play notes past the period table, where the original reads past its code hunk (in deliplayer, the next Eagleplayer loaded) and the port writes period 0: with the words of the Hippel Eagleplayer's hunk there, 24 of them are identical and 2 differ only in such periods; `Amberstar - Intro (COSO)` makes the reference crash in its second tick (an address error in the fourth voice's frequency table, which the port stops). Non-first subsongs of 4 modules identical but for those periods; 6 Amberstar modules with the shared `hipc.samp`, played from their own folder, identical. Writes spaced by the original's code-path costs fitted to its logs, and InitSound timed as the reference's ExtLoad takes (by the path's length); renders of 10 modules within a median 0.33 dB (specdist), the Amberstar ones 0.34 to 2.17 dB, as their one-word loops play 65536 words of whatever memory follows the samples |
| Hippel TFMX | NostalgicPlayer Hippel (its TFMX modules), the replayer at the module's start | Hippel (Eagleplayer, which calls the module's own replayer) | One version of the replayer, the one with the song data at $A94 (Wings of Death, Leaving Teramis and others, told apart by a hash of its code): 22 of 22 modules identical for 20 s, subsongs too (the replayer steps through its 6-byte subsong entries by 4 bytes, so later subsongs read across two entries, as the port does); 62 modules with other versions of the replayer the port refuses. Fields InitSound leaves alone come from the module's saved voice variables, so modules saved from a game's memory start as in the original. Writes spaced by the original's code-path costs fitted to its logs, its line waits by a model of its VHPOSR polling; renders of 10 modules within a median 0.27 dB (specdist) |
| David Whittaker | NostalgicPlayer David Whittaker | DavidWhittaker (Eagleplayer V4.13, which runs the module's own replayer with its Paula writes redirected) | 172 of the 178 modules the original plays identical for 20 s (9 more it refuses, 54 of the list are missing), and later subsongs of 6 modules; the port reads each game's replayer variant from its code, keeps its variables in the module and finds the routines and subsongs with the original's own scans. Not ported: the old replayer of `Q-Ball` and the $1e-byte voices of `Leviathan` (refused); an effect that calls the game's sound effects (`Super League Manager` twice, `Emlyn Hughes Int Soccer`: identical for 121 ticks); `Jaws`, where the original's relocation reaches only part of the code, so the replayer writes Paula itself. Writes spaced by the original's flush costs and fitted play-routine times; renders of 13 modules within a median 1.64 dB (specdist) |
| Actionamics | NostalgicPlayer Actionamics | Action Amics (Eagleplayer V1.1, with a replayer of its own) | Only 2 modules known (`AST.bomber.all`, Dyna Blaster; the 2 `MOD.ast` of the list are other formats, which the original refuses too): all 32 subsongs of both (the original's 0-31, empty ones too) identical to the song end, for 20 s and for 90 s. It plays from the module in guest memory with its variables in the original's layout; LOC/LEN/PER every tick, DMACON on every tick and off on every row; writes spaced by code-path costs fitted to its logs (within a median 2 colour clocks of the reference's, at most 30); renders of the 66 runs within a median 0.05 dB (specdist, at most 0.22). The sample effect, the sample number, arpeggio and frequency lists, portamento and the arpeggio and tremolo effects are not used by these modules, so they are unchecked |
| SoundControl 3.x | NostalgicPlayer SoundControl (its 3.x player) | SoundControl (Eagleplayer V2.1, "SoundControl player module V1.01", adapted by Turbo/Infect & DEFECT; the PlayerStore DeliPlayer "Soundcontrol 3.0-5.0" is another player), with one replayer for 3.x and one for 4.0/5.0 | 8 of 8 modules identical for 120 s (3 more files of the list are ProTracker modules, which the original refuses); the original's 3.x replayer is NostalgicPlayer's 3.2 for every module (the sixth track's transposes, applied less one) with its own speed (ProcessCounter twice every fourth tick); no subsongs, as the original has none. Its variables live in guest memory in the original's hunk layout; writes and start spaced by code-path costs fitted to its logs (rms 7.8 colour clocks over the 20 modules), no DMA off to on gap differs; renders of the 8 within a median 0.04 dB (specdist) |
| SoundControl 4.0 | NostalgicPlayer SoundControl (its 4.0/5.0 player; 4.0 for the 3 Hot Numbers Deluxe modules it knows by length) | SoundControl (as for 3.x), which plays 4.0 as 5.0 | 3 of 3 modules identical for 120 s: from position 0 at the module's speed, without NostalgicPlayer's subsongs and speeds for them; the play sample command's DMA bits come from whatever the original's d4 holds (twice the note after a switch sample command), as the original writes them; renders within a median 0.28 dB (specdist) |
| SoundControl 5.0 | NostalgicPlayer SoundControl (its 4.0/5.0 player) | SoundControl (as for 3.x) | 9 of 9 modules identical for 120 s, as for 4.0; renders within a median 0.29 dB (specdist) |
| Digital Mugician 1 | NostalgicPlayer Digital Mugician, following the original's replayer | Digital Mugician (Eagleplayer, "Digital Mugician player module V1.3", adapted by DEFECT) | 94 of 94 4-voice modules identical for 20 s (25 more of the list are missing, 37 are copies of others), and later subsongs of 2 modules (up to the seventh); where NostalgicPlayer differs from the original (the note's DMA order, the no-DMA and no-wander effects, the morphing effect's second source, the finetune and volume-loop bits, two periods of the table, no range checks) the port follows the original, and plays from the module in guest memory as it does. Writes spaced by the original's code-path costs fitted to its logs (rms 0.19 colour clocks; within 8 colour clocks of the reference's over 20 s); renders of 10 modules within a median 0.03 dB (specdist) |
| Digital Mugician 2 | NostalgicPlayer Digital Mugician (its 7-voice modules), following the original's replayer | Digital Mugician (the same Eagleplayer), which does no mixing: it plays the four tracks of the subsong's own slot with its 4-voice replayer | 8 of 8 7-voice modules identical for 20 s, and later subsongs of 2 modules; NostalgicPlayer's mixing of the next slot onto the fourth voice is not ported, as the original has none. Writes timed as for Digital Mugician 1; renders of 8 modules within a median 0.02 dB (specdist) |
| Art of Noise 4V | NostalgicPlayer Art of Noise (its `AON4` modules) | Art of Noise (Eagleplayer, "Art of Noise player module V1.01", 13-Dec-96) | 98 of 98 modules the original plays identical for 20 s (`replay-batch.py --tick-gap 2000`: its timer runs at three times the tempo's rate, ticks 23650 colour clocks apart at tempo 125); the original crashes on 31 more (and 29 `AON8` ones) whose empty `AUTH` chunk it copies as 65536 bytes, which the port plays; 9 more files are packed (`S404`, `SFCD`) and 32 of the list are missing. Its tables and variables in guest memory in the original's layout, so reads past the tables come out the same; the original's first period, 3424 (NostalgicPlayer: 3434). Writes spaced by fitted code-path costs (rms 6.0 colour clocks over 818163 writes on 33 `AON4` and 12 `AON8` modules), the start by the time of the original's chunk searches and text copies (rms 2); renders of 12 modules within a median 1.11 dB (specdist) |
| Art of Noise 8V | NostalgicPlayer Art of Noise (its `AON8` modules) | Art of Noise (the same Eagleplayer, which plays both with one code) | 39 of 39 modules the original plays identical for 20 s (`--tick-gap 2000`); it crashes on 29 more, as for 4V. The original asks for the Eagleplayer amplifier (8 AudioStructs, `EPAMB_Direct`) and writes all 8 voices through `ENPP_Poke*` and `ENPP_DMAMask`; the port pokes the host's amplifier the same way (`Hw::voice_*`), and the 39 compare on all 8 virtual voices. Writes spaced by the same costs, first writes within 3 colour clocks; renders of 12 modules within 0.1 dB of the reference's RMS per side (the 8-bit mix's quantisation and the buffers' phase make their short-term spectra differ in the quiet bins) |
| Ron Klaren | NostalgicPlayer Ron Klaren, working on the module's own variables in guest memory | RonKlaren (Eagleplayer V1.13, which patches the module's own replayer and runs it) | 15 of the 15 modules the original plays identical for 20 s, at the module's own tick rate (its CIA timer value, 496 to 748 ticks in 10 s), every subsong of the multi-subsong ones too (19 for 30 s); 5 more of the list (`Other_RK_Versions`, no hunk header) the original refuses, and so does the port; the writes come at the original's times, its start, its interrupt's path and the costs between writes counted in its cycles: 96.7% of 196k writes at the same colour clock, 98.8% within 2; renders of 15 modules within a median 0.00 dB (specdist) of the reference, the worst mean 0.07 dB |
| Digital Sound Studio | NostalgicPlayer Digital Sound Studio, following the original's much smaller replayer | DSS (Eagleplayer, "Digital Sound Studio player module V1.71", adapted by Buggs/Defect; the PlayerStore DeliPlayer DSS has the same name, so run with `DELIPLAYERS` pointing elsewhere), which knows effects 0 to 6 only | 32 of 32 modules identical for 20 s and for 180 s (11 more of the list are copies); no subsongs, as the original has none. The original reads the speed as a word with the tempo byte before it, so the 11 modules with a tempo play their first row after 11 minutes or more, silent for 20 s in both (`Africa` checked to its first row at 700 s). It plays from the module in guest memory as the original does (InitSound clears each sample's first long, set speed writes the module); the arpeggio reads on past the original's period table in a copy of its hunk. Writes spaced by fitted code-path costs (rms 0.16 colour clocks), the timer's start by the start's loops over positions and samples: 99.7% of 567k writes within 2 colour clocks of the reference's; renders of the 21 modules with writes within a median 0.01 dB (specdist), the worst mean 0.51 dB |
| Music Assembler | NostalgicPlayer Music Assembler, working on the module's own variables in guest memory | Music Assembler (Eagleplayer V2.11, adapted by DEFECT, with its own copy of the replayer, which plays on the module's tables at fixed offsets) | 8 of the 8 modules the original plays identical for 20 s and for 180 s, every write at the reference's colour clock, and subsongs 1-9 of all 8 (which the original plays as subsong 0: its InitSound overwrites the subsong number with the module's data offset); the other 2 files of the list are a ProTracker module and an XM, which the original refuses too, 5 are missing; 210 variants of 5 of them with random instruments and voices' saved state (110 with random track bytes too) are identical for 15 s; renders of the 8 within a median 0.00 dB (specdist; byte-identical) |
| Sound Factory | NostalgicPlayer Sound Factory, following the original's replayer, on its variables and the module in guest memory | Profiteam-Soundfactory (Eagleplayer, "Profiteam-Soundfactory Player Module V1.0", adapted by marley/INFECT; the PlayerStore DeliPlayer "Soundfactory" is a changed replayer of its own) | 8 of the 8 modules the original plays identical for 20 s and for 90 s (9 files of the list, two the same Danger Castle; 2 more are missing), and the 4 later subsongs of Danger Castle, with every write on the reference's clock: the port counts the original's instruction cycles on each code path, and its renders are bit-identical to the reference's (specdist 0.00 dB on 12 renders). The original crashes on 3 more (`Im Maien`, `Mystic`, `Oriental`), which use opcode `$9C` past its jump table; the port transposes there, as NostalgicPlayer and the DeliPlayer do |
| InStereo! 1.0 | NostalgicPlayer InStereo! 1.0, following the original's replayer | In Stereo 1.0 (Eagleplayer "Sound Invasion Music System player module V2.11", 13 Dec 96) | 3 of 3 modules identical for 20 s and to their ends (the list's other 14 paths are other formats or missing), and a second subsong added to one; where NostalgicPlayer differs from the original (the row's write and DMA order, the envelope generator counter's buffer and last step, the vibrato test, the ADSR volume, the speed effect's copy, the fourth voice's vibrato delay left in the player's file) the port follows the original, with its variables, tables, voices and synth buffers in guest memory in the original's layout. The writes come at the original's times, its start and its code's paths counted in its cycles: 100% of 450187 writes at the same colour clock; renders of 3 modules within a median 0.00 dB (specdist), the worst frame 0.34 dB (`Fantasi 8`) |
| InStereo! 2.0 | NostalgicPlayer InStereo! 2.0, following the original's replayer | In Stereo 2.0 (Eagleplayer "InStereo 2.0 Amplifier module V2.11", 13 Dec 96, which writes through `ENPP_Poke*`) | 6 of 6 modules identical for 20 s and to their ends, and subsong 1 of the 2 with two; where NostalgicPlayer differs from the original the port follows it: a sample's loop goes out the tick after its note, from address 0 (the original passes a register it never set) or from an empty buffer of its own for 1 word; sample number 0 has no data; the per-voice DMA off clears other voices' bytes; a synth's portamento speed and its effect-7 volume come from the bytes the original reads; the tempo sets the timer (`tempo * timer / 50`). Its variables, tables and voices in guest memory in the original's layout; the writes at the original's times, counted in its cycles: 100% of 691576 writes at the same colour clock; renders of 6 modules within a median 0.00 dB (specdist), but `Firefox II`'s loops play 6.9 KB from address 0, which holds other data in the two runs (mean 0.04 dB, worst frame 12 dB) |
| Synthesis | NostalgicPlayer Synthesis, following the original's replayer | Synth 4.0 (Eagleplayer, "Synthesis Replayer V4.0 player module V1.21", adapted by DEFECT), which plays `Synth4.x` files and `Synth.Pc` ones (a game's replayer with its module) | 43 of 43 modules the original plays identical for 20 s, 8 of them `Synth.Pc` files, and every later subsong of the 5 multi-subsong ones (19); 1 more file of the list is another format, which both refuse, and 19 are missing. Its variables, synth buffers and tables in guest memory in the layout of the original's data hunk, starting as zeros (not the leftovers the original's file holds); where NostalgicPlayer differs (the row effects 8, 9 and B, note `$7F`, the envelope generator's count and copy, the synth effects' loops, a track arpeggio's reads through the period table, no FM drum, no range checks) the port follows the original. Writes at the original's times, its paths' costs counted in 68000 cycles (the loops' from its code, the rest fitted): 97.3% of the gaps between writes exact, 88.5% of 377k writes at the same colour clock, 99.8% within 2 (one module's Alising effect reads past its note's copy into the buffer's leftovers, and takes other times there; a write one clock off before a DMA wait can come one line, 227 clocks, off); renders of 43 modules within a median 0.00 dB (specdist), means a median 0.05 dB, the worst 2.67 dB |
| MED | NostalgicPlayer OctaMED (its mixer's player on Paula's 4 channels: periods, volumes 0-64) and its MED 2.10 converter | MED (Eagleplayer, OctaMED's proplayer 7.0), which plays MMD0, MMD1 and MMD2 modules without the 5-8 channel and mix modes (the next rows' port plays those, and MMD3), and MED 2.10 files (`MED\x04`, converted to MMD0) | 877 of 885 files the original plays (918 of the list, the rest copies; it refuses 7 MED 2.10 files with 8 channels and one MMD1): 694 identical for 20 s, 166 with every write the same but grouped into ticks differently (the original's first two timer interrupts come early, see Status; at the fastest tempos two ticks are less than 20000 colour clocks apart), 13 MED 2.10 files with the same writes but the samples laid out apart in memory (each in an allocation of its own there, the synth sounds' waveforms too), 4 MED 2.10 files that differ (instruments the original does not play, its sample mask read otherwise); no DMA off to on gap differs. Later subsongs (the modules after the first, `MCNT` continuations as the first's format): all 20 of the 2nd and 11 of the 12 3rd match; in the one file of `MCNT` modules the original reads its instruments from nowhere. Where NostalgicPlayer differs the port follows the original: notes as ProTracker periods (synth sounds and types 7+ two octaves lower, from tables computed in equal-tempered steps from 4 times C-1, finetune -7 from 4 * 897), tremolo 07 (absent from NostalgicPlayer), the timer from the CIA tempos (BPM songs advance every 4th interrupt), the first module's instrument extensions for every song, MED 2.10 files keeping their multi-octave sample types, played at 6 ticks a row without a version chunk, and with every sample of their sample mask; writes spaced by fitted code-path costs; renders of 7 modules within a median 0.67 to 2.96 dB (specdist, at the lag of the original's later start) |
| ActivisionPro | NostalgicPlayer Activision Pro, working on the module's own replayer variables in guest memory | Activision Pro (Eagleplayer) | 38 of 38 modules identical for 20 s (47 files of the list, 9 of them copies), and the 3 subsongs after the first of all 38 (the Eagleplayer's range is the first to 32);the port reads each game's replayer variant from its code with templates (track forms, sample tables, effects, envelope, speed variation, sound effect hooks, transposes, volume gates, the later replayers that write after their delay, those reached through a jump table), and refuses code it does not know. A vibrato of depth 0 is a division by zero that crashes the original in later subsongs of the Thunderhawk modules; the port plays on. Writes spaced by code-path costs fitted to its logs (median error 19 to 86 colour clocks on 3 modules); in 4 of the 38 one DMA off to on gap of 102 to 148 differs; renders of 13 modules within a median 2.85 dB (specdist; 2.41 to 3.89 by module) |
| MusicMaker 4V | The original replayer, `MusicMaker4.asm` (Thomas Winischhofer, published by him under the BSD licence), translated | MusicMaker-4V as UADE runs it (`uade123`, built from that source; DeliTracker's MusicMaker4 Eagleplayer stays silent in deliplayer) | 1 of 1 module identical for 20 s (`MM8.Buergermeister`, the only STD song of the 19 MusicMaker files known; 1011 ticks), compared with UADE's write log tick by tick. Only the single-file form (`FORM` `MMV8`, with `INST` or packed `PINS`) is read: no `.sdata` with `.i`/`.ip` pair is known. The row's DMA off and on are two interrupts of the replayer's own CIA-B timer, $170 E-clocks apart, done in one tick with that wait; writes timed as UADE's run (a 68020, not the original's 68000): within a median 2 colour clocks of it, ticks within 660. Renders against UADE's: specdist median 6.2 dB, as for a port with identical writes (JamCracker) against UADE's renderer |
| MusicMaker 8V | The original replayer, `MusicMaker8.asm` (Thomas Winischhofer, published by him under the BSD licence), translated, with its software mixer (8 voices in pairs onto Paula's 4 channels, double buffers, the dbra mixing routines a 68020 takes) | MusicMaker-8V as UADE runs it (`uade123`, built from that source) | The 18 EXT songs known: register writes the same for 74% of the interrupts over 20 s (9752 of 13145, the channels' order within one aside), the mixed buffers byte for byte the same up to the first difference; with UADE's polling passes fed in, all 18 have every interrupt's writes and every buffer the same. The rest come from when each channel's interrupt is raised: UADE's Paula restarts a buffer at a raster line's DMA slot, deliplayer's at the period's end, so the replayer's wait for all 4 channels takes them in other passes, and its per-channel correction (`calccorr`: a byte or more for a late one) gives a buffer a word more or less from there. Writes timed as UADE's run: a median 0 to 4 colour clocks from it. Renders against UADE's: specdist median 6.1 to 6.7 dB on 10 songs, as for a port with identical writes (JamCracker) against UADE's renderer. The 68000 mixing routines (self-modifying, for `mixertype` other than $0f before a 68020) are not translated; no known song uses them |
| AMOS Music Bank | NostalgicPlayer AMOS Music Bank, following the original's replayer | AmosMusicBank (the PlayerStore DeliPlayer, "AmosMusicBank V1.5 player module V0.8", Francois Lionet's replayer adapted by Delirium; UADE's "Amos Music Bank" Eagleplayer converts the bank to a ProTracker module and plays that, so it is no reference) | 78 of 78 banks identical for 20 s and for 180 s (1 more of the list is missing), and 88 of NostalgicPlayer's 89 for 180 s too (its `Technology-Rock` has $C0, not the chip flag $80, in its length's top byte, which both refuse); every bank has one song, so subsongs (from 1) were checked on 8 banks given a second song: identical. Every write at the reference's colour clock: the port counts the original's cycles on each code path, the host's times around them measured; renders bit-identical (specdist 0.00 dB). Where NostalgicPlayer differs (the note's sample, length, DMA and loop on the next two ticks, no volume on a note, the arpeggio's low byte, the vibrato's unsigned position, the slide at 856, the position jump read in the same step, pattern and instrument numbers unchecked, no song end on a loop, the song end when no voice counts, the `$C0` memory form) the port follows the original. 11 synthetic banks (repeats, the song end, the memory form, random tracks) identical for 60 s but for an arpeggio past the period table, where the original reads on into the next DeliPlayer loaded and the port reads zeros |
| Game Music Creator | NostalgicPlayer Game Music Creator, following the original's replayer | G.M.C (UADE's Eagleplayer, "Game Music Creator player module V1.6", Andreas Tadic's replayer adapted by Delirium and Buggs; run with `--eagleplayers` pointing at UADE's players) | 90 of 90 modules identical for 20 s and for 180 s (115 files of the list, 25 of them copies), and 7 more of NostalgicPlayer's test set; every write at the reference's colour clock and the renders byte-identical. Where NostalgicPlayer differs from the original the port follows it: a row writes a note's volume 0, address, length and period and then its DMA off, the next tick DMA on and the tick after its loop, always the stored loop (a 1-word loop plays the player's 8 bytes of silence); effect 3 is not capped at 64, effects 6 and 7 do nothing; position words of `$8000` or more play; effect 5 counts the position one lower than the one it reads and leaves the row pointer on the position table for the voices after it. One module of the list has an empty first pattern, which NostalgicPlayer refuses and the port takes |
| AHX | 8bitbubsy's ahx2play (BSD 3-Clause), changed to follow the binary replayer the Eagleplayer runs, not the tracker's | THX ("THX Soundsystem Eagleplayer Replayer module V2.1", Martin Wodok and Buggs of DEFECT, running THX-Sound_System_BinaryPlayer_2.00; run with `DELIPLAYERS` and `DELIGENIES` pointing to an empty directory) | 482 of 483 modules identical for 20 s (529 files of the list, the rest copies; `legoz`, at a very high tempo, needs `--tick-gap 2000` to compare), and the subsongs after the first of `simple e` and `Moe doet de Koe` (the Eagleplayer's range is 0 to the module's count); the waveforms it builds (410,760 bytes) and the voices' chip buffers are byte for byte the original's. In `FISH` 3 ticks differ: a note below 0 makes the original read its own code before the period table, which the port does not copy (it uses period 3424). `Rubber_Spine` has an instrument of 0 decay frames, a division by zero that crashes the original after 302 ticks; the port plays on. Subsongs that start past the song's end read memory past the module: `Moe doet de Koe` subsong 3 differs from tick 819 in periods only, all at volume 0. Writes spaced by code-path costs fitted to its logs, and the start by the Eagleplayer's 68000 cycles: the module-dependent part of its InitPlayer (a song-length scan of the module, loops over the instruments and their names) is counted per code path, so InitSound's first write falls on the reference's colour clock for all 483 modules and the Paula line phase the rest follows is the reference's. 91% of the writes of 47 modules are on the reference's clock and all within 2 colour clocks (the rest are 1 off, in runs of ticks from the second); renders of the first 10 50 Hz modules are 0.00 dB (specdist, median, every module; 2.65, 0.28 to 4.19, before), 47 modules 0.00 median, the worst 1.32 |
| PreTracker | Raspberry Casket (Chris Hodges) and rePlayer's C player (Daniel Collin), both MIT (`LICENSE-PreTracker`; released with permission from Pink / Abyss) | UADE's PreTracker player (Pink's V1.5 replay; it has no check, so it needs one that accepts the module) | 109 of 109 modules identical for 20 s, 10 of them for 180 s, and 6 subsongs of 2 V1.5 modules (numbered 0 to n-1 as the reference does); the port signals a song end where the reference plays on. The waves are generated at init as Raspberry Casket does, in the original's 8-bit integer arithmetic (the C player uses floating point): 108 of 109 modules byte-identical in chip memory; in `prt.m065` 265 of 215762 bytes differ, a wave at base note 117 whose phase step passes its oscillator table, where the original reads its own memory and the port 0. Where the two sources and the reference differ, the port follows the reference: first row 15 ticks, ping-pong forwards, 32-bit chord phase, V1.0 subloop wait, word vibrato delay. Writes spaced by the original's work per tick fitted to its logs over 37 modules (rms 309 colour clocks); the wave generation takes no time, so the song starts mid-frame; renders of 10 modules within a median 2.5 dB (specdist; 0.56 to 4.13 by module; the format is very sensitive to the write timing) |
| Musicline Editor | NostalgicPlayer MusiclineEditor (MIT, Thomas Neumann; `LICENSE-NostalgicPlayer`), 4 and 8 voices; the tables are NostalgicPlayer's | MusiclineEditor (Eagleplayer; UADE's score for the mixing) | 4 voices: 162 of 192 modules identical for 20 s against UADE's write log; on the other 30 every tick both play is identical but the port plays fewer of them in 20 s (547 to 1880 against 951 to 2021), (its tick durations follow the Eagleplayer on a 68000, UADE's 68020 runs at maximum speed; the build before the first-tick and tick-duration commits matched 189 of 192); against the Eagleplayer's own write log (empty player and genie directories, so the heap layout is the same; the port's data hunk is at the top of memory like the original's and differs by one constant) 74 of 192 are identical and 53 more have all but 3 ticks equal (the last tick cut by the 20 s window), the other 65 differ because tunes that overrun a 68000 tick take longer per tick in the Eagleplayer than the port's cost model gives (5 to 20 percent), so their ticks drift apart; no sample address differs any more except where the two runs played a different number of ticks. 8 voices (mixed in software, two voices to a Paula channel, from the channel-0 audio interrupt): the mix buffers of 112 of 113 modules are byte-identical to UADE's; the last one differs where UADE itself plays a buffer twice (a late interrupt). The 4-voice timer starts before its latch is written, so it counts down from $FFFF and the first tick comes 65536 E-clocks after the start, regular from there as in the Eagleplayer; each tick takes the time the original's takes before its first write (fitted per row and per effect byte). The 8-voice mode is too heavy for a 68000: the original Eagleplayer's audio interrupt overruns its buffer on a 68000-timed machine, so in deliplayer's engine it swaps buffers only every second interrupt (roughly 283 swaps in 10 s, against 627 when the CPU keeps up). The port plays as on a CPU that keeps up, as UADE (68020, max speed) does. Its PAL buffers are byte-identical to a PAL UADE run on 112 of 113 modules; UADE itself runs the player as NTSC, because its score leaves dtg_GfxBase unset. The port follows the original where NostalgicPlayer differs (acknowledged interrupts, restart waits, word-multiply volumes, no range checks, no song end) |
| HivelyTracker | NostalgicPlayer HivelyTracker (HVL; AHX files stay with the AHX port), following Pete Gordon's hvl_replay where they differ (the position after the last is 0, a song ends by the original's flags); played on the Eagleplayer amplifier, two virtual voices (left, right) a channel for the pan | no Amiga player plays HVL (the THX Eagleplayer refuses it), so the original C replayer, hvl2wav (replayer 1.9), built with 32-bit types, dumped tick by tick (volume, period, pan and a hash of the 640-byte waveform loop of every voice) | 52 of 52 distinct modules (4 to 16 channels, speed multipliers 1 to 3, none with subsongs) identical to the song's end, with the reference run on the port's waveform tables: the reference's own differ from AHX's (its noise has $7F where AHX has $80, its filters are floating point where AHX and NostalgicPlayer settle integer filters; its plain triangles, sawtooths and squares are the port's byte for byte, its filtered ones differ by 1 to 12 steps on average). Renders against hvl2wav (20 s, 100 Hz to 2 kHz, level matched, 52 modules): mean 5.96 dB (3.45 to 12.10 by module), spectral correlation 0.77; the amplifier's output is 14.6 dB quieter (its own boost; the song's mix gain is not applied) and Paula's filters roll off what is above 2 kHz. Ring modulation is approximated: the modulating waveform is multiplied into the loop at the ratio of the periods, from the loop's start |
| 15-instrument SoundTrackers (Ultimate SoundTracker 1.0 and 1.8, SoundTracker II, VI and IX, Master SoundTracker, SoundTracker 2.x) | NostalgicPlayer's ModTracker player (MIT, Thomas Neumann; `LICENSE-NostalgicPlayer`): its identification of the variant and its playback; the modules have no tag, and the 31-instrument formats stay with the built-in ProTracker | UADE's Ultimate Soundtracker player for the two Ultimate SoundTrackers, its Soundtracker IV player for the rest | Against the references' write logs for 20 s, 100 archive modules a variant: Ultimate SoundTracker 1.0 100 identical, 1.8 100, SoundTracker II 98, VI 95, IX 5, Master SoundTracker 58, SoundTracker 2.x 33. The rest differ only where the reference lacks an effect (E, the filter; D and B in 2.x, where the reference's D is a volume slide; 3 to 8 in the later ones), so on 500 copies with those effects removed (effects 0, 1, 2, C and F kept): 1 of 1, 3 of 3, 121 of 123, 203 of 204, 154 of 159, and SoundTracker IX 2 of 10 with every tick the same but the count (the reference ignores the tempo byte that NostalgicPlayer, and so the port, takes as a CIA timer). What stays: an arpeggio on a period that is not in the table (the original reads on past its table into its own code and data, the port writes 0; 3 modules); a first pattern of 32 or more (the original's offset is a signed word and reads before the module; 5 modules); and a module the reference breaks off early. The port follows the references where NostalgicPlayer differs (a loop length of 0 words is a loop in the D.O.C. players; any sample puts a voice on the restore list; F00 stops the song; Ultimate SoundTracker's bend starts from the period of its own row). Renders of 10 modules within a median 0.1 to 0.4 dB (specdist; worst frame 4.6 dB) |
| Multichannel MODs (`Multichannel MOD`: `6CHN`, `8CHN`, `xxCH`, `CD61`, `CD81`, `OCTA`, `FA06`, `FA08`, and `xCHN` but `4CHN`; `StarTrekker 8`: `FLT8`) | NostalgicPlayer's ModTracker player (MIT, Thomas Neumann; `LICENSE-NostalgicPlayer`), its ProTracker and StarTrekker 8 playback generalised from four channels to up to 32. Its identifier knows only `FLT8`; the other tags have no source of their own (same layout, more cells to a row), no code from elsewhere. Four-channel tags stay with the built-in ProTracker. Played on the Eagleplayer amplifier, one voice a channel (Amiga LRRL sides). Not played: StarTrekker AM instruments, E8x (Karplus-Strong). Fixed where the source is wrong: period slides wrap past their limit, a tremolo's volume outlives it, FLT8's vibrato has no depth | the PS3M DeliPlayer (software mixer; no per-tick state to compare), and openmpt123 as a second opinion | 420 of 420 corpus modules (402 FST.* and others, 2 to 32 channels; 21 `4CHN` stay with ProTracker) play under their own format name with the player directories empty; none silent but one that is silent in PS3M and openmpt too. 40 spread over the tags, 20 s: level within 3 percent of PS3M (median); against PS3M mean 10.9 dB (specdist, aligned by cross-correlation, usually within 4 ms), spectral correlation 0.77, as close as openmpt123 is to PS3M (10.8 dB, 0.68); against openmpt123 14.0 dB, 0.77. Not bit-exact: PS3M's mixing rate and interpolation differ from the amplifier's. FLT8, OCTA, CD61, FA06 have no module in the corpus (FLT8 only a synthetic test) |
| Quadra Composer (IFF EMOD) | NostalgicPlayer QuadraComposer (MIT, Thomas Neumann; `LICENSE-NostalgicPlayer`; the tables are its own) | IFF EMOD (Eagleplayer) | 27 of 27 modules identical for 20 s (the archive's `emod.*`), 23 of 27 for 90 s: the other 4 (`OmenousChug`, `Enigma` and two more) differ in a few later vibrato periods. Where the original differs from NostalgicPlayer it is followed: the row's DMA goes off and on once for every voice a tick starts, with the loop registers of all four voices after it (0 and length 0 for a sample without a loop); a retrig or note delay starts its voice the same way; a sample of one word or less is written with length 0; the tempo's CIA timer is 1789773 / tempo; effect 6 writes the period without vibrato. Renders differ widely (specdist median 17 dB over 12 modules) though the writes are identical: after a sample without a loop the original's loop registers (address 0, length 0) make Paula play 128 KB from address 0, the engine's own memory, which differs between a 68k run and a port run |
| Face The Music | NostalgicPlayer FaceTheMusic (MIT, Thomas Neumann; `LICENSE-NostalgicPlayer`; the tables are its own), through the host's Eagleplayer amplifier (`Hw::voice_*`), one virtual voice a track, a track on its pair's side as NostalgicPlayer pans them | Face The Music (Eagleplayer) | 55 of 55 modules of the archive's `ftm.*` play under their own name with `DELIPLAYERS`, `EAGLEPLAYERS` and `DELIGENIES` empty (all have their samples inside; a module with external samples is refused); 0 identical to the Eagleplayer, by design: the Eagleplayer plays the 8 tracks on Paula's 4 channels with a scheme of its own (its logs show audio interrupts re-pointing the channels to buffers of 100 words and rewriting period and volume, which suggests it mixes pairs of tracks in software; not read from its code) that NostalgicPlayer, which has a mixer of its own, does not describe, so the write logs cannot be made to match; what it takes from the original is the module's CIA timer. Renders of 12 modules: specdist median 17.0 dB at the cross-correlation lag, 9.5 dB with the port's level raised to the reference's (the port is 9 to 13 dB quieter: the amplifier's volume stops at 64 and it divides by the voices of a side); the loudest note of a frame has the reference's pitch in 37 to 65 % of the frames of the two modules checked. No subsongs |
| DigiBooster (1.x) | NostalgicPlayer DigiBooster (MIT, Thomas Neumann; `LICENSE-NostalgicPlayer`), following the Eagleplayer's pokes where they differ (a sample plays up to its loop's end, the loop is set after DMA on, the fine tune stretches a period by 2^(-ft/96), the first row after 4 ticks). 8 channels poked on the amplifier's 8 virtual voices (L L R R R R L L), no mixing of its own. DigiBooster Pro (`DBM0`) is not played. Subsongs: none | Digibooster (Eagleplayer), run with the DeliPlayer directory empty | 238 distinct modules of the archive, all played under their own name with all three player directories empty. Register writes (the Eagleplayer's virtual voices, 20 s): **0 of 238 identical**; 142 128 of 247 678 ticks (57.4 percent) are identical, tick counts equal (999, or 1004 with BPM changes). Everything that goes to the voices' LOC, LEN and VOL matches in the cases examined; what differs is which ticks the 68k player writes a voice's period on and how many times (it writes it once or twice, by the row's effect, and every tick under some effects), which this port approximates from the rows seen (and the first row's, the loops after each row, and the E1x and E2x pairs are the Eagleplayer's). The robot effect (8xx) plays the sample plain and E3x (backward play) forwards. Renders against the Eagleplayer's: specdist median 5.8 dB (238 modules, xcorr lag, no render sample for sample identical), a measure the lag search and the different period writes both spoil; the audio was not checked beyond that |
| Oktalyzer | NostalgicPlayer Oktalyzer (MIT, Thomas Neumann; `LICENSE-NostalgicPlayer`), following the Eagleplayer where they differ (listed in the file's header). It does not mix, as the Eagleplayer does not: with no mixed channel it writes Paula's registers, with one it pokes the amplifier's voices for the single channels and fills its 8 AudioStructs for the mixed ones, and the host's mixer plays them. Subsongs: none | Oktalyzer (Eagleplayer), run with the DeliPlayer directory empty (the DeliPlayer `Oktalyzer` plays nothing here) | 450 distinct modules of the archive (CMOD of 4 single channels: 237; with mixed channels: 213), all played under their own name with the DeliPlayer, Eagleplayer and genie directories empty but one (truncated: the Eagleplayer refuses it too). Register writes, 20 s: 4 single channels 236 of 236 identical, the 237th refused by both; with mixed channels the single channels' pokes 213 of 213 identical (many have none). The mixed channels' AudioStructs as the mixer reads them at each buffer (trace of the amplifier), when the port is given the Eagleplayer's own start-up times (the time from the amplifier's start to its first writes, to the restart and to the first tick, which depend on the module): 196 of 213 modules equal in every one of their 970 buffers, 99.5 percent of 201 212 buffers, and 194 of 213 renders sample for sample identical to the Eagleplayer's (specdist 0.000 median, 6.1 worst). Without them (the port's default, a fit of those times to the number of pattern cells, rms 350 000 colour clocks) the phase between the ticks and the buffers is not the original's, so a note may reach the mixer a buffer early or late: renders against the Eagleplayer's, 213 modules, specdist median 3.7 dB (xcorr lag; 4 renders with a wrong lag give 10 to 36); 4 single channels, median 0.8 dB. The Eagleplayer's InitPlayer time, which sets that phase, is not reproduced (it goes through the patterns; its cost is not a function of the module that fitted). A mixed voice whose note an effect moves above the table plays a period read past the original's table (not reproduced: it stops). No regression: 143 modules of the other 48 corpus lists play unchanged |
| OctaMED (mix mode, 5-8 channels) | NostalgicPlayer OctaMED (MIT, Thomas Neumann; `LICENSE-NostalgicPlayer`; it imports no code of its `Ports` directory): its loader (MMD0 to MMD3, the old songs' conversion to mix mode, the 5-8 channel songs' tempo conversion), player tick (rows, hold and decay, the effect commands, synth and hybrid sounds) and frequency table (`src/replay/med/mix.rs`), on the host's amplifier (`Hw::voice_*`): one voice a track, Amiga sides LRRL, at most 32 voices. A note is the voice's DMA off, the sample's start and length, the period for its frequency, the volume, DMA on and the loop; the amplifier got a LEN in bytes and a LOC at any byte (`Poke::LenBytes`, `LocByte`: a software mixer's samples are longer than Paula's 65535 words and loop at odd samples), and its mix rate is set to 31 kHz, the highest the emulated Paula plays. What the amplifier has no counterpart of: 16-bit and stereo samples become 8-bit mono when the module is loaded (the high byte, the mean of the two channels), in the module's own memory where no other sample lies; a ping-pong loop is unrolled into a loop of twice the length; a synth or hybrid sound's waveform is the voice's next block, taking effect where the block ends as on Paula; the echo (the song's or its one effect group's, normal or crossed) is a copy of each track's pokes a delay later (to the nearest tick) on a voice of the same side (crossed: the other) at the level of one repeat, 2 to the minus depth (the echo of the echo is not played, nor are several effect groups, and a song with more than 16 tracks has none); not played: stereo separation, the track pans (a voice keeps its Amiga side), "sample backwards" (command 20), MIDI instruments, the filter sweeps, the volume adjust, and the original's own mix rate and level (the amplifier's: every voice of a side may sound at full volume). Timing and pitch as UADE's players have them on a PAL Amiga: a tick of 3579545 / Hz colour clocks and periods from 3579545 / frequency (0.9% low), the older songs' ticks another 0.45% longer (fitted) | Octa-MED (the 68k player that UADE (`uade123`) takes for MMD0-MMD2 songs; the Eagleplayer OctaMED refuses these songs), mixing in software; its renders only, no register writes to compare | 335 distinct modules of the archive (MMD0-MMD2 with the 5-8 channel flag or mix mode, `MED.*`), all played under their own name with the DeliPlayer, Eagleplayer and genie directories empty. Renders of 20 s against UADE's (specdist at the xcorr lag, level matched since the amplifier's scale is its own; two renderings of the same notes through the Paula path and through this one give a mean of 6.3 dB, 4.7 on the bins that carry energy): 334 compared, mean 6.36 dB median (p25 5.78, p75 7.26, p90 8.94; 279 under 8 dB, 8 over 12), spectral correlation 0.78 median; the older songs' tempo drifted 0.02 s in 4 s from UADE's before the fitted 0.45% (4 songs checked after: at most 0.005 s in 4 s, 0.01 s for the one 8-channel song of tempo 8). Not compared note by note: the original's mixer is software. 8-channel songs of tempo 1 to 9 (NostalgicPlayer's conversion table) still drift 0.15%. No regression: 1081 of the 1194 distinct modules of ports/med.txt (the 4-channel MMD0-MMD2 and the MED 2.10 files) and 145 distinct modules (3 of each other corpus list) render WAVs, 10 s each, identical to the previous build's; the 113 that differ are the ones it refused before (25 MED, 88 OSS) |
| OctaMED SoundStudio | NostalgicPlayer OctaMED (MIT, Thomas Neumann; `LICENSE-NostalgicPlayer`; it imports no code of its `Ports` directory): its loader (MMD0 to MMD3, the old songs' conversion to mix mode, the 5-8 channel songs' tempo conversion), player tick (rows, hold and decay, the effect commands, synth and hybrid sounds) and frequency table (`src/replay/med/mix.rs`), on the host's amplifier (`Hw::voice_*`): one voice a track, Amiga sides LRRL, at most 32 voices. A note is the voice's DMA off, the sample's start and length, the period for its frequency, the volume, DMA on and the loop; the amplifier got a LEN in bytes and a LOC at any byte (`Poke::LenBytes`, `LocByte`: a software mixer's samples are longer than Paula's 65535 words and loop at odd samples), and its mix rate is set to 31 kHz, the highest the emulated Paula plays. What the amplifier has no counterpart of: 16-bit and stereo samples become 8-bit mono when the module is loaded (the high byte, the mean of the two channels), in the module's own memory where no other sample lies; a ping-pong loop is unrolled into a loop of twice the length; a synth or hybrid sound's waveform is the voice's next block, taking effect where the block ends as on Paula; the echo (the song's or its one effect group's, normal or crossed) is a copy of each track's pokes a delay later (to the nearest tick) on a voice of the same side (crossed: the other) at the level of one repeat, 2 to the minus depth (the echo of the echo is not played, nor are several effect groups, and a song with more than 16 tracks has none); not played: stereo separation, the track pans (a voice keeps its Amiga side), "sample backwards" (command 20), MIDI instruments, the filter sweeps, the volume adjust, and the original's own mix rate and level (the amplifier's: every voice of a side may sound at full volume). Timing and pitch as UADE's players have them on a PAL Amiga: a tick of 3579545 / Hz colour clocks and periods from 3579545 / frequency (0.9% low), the older songs' ticks another 0.45% longer (fitted) | OctaMed Soundstudio (the 68k player UADE (`uade123`) takes for MMD3 songs; the Eagleplayer OctaMED refuses mix mode, and in the shipping configuration TME claims these files and plays silence) | 300 of the 887 distinct `OSS.*` modules of the archive (all `MMD3`), all played under their own name with the DeliPlayer, Eagleplayer and genie directories empty; 298 sound (one has no blocks and plays silence under its name; one has a drone of single steps of the 8-bit stage, as its reference is faint too). Renders of 20 s against UADE's (its player starts mixing about 0.85 s late, which the xcorr lag removes; its tempo is within 0.01 s over 12 s): 148 without echo, 147 compared: mean 6.36 dB median (p25 5.92, p75 7.50, p90 9.82; 113 under 8 dB, 7 over 12), correlation 0.80; 152 with echo, 151 compared: 6.64 dB median (p90 15.48; 23 over 12; in an earlier run with the echo's voices left out 7.38, p90 18.6, and the echo helped 125 of the 152), correlation 0.78. The modules over 12 dB were not examined one by one; echoes of the echo, of several effect groups and of songs of more than 16 tracks, and stereo separation, are not played. No regression: as the row above |

Not implemented:

* of the Eagleplayer amplifier, all but the 8 Bit Amplifier's default mixing:
  16-bit samples, its delay, surround and mono effects, its prefs, and the 14
  Bit and AHI amplifier engines (the 68k engines themselves do not run)
* most `ENPP_*` functions besides the DeliTracker ones, the amplifier pokes,
  `LoadExecutable` and `AllocSampleStruct` (the rest log once and return)
* the windows of GUI genies (scopes, analyzers, ModuleInfo): they load and run,
  but intuition and graphics are stubs, so they show nothing
* `ModSave-NotePlayer`, which saves the notes as a module file, fails in its
  process startup
* XPK decrunching
* the DataType player

## ProWizard

`src/prowizard/` is a Rust port of NostalgicPlayer's ProWizard converter (MIT,
Thomas Neumann), itself after Asle's ProWizard and libxmp: 71 packers and
protectors of ProTracker modules (The Player 2.2A-6.1A, NoisePacker 1-3,
ProRunner 1/2, Promizer, ProPacker, Tracker Packer, Unic Tracker, Pha Packer,
Channel Player, Images Music System, ...). After decrunching, the engine hands
every module to `prowizard::convert`; the first format whose check passes turns
it into a ProTracker module in memory, which the built-in ProTracker then plays
(`--player` skips it). The checks run in NostalgicPlayer's order, also on
plain modules, as Devils Replay and HCD Protector keep the `M.K.` mark.

* Detection, over NostalgicPlayer's test files of all 214 formats and the
  archive lists: 1554 of the 1555 test files of the 70 formats that have any
  are taken by their own format (the other, a Tracker Packer 3 module, is
  XPK-crunched); no file of any other format, plain MODs included, is taken.
* Conversion, against libxmp's (`xmp`, for comparison only): most modules give
  the same patterns and samples. The differences are NostalgicPlayer's choices,
  kept (The Player 4.x's E02, B and D parameters, Images Music System's notes,
  NoisePacker's B parameter, ...); two of its bugs are fixed: Fuchs Tracker
  skips the `INST` mark, NovoTrade Packer's rows end on bit 15.
* Playback, against UADE's PTK-Prowiz Eagleplayer in this engine, 20 s per
  module, up to 15 archive modules per family; state = Paula's registers after
  each tick:

| Family | Played | State identical (before) | specdist median, to 2 kHz (before) |
| --- | --- | --- | --- |
| AC1D Packer | 7 | 2 (1) | 1.38 dB, 0.87 (6.61, 1.60) |
| Channel Player | 9 | 4 (1) | 0.96 dB, 0.52 (6.46, 1.41) |
| Digital Illusions | 15 | 5 (3) | 2.35 dB, 1.71 (6.79, 2.42) |
| Images Music System | 12 | 0 (0) | 10.62 dB, 10.54 (12.38, 10.52) |
| NoisePacker 2 | 15 | 13 (13) | 1.16 dB, 0.54 (6.44, 1.36) |
| NoisePacker 3 | 15 | 11 (9) | 1.27 dB, 0.67 (6.46, 1.37) |
| Pha Packer | 15 | 11 (9) | 0.96 dB, 0.30 (6.46, 1.25) |
| ProPacker 2.1 | 7 | 3 (2) | 1.63 dB, 0.74 (6.54, 2.02) |
| ProPacker 3.0 | 5 | 4 (3) | 2.58 dB, 1.74 (2.68, 2.10) |
| ProRunner 1 | 15 | 4 (1) | 1.82 dB, 1.05 (6.65, 2.00) |
| ProRunner 2 | 15 | 9 (6) | 2.09 dB, 1.45 (6.80, 2.21) |
| The Player 4.1A | 7 | 6 (1) | 2.74 dB, 1.92 (7.04, 2.55) |
| The Player 4.x | 15 | 7 (4) | 1.61 dB, 1.13 (6.72, 2.12) |
| The Player 5.x | 4 | 3 (2) | 1.58 dB, 1.23 (6.35, 2.54) |
| The Player 6.0A | 15 | 12 (8) | 2.49 dB, 1.69 (6.89, 2.70) |
| The Player 6.x | 15 | 10 (5) | 2.60 dB, 1.71 (6.79, 2.64) |
| Tracker Packer 3 | 15 | 9 (5) | 2.13 dB, 1.45 (6.88, 2.21) |
| Unic Tracker | 12 | 8 (5) | 2.29 dB, 2.00 (6.93, 2.59) |

  "Before" is the built-in ProTracker before it became a translation of
  ProTracker 2.3D (see Built-in ProTracker): it started with the LED filter
  off and computed fine-tuned periods by formula. Over all 213 modules, 121
  are state-identical now (78 before), specdist median 2.19 dB (6.75). Most of
  what still differs is PTK-Prowiz's: it plays these conversions in a
  NoiseTracker-like mode (the period written in every effects tick), starts
  portamentos a tick later than 2.3D, and does not play 0-length samples.
  Images Music System is the converter's choice against PTK-Prowiz's (Asle's
  ProWizard) conversion: the files' note numbers run from 12 to 47, which
  NostalgicPlayer's table (from 1712, so 12 is 856 and 47 is 113) maps onto
  ProTracker's three octaves exactly; PTK-Prowiz takes note n as ProTracker's
  period n - 1, 11 semitones higher, with notes above 36 off its table. Unic
  Tracker's octave drift was the old replayer's (its notes are now 8 of 12
  identical; the other 4 differ only by PTK-Prowiz's late portamentos). The
  reference refuses Magnetic Fields Packer, NovoTrade Packer, Devils Replay and
  HCD Protector, so their playback is unchecked.

## Licence

deliplayer is free software under the GNU General Public License, version 2 or
(at your option) any later version: see `LICENSE`. `src/paula.rs` and
`src/paulaout.rs` are derived from UADE's (UAE's) GPL audio emulation, and
`src/replay/tfmx.rs` from libTfmxAudioDecoder (GPL-2.0-or-later) by way of
NostalgicPlayer. The built-in ProTracker is translated from pt2-clone and
ProTracker 2.3F's replay source (BSD-3: `LICENSE-pt2-clone`), and the
MusicMaker replayers from Thomas Winischhofer's MusicMaker sources (BSD), the
AHX one from ahx2play (BSD-3: `LICENSE-ahx2play`), the PreTracker one from
Raspberry Casket and rePlayer's C port (MIT, released with permission from
Pink / Abyss: `LICENSE-PreTracker`). The
other open replayers are ported from NostalgicPlayer (MIT:
`LICENSE-NostalgicPlayer`, which has to travel with them). The 68k players
deliplayer runs (DeliPlayers, Eagleplayers, genies) and the Rust recompiled from
them (`crates/players-native/gen`) are not part of it and are not covered.
