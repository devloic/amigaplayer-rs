# Rob Hubbard (Amiga, `rh.*`) replayer: behaviour spec

> **Sanitised for publication.** This copy was reviewed before it was added to the repository. Removed or rewritten: the register-level recipe for locating the layout parameters in the init code (rewritten as a prose description of what is loaded), instruction-level wording in the detection constants and in one cycle breakdown. Numbers, formulas and behaviours are unchanged. References to `logs/` files, scripts and traces name data that is not shipped (see `docs/specs/README.md`).

Scope: the Amiga "Rob Hubbard" module format played by the UADE/Eagleplayer player "Rob Hubbard"
(prefix `rh`). Not covered: the Atari ST variant (`rho.*`, already `src/replay/robhubbardst.rs`) and
the separate "Rob Hubbard 2" family (`rh2.*.dum` + `rh.ins`, see README).

This is a behaviour description derived from write logs, DMA logs and execution traces of the 68k
reference. It contains no code of the original. The cycle numbers in section 8 are the 68000 cycle
counts that the reference machine charges (2 cycles = 1 colour clock, "cc").

Status (all of it checked by a private model that follows only this text, against the reference):

| item | result |
|---|---|
| 5 corpus modules, 22 subsong runs, about 61 000 register writes | value, order and tick-relative time of every write equal the reference |
| 5 synthetic modules (edited copies of corpus modules that exercise slide, vibrato, waveform modulation and the no-op/unknown commands) | same, about 58 000 writes; DMA log of the waveform modulation equals the model's sample memory (104 719 words) |
| paths never executed by any reference run | listed in README (rest command of the Populous generation, stop command, zero-length note) |

"Tick-relative time" = colour clock of a write minus the colour clock of the first write of its
tick. Tick start phase and tick spacing belong to the host framework (section 8.1).

## 1. File format

### 1.1 What a module is

A module is a complete, position-independent 68k executable image (no hunks, no header fields):
the Rob Hubbard driver code, followed by its data (tables, instrument records, song table, track
lists, note streams), followed by the sample bank. The reference runs this code as it is. An open
replayer reimplements the driver, so it must read the data from the image. The image is loaded at
any address; every pointer inside the image is stored relative to the image start and is made
absolute by the init code.

Entry points (offsets in the image): +0 play one tick, +4 init (d0 = subsong number, low 8 bits
used; d1 = see 7.2), +8 stop. +12 and +16 (install/remove an audio interrupt vector) are not used
by any player.

### 1.2 Detection

All of these hold:

- the 16-bit big-endian words at offsets 0, 4, 8, 12, 16 are `0x6000` (relative branches);
- the word at offset 20 is `0x41FA` and the 32-bit word at offset 28 is `0x4E7541FA`
  (detection constant covering the first bytes after the entry table);
- the sample bank (1.4) is well formed: starting at its first header and walking `n` entries
  (`header = u32 byte length, u16 rate`, then data) ends exactly on a 16-bit word `0x4E71` that is
  the last word of the file (`file size - 2`). `n` is the number of samples of the module. A
  length above 0x10000 means corrupt;
- the song table (1.5) can be located (below).

Files whose header matches but whose bank does not end on `0x4E71` are not Rob Hubbard modules
(for instance the `rh.ins` files of the `rh2` family do not start with these words).

### 1.3 Layout parameters

The driver code names its tables by displacements. These are the parameters an implementation needs.
All values are offsets in the image.

| parameter | meaning |
|---|---|
| `bank` | first sample header |
| `silent` | silent area (see 4.4): 64 bytes of zero in front of `bank` (generations A, C) or a 512-byte zero area (generation B) |
| `ins` | instrument records, 32 bytes each, `nsm` of them for the bank samples, followed immediately by 3 synth records at `synins` (`= ins + 32*nsm` in all corpus files) |
| `syn` | three 32-bit offsets (image-relative) of the three synth waveforms |
| `per` | period table, 16-bit words |
| `vib` | vibrato table base |
| `song` | song table, 18 bytes per subsong |
| `voices` | four voice records, `rec` bytes each (0x26 or 0x28) |
| `tempo` | one byte: ticks per duration unit, copied from the song table at init |
| `playflag` | one byte, non-zero while playing |
| `defins` | generation A only: one byte, default instrument |

