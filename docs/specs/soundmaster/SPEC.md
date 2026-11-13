# Sound Master (`sm.*`, `sm2.*`, `sm3.*`, `smpro.*`) replayer: behaviour spec

*Behaviour only: no code or data of the original.*

Scope: modules of Michiel Soede's Sound Master (1990-94), played by the Eagleplayer
"SoundMaster" (UADE player file `SoundMaster`). A module is a raw memory image: the replay routine
compiled together with one song and its samples (the routine is not run by the open replayer).
Source: `src/replay/soundmaster.rs`. The behaviour below was drawn up from the Paula register writes
of the reference player run through the host and from the structure of the corpus files; the open
replayer is checked against those writes (see Status).

Status (UADE's SoundMaster Eagleplayer run alone against the open replayer, ordered Paula writes
per 50 Hz tick, `tools/replay-cmp.py`; refcmp for the sound):

| item | result |
|---|---|
| first corpus (13 files: builds A, B, C) | identical writes in every tick, 20 s, and 110-280 s runs of one file per build through the song wrap |
| second corpus (44 files: builds A-8, A+8, A, B/C, D) | all recognised; identical writes in every tick for 20 s (1000 ticks); 150-200 s runs (through the wrap where there is one) identical with equal tick counts for the 30 or so files run that long; second songs of 3 files identical |
| Michael Jestram `.sm3` (14 files) and delta-rob hubbard.sm | all recognised as build D; identical writes in every tick for 20 s (1000 ticks); 180 s runs of 7 files (9000, 5380, 6945, 5125, 4610, 3845 ticks) identical with equal tick counts; second songs of necromania-loader fixed and x-out-loader & jingle identical (60 s) |
| refcmp, SoundMaster alone | 44 of 44 SIMILAR (the WAVs differ in write timing and phase only), no inconsistency |

## 1. Builds

The routine exists in several builds; they differ in layout and in small points of behaviour.

| build | corpus | header shift | voice record | step table | notes |
|---|---|---|---|---|---|
| B | Future Shock | fixed layout | 0x34 | columns of 0x40 bytes at `G + 0xE8` | volume vibrato; 64-entry periods |
| C | unused Doofus song | B shifted by 0x10 | 0x38 | as B | sample sweep; 46-entry periods |
| A | Doofus, `.sm3`/`.smpro` families | 0 | 0x34 | 8-byte steps at `G + tb` (`tb` 0xF4) | tables behind an offset table, master volume per voice |
| A-8 | rackney'sisland 1, 2 | -8 | 0x32 | `tb` 0xEC | start at step 0; speed (0xFD) and break (0xFE) words; no sound-effect record copy |
| A+8 | rackney'sisland 3..8 | +8 | 0x36 | `tb` 0xFC | plays as A |
| D (version 3) | `.sm3` of v3, delta-rob hubbard | +0x20 | 0x3C | `tb` 0x12C | A's tables with B/C's tick, a global fade, sweep, volume vibrato |

`G` is the routine's variable base: the target of the first PC-relative `LEA` of the init routine
(within 30 bytes of its start). The A family is told by the init's table setup (`LEA tb(A3),A0`,
`ADDA.L yy(A3),A0`, `MOVE.L A0,d(A3)`): the header shift is `yy - 0xEE`, `tb` the step table's offset.
Header fields of the family are at `G + field + shift` (field offsets of build A below).

### 1.1 Recognition

A module is refused unless: it starts with three `BRA.W` words (offsets positive and even, the
branches at 0, 4, 8), the init routine holds the `LEA`, and the `RTS` that ends it follows the
audio-filter operand (`$BFE001`, with an optional `MOVE.B #0,d(A3)` store in between). The family is
told by its table setup; B and C by the init routine at 0x0C and the voice stride (`ADDA.L #0x38`: C);
D by init at 0x0C with the stride `0x3C`. The period table (46 or 64 words) is the one the note
masking's `LEA` names. The family needs only `G + tb + 8` bytes (small loader files); B and C
need `G + 0x1200`.

### 1.2 Layout common to the A family (offsets from `G + shift`)

- `0xD1..0xD4`: speed (ticks per row), pattern length (bytes: 2 per row), loop step, end step.
- long at `0xD6`: pattern list (per step and voice: pattern number, transposition), at `0xDA`:
  instruments (16 bytes each, 64), at `0xDE`: arpeggio notes, at `0xE2`: arpeggio samples, at `0xE6`:
  patterns, at `0xEE`: sample table (10-byte records: start, length, loop start, loop length), all
  relative to the step table; `0xEA`: base added to every sample start; `0xF2`: a silent word.
- A step (8 bytes): first position in the pattern list, last position, transposition, fade byte, the
  four voices' instrument offsets.
- Build D's variables hold the loop step (`0xF5`) and end step (`0xF6`) the routine uses, which may
  differ from the header's; a second song is named by `0xF7` (start), `0xF8` (loop), `0xF9` (end),
  absent when they read 0, 0, 1.