Values for the corpus modules:

| module | gen | rec | nsm | ins | syn | synins | per | vib | song | voices | tempo | playflag | defins | bank | silent |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Bards_Tale_3 `rh.driver` | A | 0x26 | 5 | 0x448 | 0x43C | 0x5E8 | 0x3DA | 0x648 | 0x6AC | 0x342 | 0x33E | 0x33C | 0x340 | 0x12D4 | 0x1294 |
| PGA_Tour_Golf `rh.FLYMUS` | A | 0x26 | 6 | 0x448 | 0x43C | 0x5E8 | 0x3DA | 0x648 | 0x6B0 | 0x342 | 0x33E | 0x33C | 0x340 | 0xBDE | 0xB9E |
| PGA_Tour_Golf `rh.GMUSIC` | A | 0x26 | 11 | 0x44A | 0x43E | 0x5EA | 0x3DC | 0x64A | 0x6AE | 0x344 | 0x340 | 0x33E | 0x342 | 0x104E | 0x100E |
| Populous `rh.Populous` | B | 0x26 | 8 | 0x618 | 0x60C | 0x7B8 | 0x5AA | 0x818 | 0x87E | 0x312 | 0x310 | 0x30E | - | 0x1006 | 0x3AA |
| Budokan `rh.Budokan` | C | 0x28 | 13 | 0x468 | 0x45C | 0x608 | 0x3FA | 0x668 | 0x6CC | 0x35A | 0x358 | 0x356 | - | 0x1994 | 0x1954 |

Finding them in a module that is not listed: they are the pointers that the init code, the note-start code and the
instrument-select code load. The init code is the code the branch at offset 4 leads to, up to its first return; the play
code is the branch at offset 0. (The step-by-step decoding recipe was removed for publication; the cross-checks of
1.2 and 1.4 expose a wrong guess.)

- `silent` and `bank`: the first position-independent address load in init gives `silent` (generations A, C); in those
  generations `bank` is 0x40 bytes after it (a 64-byte skip is applied right after that load). In generation B there is no
  such skip: the load gives `bank` itself and `silent` is found in the play code instead (a base-relative address load
  used by the rest command).
- `ins`: the first position-independent load of the instrument-table pointer in init; `nsm` = the small counter constant
  of the copy loop that follows it, plus one (cross-check: the bank walk of 1.2 gives the same number).
- `syn`: the position-independent load of the synth-offset table in init; `synins`: the second instrument-table load in init.
- `voices`, `song`: the base-relative loads in init of the voice-record array and of the song table; `rec`: the stride
  added at the end of the per-voice init loop; `playflag`: the byte set at the very end of init; `tempo`: the destination of
  the byte copied from the song table; `defins`: the destination of the byte copied from the second argument (d1) at the very
  start of init.
- `per`: the position-independent load in the note-start code; `vib`: the one in the instrument-select code.

### 1.4 Sample bank

Consecutive entries starting at `bank`: `u32 length in bytes`, `u16 rate`, then `length` bytes of
signed 8-bit data. Entry `i` becomes instrument record `i`. The bank ends on the `0x4E71` word.

### 1.5 Instrument record (32 bytes, big-endian)

| off | size | meaning |
|---|---|---|
| 0x00 | u32 | address of the waveform (set at init, 2.1) |
| 0x04 | u32 | loop offset in bytes into the waveform; 0 = no loop, negative (0xFFFFFFFF) = keep what the first start wrote (synth records) |
| 0x08 | u16 | length in words (set at init = byte length / 2; synth records carry their own value) |
| 0x0C | u16 | period scale (set at init for bank samples: `floor(3579545 / rate)` as a 16-bit quotient; synth records carry their own value) |
| 0x0E | u16 | volume (0..64) |
| 0x10 | u16 | vibrato divisor; 0 = no vibrato |
| 0x12 | s16 | offset of this instrument's vibrato sequence from `vib` |
| 0x14 | u16 | waveform modulation limit; 0 = none |
| 0x16 | u16 | waveform modulation start index |
| others | | unused (zero) |

Fields not marked "set at init" are plain file data and are never written by the player. The
three synth records (index `nsm .. nsm+2`) get only their waveform address at init: image start +
the matching 32-bit offset of `syn`. Corpus synth waveforms are 16 or 32 bytes long.

Period table: 16-bit words, 49 entries in all corpus files (4096 down to 256, 12 per octave, the
usual equal-tempered ratio), indexed by the note byte (0..255). Index above 48 reads whatever
follows the table in the image (16-bit words at `per + 2*note`); no corpus stream uses it.
Vibrato sequence: signed bytes, terminated by `0x84`.

### 1.6 Song table, track lists, streams

Song table entry `s` (18 bytes at `song + 18*s`): byte 0 unused (0 in all files), byte 1 `tempo`, then
four u32 image-relative offsets, one per voice 0..3, each pointing to a track list.

- Track list: u32 image-relative offsets of note streams, terminated by a zero u32.
- Note stream: bytes, see section 4.

Subsong count: entries `0..n-1`, where entry `n` (counted from 1) is the first one whose first two
bytes are both zero (the entry after the last real one; the track lists follow the table directly).
Corpus: Budokan 6, Bards_Tale_3 8, FLYMUS 2, GMUSIC 1, Populous 1.

There is no title, no author and no time length in the format. The player loops forever; the length
a host reports is the "song end" event of section 7.4. The number of patterns of the longest track
list of a subsong (`Length` in the Eagleplayer's module info) is the only size figure.

### 1.7 Generations

Three code generations occur in the corpus (a fourth, 1991, is known from the Eagleplayer but not in
the corpus, README). They share everything above and section 3 to 5; differences:

| | A (Bards_Tale_3, FLYMUS, GMUSIC) | B (Populous) | C (Budokan) |
|---|---|---|---|
| voice record | 0x26 | 0x26 | 0x28 (adds the volume override word) |
| stream commands | 80 81 82 83 84 85 | 80 81 82 83 84 | 80 81 82 83 84 85 86 87 |
| instrument operand of 80 | operand >= 0x80 = use `defins` | always the operand | always the operand |
| rest/silent area | 64 zero bytes before `bank`, length 0x20 words | 512 zero bytes at `silent`, length 0x100 words | like A |
| volume written | instrument volume | instrument volume | override if non-zero, else instrument volume |
| bank | `silent + 0x40` | `bank` | `silent + 0x40` |

### 1.8 Generation D (Sanxion demo, `sanxion-demo.rh`)

An earlier driver with the same song table, track lists, note streams and sample bank, and with the layout of
generation B (512 zero bytes at `silent` found through a base-relative load, `bank` = the single position-independent
load of init, no `0x4E71`-preceded skip), but:

- the instrument table is loaded once and its records are 16 bytes (the same field offsets as 1.5: wave 0x00, loop
  offset 0x04, length 0x08, period scale 0x0C, volume 0x0E; the fields from 0x10 on do not exist). There are no synth
  records, no synth-offset table (`syn`), no vibrato table (`vib`) and no waveform modulation, so only the slide
  effect of section 6 exists;
- the voice record is 0x1C bytes (stride added at the end of the init loop); the layout of 3 is kept up to `ins`;
- stream commands: 80 81 82 83 84 (the ones of B); the 0x85 to 0x84 rewrite of 7.1 is applied as for the others;
- period table: 48 words (4096 down to 271) then zeros; the bank walk, `nsm`, the loop set-up (4.1), the note start (4.3)
  and the rest (4.4) are as in B (silent length 0x100 words);
- a note before any instrument select (7.3) does not read the 8-byte zero block: the voice's instrument pointer is 0 and
  the loop set-up of the first tick reads the long at address 0 (0, wave) and at address 4 (0x2348, the reference
  machine's ExecBase pointer, so the "loop offset") and the word at address 8 (0, length): AUDxLC = 0x2348 and
  AUDxLEN = 0 - 0x11A4 = 0xEE5C for each voice. A host property, reproduced as constants.

Timing is not the cycle model of section 8: the code is another one. The slot times were measured, not derived (see
`src/replay/robhubbard_dtab.rs`): a voice slot is a sequence of paths (loop set-up pair or silent pair, DMA-off,
effects, fetch with the commands met and the note or rest); each path has a duration and its writes have offsets,
fitted from 25 000 logged writes by least squares and rounded. Tick-relative stamps equal the reference's within one
colour clock for 93% of the writes (the rest are off by one; the three song-end wrap slots by up to 6). The reference's
log stamps lie 4020 colour clocks after the audio they produce; the port's first tick is placed for the audio.
Corpus: one module (1 subsong, 10 samples, 290 s to the song end); 97.8% of its frames equal the reference's.

## 2. Initialisation

Init is called once per (re)start with the subsong in d0. In order:

1. Stop: write DMACON (`0xDFF096`) = `0x000F` (clear the four audio DMA bits), then ADKCON
   (`0xDFF09E`) = `0x00FF` (clear all audio modulation bits), and clear `playflag`. (The host
   framework has already written volume 0 to the four channels twice before; see README.)
2. For each bank sample `i`: record `i`: waveform address = absolute address of its data,
   length (words) = byte length / 2 (16 bits), period scale = `3579545 / rate`, 16-bit quotient
   (if the quotient does not fit 16 bits the field becomes the low half of 3579545, i.e. 0x9E99;
   no corpus rate does this).
3. For synth record `k = 0..2`: waveform address = absolute address of image + `syn[k]`.
4. `tempo` = byte 1 of the song table entry (`d0 & 0xFF`).
5. For voice `v = 0..3`: counter (+0x10) = 1; flags (+0) = 0; track list pointer (+8) = absolute
   address of the list given by the entry; list index (+0xC) = 4; stream pointer (+4) = absolute
   address of the first stream in the list. Generation C also clears the volume override. Nothing
   else in the voice record is touched (it keeps whatever it held, zero in the file; the
   Eagleplayer also preloads the instrument pointer, see 7.3).
6. Set `playflag`.
7. Generation A: the byte `defins` = d1 (7.2).

No Paula register is written by steps 2 to 7.

## 3. Voice record

Offsets, per voice (voice `v` owns channel `v`, registers `0xDFF0A0 + 16*v`):

| off | size | name | meaning |
|---|---|---|---|
| 0x00 | byte | flags | bit 1 = slide active, bit 2 = modulation direction (1 = down) |
| 0x02 | byte | note | current note index |
| 0x04 | u32 | stream | pointer to the next stream byte |
| 0x08 | u32 | list | pointer to the track list |
| 0x0C | u32 | listpos | byte offset in the list of the next entry (starts at 4) |
| 0x10 | u16 | counter | ticks left in the current event |
| 0x12 | u16 | period | current period (without vibrato) |
| 0x14 | byte | slide | signed slide step per tick |
| 0x16 | byte | started | 0 = the note was just started and its loop part is not set up yet; 0xFF otherwise |
| 0x18 | u32 | ins | pointer to the selected instrument record |
| 0x1C | u32 | vibpos | pointer to the next vibrato byte |
| 0x20 | u32 | vibstart | start of the vibrato sequence of the selected instrument |
| 0x24 | u16 | modidx | waveform modulation index |
| 0x26 | u16 | override | (generation C only) volume override, 0 = none |

## 4. The tick

The play routine is called once per tick (50 Hz, one call per vertical blank). If `playflag` is
clear it returns at once (no writes).

Otherwise the four voices are handled in the order 3, 2, 1, 0 (channel 3 first). For each voice,
`ins` is read at the start (an instrument select later in the same tick changes the voice record
but the rest of this voice's tick uses the selection in force when the slot started, except for what
the commands themselves do, see 5).

For voice `v`, in this order:

### 4.1 Loop set-up (once per note)

If `started == 0`: set `started = 0xFF`, then look at the instrument's loop offset `lo`
(a signed 32-bit value):

- `lo == 0`: write the silent area to the channel: AUDxLC (two word writes, high then low) =
  address of `silent`, AUDxLEN = 0x20 (generations A, C) or 0x100 (B).
- `lo < 0`: nothing.
- `lo > 0`: AUDxLC = waveform address + `lo`; AUDxLEN = waveform length (words) minus
  `lo >> 1` (16-bit arithmetic: `lo` is shifted as a 16-bit value, the subtraction is 16-bit).

This happens at the first tick after a note start (the sample has played its start part by then,
because the hardware latches AUDxLC when the DMA starts and re-latches at each loop end).

### 4.2 Counter

`counter = counter - 1` (16-bit wrap).

- `counter == 0`: the event is over: fetch the next event (4.3). No effects are processed on this tick.
- `counter == 1`: write DMACON = `1 << v` (switch this channel's DMA off one tick before the note
  ends so that the next note can restart it), then process the effects (section 6).
- otherwise: process the effects (section 6).

A zero-length event (counter 0 at start) therefore decrements to 0xFFFF and lasts 65 535 ticks.

### 4.3 Event fetch

Clear `flags` (all slide/modulation state goes). Then read stream bytes in a loop; `b` is the
byte read, the stream pointer advances. The loop ends when a note or rest is processed.

- `b < 0x80` (note): `duration = b * tempo` (16 bits); the next stream byte is the note index
  (stored in `note`); `stream` = pointer after it. `modidx = instrument[0x16]`; `vibpos = vibstart`.
  Then write, in this order: AUDxLC (two words) = waveform address, AUDxLEN = instrument length,
  AUDxVOL = volume (instrument volume, or the override word if generation C and the override is
  non-zero; value masked with 0x7F), AUDxPER = `(per[note] * scale) >> 10` (32-bit product of the
  table word and the instrument scale, shifted right 10, low 16 bits; also stored in `period`),
  DMACON = `0x8200 | (1 << v)`. Finally `started = 0`. `counter = duration`.
- `b >= 0x80`: command, section 5. After a command other than rest the loop continues with the next byte.

So a note: LC (hi, lo), LEN, VOL, PER, DMACON-set, in that order, always, with no DMA-off write in
the same tick (the previous note switched the channel off one tick earlier). The two LC words are
written by one 32-bit store: both carry the same time stamp.

### 4.4 Rest and the silent area

A rest sets the counter, stores the stream pointer and points the channel at the silent area:
AUDxLC = `silent`, AUDxLEN = 0x20 (0x100 for generation B). DMA is not touched: the channel keeps
playing the (silent) area, and the DMA-off write at counter 1 still happens. `started` is not
changed. The rest tick has no effects.

## 5. Stream commands

All operands are one byte. Unknown commands (0x80 and above that are not listed for the
generation) consume only the command byte and do nothing; the loop continues.

| cmd | gen | operand | action |
|---|---|---|---|
| 0x80 | all | instrument n | `ins` = record `n` (offset `n << 5` from `ins`, `n` is 0..255 unchecked; synth records follow the bank records). A only: if `n >= 0x80`, `n` = `defins`. `vibstart = vibpos = vib + s16 at record +0x12`. |
| 0x81 | all | step | `slide` = signed step; set flag bit 1. The command does not end the event; a note or rest follows. |
| 0x82 | all | n | rest of `n * tempo` ticks (4.4). Ends the event. |
| 0x83 | all | none | nothing |
| 0x84 | all | none | next pattern: `a = list + listpos`; `listpos += 4`; if the u32 at `a` is 0, the song wrapped: use `a = list` and `listpos = 4`; the stream pointer becomes image start + the u32 at `a`. (Wrap point: the track list ended; the first pattern is replayed; the second one is `listpos = 4`.) |
| 0x85 | A, C | none | stop: clear `playflag`, write DMACON=0x000F and ADKCON=0x00FF and leave the tick routine at once (remaining voices are not handled). Unreachable in the reference, see 7.1. B: unknown command. |
| 0x86 | C | none | nothing |
| 0x87 | C | volume | `override` = unsigned byte (0 clears the override) |

## 6. Effects (every tick that is neither a note start nor a rest start)

Order: slide, vibrato, waveform modulation. They read `ins` at the time the slot started.

- Slide: if flags bit 1: `period = period + slide` (16-bit, `slide` sign-extended); write AUDxPER = `period`.
- Vibrato: if instrument divisor `d != 0`: `q = period / d` (unsigned 16-bit division, quotient 16 bits);
  `x` = the signed byte at `vibpos`, then `vibpos` advances; if the byte read is 0x84, reload `vibpos` from
  `vibstart`, read the byte there (and advance). Write AUDxPER = `(period + q * x) & 0xFFFF` (`q` as signed 16-bit,
  product 32-bit, only the low 16 bits are used). `period` itself is not changed. If a slide wrote
  AUDxPER in the same tick, the vibrato write follows it.
- Waveform modulation: if instrument limit `L != 0` (instrument start index `S` = field 0x16):
  the byte memory of the instrument's waveform is changed in place. `idx = modidx`; if flags bit 2
  is clear: `idx += 1` (16-bit); if `idx <= L` store byte 0 at `waveform[idx]`; else set bit 2 and
  store byte 0x3C at `waveform[idx]`. If bit 2 is set: `idx -= 1`; if `idx > S` (unsigned) store
  0x3C at `waveform[idx]`; else clear bit 2 and store 0 at `waveform[idx]`. `modidx = idx`.
  There is no register write. The memory change is audible through Paula's DMA reads; it is
  permanent (not undone at a new init) and shared by all voices using the instrument.

Effects use the instrument that was selected when the voice slot started; during a rest they keep running
with the last selected instrument (the vibrato keeps writing AUDxPER).

## 7. Reference-host behaviours that appear in the output

The reference is the Eagleplayer wrapped around the module; the following things it does change
what the module does, and the logs contain them.

### 7.1 Stop command rewrite

At player init the Eagleplayer replaces every byte equal to 0x85 by 0x84 in the image range from
the terminating song-table entry (the first entry whose first two bytes are zero) up to and
including the first byte of `bank` (assumed less than 64 KiB long). That covers the track lists,
streams and tables, and is not stream-aware: a data byte 0x85 (a slide step of -123, an operand)
is changed too. Consequently no module ever executes "stop": the song loops.

### 7.2 Default instrument (generation A)

The byte `defins` is the value of d1 on entry to init. The Eagleplayer sets d1 to the number of
entries of the track list of voice 3 of the selected subsong (entries before its terminating zero)
as a side effect of its own length scan. Instrument operands >= 0x80 select the instrument with
this number. Observed: Bards_Tale_3 subsongs 3 and 7 use it (value 3); the other subsongs never do.

### 7.3 First note without instrument

The Eagleplayer pre-sets every voice's `ins` to an 8-byte block of zero bytes. A voice that starts a
note before any instrument select has instrument fields = zero (LC 0, LEN 0, ...) read from that
block and the memory after it; and the first tick's loop set-up (4.1) finds `lo == 0` and writes
the silent area. No corpus stream does this except the loop set-up of the very first tick of each
voice: the first tick of every subsong writes AUDxLC=`silent`, AUDxLEN=0x20 (0x100 for B) for every
voice (they appear as the first 12 writes in the logs).

### 7.4 Song end

On every execution of command 0x84 (including those in the wrapped case) the Eagleplayer notes the
position and detects the song end:

- Position: when the voice is the voice with the longest track list in the subsong (ties: the highest
  voice number), the reported position is `listpos_before_update / 4`.
- Song end: when a voice wraps (the entry read is the terminating zero), the voice's flag is cleared.
  When all four flags are clear, the host's song-end notification is called and the flags are set
  again. The notification does not stop or change the player: the song keeps looping, so a host
  decides what to do. The register writes are identical with or without it.

Expected first song end in the corpus logs: after the longest voice has finished all patterns and
every other voice has wrapped at least once.

### 7.5 Volume and balance

The wrapper passes AUDxVOL through a volume stage: `v & 0x7F`, times a per-channel gain (64 at the
defaults used), shifted right 6: identity for defaults. Channels 0 and 3 use the left gain,
1 and 2 the right gain. The logs are taken with default gains.

## 8. Timing

### 8.1 Frame

Ticks are called every 70 940 colour clocks (exactly, between two consecutive calls, verified over
about 1 300 ticks of two modules). The absolute colour clock of the first tick is host business
(it depends on the Eagleplayer's init, 140 000 .. 250 000 cc after reset in the logs). The player
is entered through the Eagleplayer's interrupt wrapper which costs **256 cycles** before the first
player cycle; this constant is only visible in absolute call times, not in tick-relative times.

### 8.2 Clock model

All costs below are in 68000 cycles. For a tick, keep a cycle counter `T` starting at 256 at the call.
A register write is stamped `call_time + floor(T / 2)` where `T` includes the cost of the writing
operation itself (the cost of a write is therefore listed as part of the step that makes it).
Both words of a 32-bit store (AUDxLC) get the same stamp. Costs are static (no data-dependent
variation), including taken/not-taken branches.

One exception: the song-end notification (7.4) re-bases the clock: cycles after it count from the
clock value reached at the notification: `stamp = floor(T_at / 2) + floor((T - T_at) / 2)`.
Only the tick that fires it is affected, and only by at most one cc.

### 8.3 Tick skeleton

| step | cycles |
|---|---|
| tick entry | 256 |
| play routine start, `playflag` set | 33 |
| (`playflag` clear: 45 and the tick ends) | |
| per voice, in order 3,2,1,0: slot head | 173 |
| per voice: loop set-up check when `started == 0` | 41, then 4.1 below |
| per voice: counter decrement and zero test | 25 |
| end of a voice (jump to next) | 10 |
| after the last voice | 16 |

The cycles of the last voice's 10 and the 16 close the tick (no writes).

After the 25 (counter test):

- counter non-zero: 27, then if the counter is 1: 12, DMACON-off write (16 incl. the write), 10; then the effects (8.5);
- counter zero: event fetch (8.4).

### 8.4 Loop set-up and event fetch

Loop set-up (4.1), after the 41:

| case | cycles |
|---|---|
| `lo == 0` | 8, LC write (16), LEN write (16) |
| `lo < 0` | 9 |
| `lo > 0` | 57, then helper block LOOP(c) (below), then 14 |

Event fetch: 12 (clear flags), then for every stream byte read: 23. After that:

- note (`b < 0x80`): 162; helper block NOTE(c) (below); then the zero-fill of the replaced code
  area (48 for generation C, 24 for A and B); then 132; then helper block PERW(c); then 4;
  then 8 + 8 + 16 (the DMACON-set write is the 16), then 16; then the voice ends (10).
- command: the dispatch tests in order; each test costs a fixed amount, the tests up to and
  including the matching one are paid:

  | gen | test order and cost of each |
  |---|---|
  | C | 0x86: 19, 0x84: 19, 0x80: 17, 0x81: 17, 0x82: 17, 0x83: 17, 0x85: 17, 0x87: 19 |
  | B | 0x84: 17, 0x80: 17, 0x81: 17, 0x82: 17, 0x83: 17 |
  | A | 0x84: 19, 0x80: 17, 0x81: 17, 0x82: 17, 0x83: 17, 0x85: 17 |

  A byte that matches none pays all tests plus 10, and the loop reads the next byte (23 again).
  After a matching test the command costs:

  | cmd | cycles |
  |---|---|
  | 0x80 | 126 (generation A: plus 11, and a further 12 if the operand was >= 0x80) |
  | 0x81 | 42 |
  | 0x82 | 130, LC write (16), LEN write (16), 10; the voice ends (no further 10, the loop jump is the 10) |
  | 0x83, 0x86 | 10 |
  | 0x87 | 34 |
  | 0x84 | see 8.6 |

  After 0x80, 0x81, 0x83, 0x84, 0x86, 0x87 the loop continues (another 23 for the next byte).

### 8.5 Effects

| step | cycles |
|---|---|
| entry (always) | 27 |
| slide (flags bit 1) | 28, then helper block PERW(c) with its PER write, then 4 |
| vibrato check | 21; if the divisor is non-zero: 181, plus 24 if the vibrato byte was 0x84, then 58, then helper block VIB(c), then 16 |
| modulation check | 35 (generation A: 33); if the limit is non-zero: 61, then 17 and one of: up/within limit 40; up/limit exceeded 26 + 40; down/above start 40; down/at start 26 + 40 |
| end of the voice | 10 |

### 8.6 Command 0x84 (next pattern)

48, then 20 + 24 + 12 + 9 (the host helper call and its bookkeeping), then:

- list not at its end: 18 + 9 + (32 if this voice is the position voice of 7.4) + 28 + 12 + 16;
- list at its end: 8 + P(v) + 12 + 9, then if this wrap clears the last flag: 20 + 20 + 16 + 16 + 16 (then the song-end notification, and 16 for its return), then 16 + 4, then the same 18 + ... tail as above.
  `P(v) = 45, 72, 95, 108` for voice `v = 0, 1, 2, 3`.

Then 12 (replaced-code area), then 8 + 16 + 10.

### 8.7 Helper blocks (hardware writes with channel-dependent timing)

The reference writes the sample registers through small routines of the Eagleplayer whose cost depends on
the channel `c` (the routines find the channel by comparing, the highest channel falls through).
Define the channel chain: `CH(c) = 31, 62, 93, 101` and

- `ADR(c) = 52 + CH(c)`, `LENR(c) = PERR(c) = SETV(c) = 48 + CH(c)`;
- `CV(c) = 174, 197, 220, 210` (volume stage, up to and including its AUDxVOL write; 16 more to its return).

Each block below starts with its call (20 cycles, included) and ends when control returns to the
play routine; write times are cycles since the block start, including the writing operation.

NOTE(c), generation C:

- LC write at 72;
- LEN write at `72 + 18 + ADR(c) + 24`;
- VOL write at `LEN write + 18 + LENR(c) + 37 + 18 + CV(c)`, where 37 becomes 49 if the override is non-zero;
- the block ends `16 + 18 + SETV(c) + 28 + 16` after the VOL write.

NOTE(c), generations A and B: the same, with 12 instead of 37 (and no override).

| c | gen C: LC, LEN, VOL, end | gen C with override: VOL, end | gen A/B: LC, LEN, VOL, end |
|---|---|---|---|
| 0 | 72, 197, 523, 680 | 535, 692 | 72, 197, 498, 655 |
| 1 | 72, 228, 608, 796 | 620, 808 | 72, 228, 583, 771 |
| 2 | 72, 259, 693, 912 | 705, 924 | 72, 259, 668, 887 |
| 3 | 72, 267, 699, 926 | 711, 938 | 72, 267, 674, 901 |

LOOP(c) (loop set-up with `lo > 0`): LC write at 36; LEN write at `36 + 18 + ADR(c) + 12`; end at LEN
write `+ 18 + LENR(c) + 16`: c=0: 36, 149, 262; c=1: 36, 180, 324; c=2: 36, 211, 386; c=3: 36, 219, 402.

PERW(c) (period write of a note start or of a slide): PER write at 44; end at `44 + 18 + PERR(c) + 16`:
c=0: 44, 157; c=1: 44, 188; c=2: 44, 219; c=3: 44, 227.

VIB(c) (vibrato period write): PER write at 36; end at `36 + 12 + 4 + 18 + PERR(c) + 12 + 16`:
c=0: 36, 177; c=1: 36, 208; c=2: 36, 239; c=3: 36, 247.

Note the position of the writes inside the note start (cumulative from the end of the 162):
LC, LEN, VOL from NOTE(c); PER from PERW(c) after the 132; DMACON-set last.

### 8.8 Observed gaps (cc) between the writes of a note start, from the reference logs

(LC -> LEN, LEN -> VOL, VOL -> PER, PER -> DMACON; the +-1 comes from the per-tick floor)

| channel | gen C (Budokan) | gen B (Populous) | gen A (Bards_Tale_3, FLYMUS, GMUSIC) |
|---|---|---|---|
| 0 | 62/63, 163, 190/191, 74/75 | 62/63, 150/151, 178/179, 74/75 | 62/63, 150/151, 178/179, 74/75 |
| 1 | 78, 196, 206, 90 | 78, 177/178, 194, 90 | 78, 177/178, 194, 90 |
| 2 | 93/94, 217, 221/222, 105/106 | 93/94, 204/205, 209/210, 105/106 | 93/94, 204/205, 209/210, 105/106 |
| 3 | 97/98, 216, 225/226, 109/110 | 97, 204, 213, 110 | 97/98, 203/204, 213/214, 109/110 |

Rest and silent-area writes: LC -> LEN = 8 cc. Loop set-up with `lo > 0`: LC -> LEN 113/144/175/183... (use 8.7).
Typical tick-relative start of the voices of a tick: voice 3's first write is the tick's first
write (time 0 in the logs); each further voice adds its slot head (173 cycles) plus what its slot does.

## 9. How to check

`tools/replay-cmp.py REF.log PORT.log` (in the repo) compares logs tick by tick. The logs in
`logs/` are the references (LOGS.md). Reference runs start the first tick at a phase chosen by the
framework, so compare tick-relative times (first write of each tick = 0), as the Jesper Olsen spec does.