## 2. Songs

- Build D: one song, or two when the three bytes above are not 0, 0, 1; song 1 starts at its start
  step with its loop and end. Build A-8: two songs when the loop step is not 0, song 1 playing from
  the loop step. Builds A, A+8, B and C have one song: UADE's SoundMaster player reports one subsong
  (`uade123 -g`: max subsong 0) for every module of those builds in the second corpus (rackney'sisland 2..8,
  Doofus, Future Shock, the `.smpro` family: 41 of the 44 files; only rackney'sisland 1 and two build D modules
  report two), whatever their header's loop step holds, so the corpus shows no rule to implement.
- Starting a song (host): DMA off, the audio modulation register `ADKCON = 0x00FF`; a subsong change
  first writes DMACON `0x000F`, volumes 0, DMACON `0x000F`.
- Build A's init runs in the host's start call: per voice the silent loop, length 1, period 0xA0,
  then DMACON `0x820F`; the other builds' init runs in the first tick.
- Starting step: build A (and A+8) the loop step (the play request is the Eagleplayer's fixed 31),
  A-8 and D step 0, B and C the loop step.
- LED: the init clears the CIA-A LED bit then sets it (filter off) unless the byte named by its
  `TST.B d(A3)` is not 0 (4 of the 44 small modules); the LED then stays on.

## 3. The tick (50 Hz)

Per tick, in voice order: write the loop registers (length then location in B/C/D, location then
length in A; a voice with an empty loop gets the silent word, length 1), and at a row start read the
row; then DMACON with the voices that start a note; per voice, at a row start start the note, then
the effect pass; then advance, then the closing DMACON (`0x8200` or'd with the voices that start
and those released from the previous tick). Write times between those are fixed numbers of colour
clocks (loop 60, row 160, note 900, effect 280).

### 3.1 Rows and steps

Rows are (note, parameter) pairs, patterns hold `patlen/2` rows. A row advances every `speed` ticks.
Note `0xFD` sets the speed to the parameter, `0xFE` breaks to the end of the pattern (B, C, A-8, D;
A and A+8 are not known to use them). After the last row of a pattern the position moves on in the
step's range, then to the next step. When the step equals the end step the song restarts at the loop
step (speed, fades and master volume reset) and the song end is announced; play goes on.

### 3.2 Notes

Notes 1..0x7F start a sample: bit 6 (`0x40`) is a portamento (the new period approached at the
parameter's rate), bit 7 (`0x80`) a hold. Parameter bit 6 sets the volume, bit 7 forbids the
transposition. The period is `table[(note + transposition + step transposition + tune - 0x12) & 0x3F]`
(B: no `- 0x12`, 64 entries); out of the 46 entries the lowest period is used. The instrument number
is the parameter unless bit 6 of the note or parameter keeps the last one; the voice's instrument
offset from the step is added (A-8 and A: not while a sound effect plays).

### 3.3 Instrument (B 14 bytes, C/A/D 16)

sample, attack step, peak volume, release step, floor volume, vibrato delay, depth, length and
increment (low nibble: length, high: increment), volume-vibrato depth (B, D), arpeggio position, end,
loop, fixed note, tune; C/D: sweep length and step/mask. Volumes are kept in 1/16 units; attack adds
the attack step up to the peak (0: peak at once), release (note 0) subtracts the release step down
to the floor. The vibrato swings the period by its depth in a triangle; the depth grows by the
increment on the first half-swing (A, C, D: only while it is at most 0x40). The arpeggio steps
through the note and sample tables (`0x80`: no change, `0x81`: the fixed note, `0x82`: in C and D
holds the sweep); a sample number with bit 7 loops the whole sample, with bit 5 (A) keeps the
loop. The sweep moves the loop start by its step every tick the masked counter is 0, reversing at its
length.

### 3.4 Volume and fades

The fade byte of a step: bit 7 clear sets the master volume to the byte (B, C, D) and ends a fade;
bit 7 set starts a fade of rate `byte & 0x1F`, bit 6 up (to 0x40) or down (to 0), bit 5 jumping to
the end first. B and C step the master volume at each count 0; A steps each voice's volume, D the
global one, when the count is 1. The written volume is `level >> 4`, times master/64 when master is
not 0x40 (and the voice is not exempt: instrument offset bit 7).

## 4. Known differences and gaps

- The WAVs differ from the reference in write timing and phase (refcmp class SIMILAR), not in the
  sound; the reference's AUDxDAT writes and absolute times are not reproduced.
- A and A+8 use of the speed and break words were not observed (no module in the corpus has them);
  A, A+8, B and C offer one song (section 2); build D's second-song semantics were inferred from three files.
- Build A's sound-effect entry points are not offered (the host never calls them); the 80 bytes
  of effect sample records its init copies to sample records 16..23 are reproduced for the A build
  itself.
