# Infogrames (Amiga, `*.dum` + `*.ins`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Format of the Infogrames in-house music driver of the late 1980s (UADE player file name `Infogrames`,
described by its Eagleplayer as "Infogrames (RobHubbard2)"; games: Bob Morane, Stir Crazy featuring
Bobo, Captain Blood, Bivouac, Chamonix Challenge, Final Assault, ...). A song is two files: the
**song file** (`NAME.dum`, the module that is opened) and the **instrument file** (`NAME.ins`, the
sample bank). This spec describes what the reference 68k player (run through the host with
DELI_WRITE_LOG) does, as observed and checked: the module layout, the per-tick state machine, the
exact Paula writes and their colour-clock timing.

Conventions: all multi-byte values are big-endian. "Byte add" means modulo 256, "word" means 16 bit
modulo 65536. Offsets inside a song are relative to that song's header, unless said otherwise.
Times are colour clocks (cc); CPU costs are 68000 cycles (2 cycles = 1 cc).

## 1. File format

### 1.1 Song file (`.dum`)

| offset | size | content |
|---|---|---|
| 0 | 2 × N | song offset table: N words, offset of each song header from the file start |

N is not stored; the player takes it as `word0 / 2` (the first song header follows the table).

Song header (at its offset `H`), then data addressed relative to `H`:

| offset in header | size | content |
|---|---|---|
| 0 | 2 | speed (only the low byte is used): ticks per row |
| 2 | 2 | offset of volume-envelope table A |
| 4 | 2 | offset of pitch-envelope table B |
| 6, 8, 10, 12 | 2 each | offset of the track (sequence) of voice 0, 1, 2, 3 |
| 14 | 2 | end of the song data (not used by the player; in all files = table B offset + 32 × 13) |
| 16 | 2 × P | pattern offset table: offset of pattern p at `H + 16 + 2p` |

- **Track**: bytes, each a pattern number; `0xFF` ends the track.
- **Pattern**: a byte stream of events (section 3); `0xFF` ends the pattern.
- **Envelope tables A and B**: 13-byte entries, entry i at `table + 13 × i`. Table A is indexed
  0..31 by a command; table B by a full byte (all real files have 32 entries in each table).

Envelope entry (13 bytes):

| byte | meaning |
|---|---|
| 0 | table A: bit 7 = loop flag, bits 0..6 = loop step (0..127); table B: ignored |
| 1..12 | four segments of 3 bytes: `count`, `delta` (signed byte), `ticks` |

(For table A the loop step is byte 0 with bit 7 cleared, so it is always 0..127 and is added to
the envelope's base as a positive amount, which lowers the output; see 4.3 step 7.)

### 1.2 Instrument file (`.ins`)

The player loads it itself when the song is opened (1.4). Layout as used by the player:

| offset | size | content |
|---|---|---|
| 0 | 4 | not read by the player (meaning unknown, no real file was available) |
| 4 + 16 × s | 16 | sample entry s (s = 0..31) |

Sample entry (offsets relative to its own start; **sample addresses are relative to file offset 4**,
called the bank base below):

| offset | size | content |
|---|---|---|
| 0 | 4 | start of the sample, from the bank base |
| 4 | 4 | start of the repeat part, from the bank base |
| 8 | 4 | not read |
| 12 | 2 | length in words |
| 14 | 2 | repeat length in words |

Sample data are 8-bit signed PCM anywhere in the file; the player only passes addresses and lengths
to Paula.

### 1.3 Detection

A file is accepted when all of these hold (`len` = file length):

1. `w0 = word at 0` is not zero and is even, and `w0 < len`.
2. With `H = w0`: `a = word at H + 2` (table A offset); the byte at `H + a` is `0x00` and the byte
   at `H + a + 1` is `0x0F` (first volume envelope: flag/step byte 0 and first segment count 15).

The file name is not checked. Every real file starts its first table-A entry with `00 0F`.

### 1.4 Companion file name

The player builds the instrument file name from the song file's full path: if the name ends with
`.dum` (any case of `d`, `u`, `m`), that suffix is replaced by `.ins`; otherwise `.ins` is appended.
If that file cannot be loaded, it retries once with `.ins` written one character earlier, i.e. the
last character before the suffix position is dropped (`bob4a.dum` -> `bob4a.ins`, then `bob4.ins`;
`rh2.blood.dum` -> `rh2.blood.ins`, then `rh2.bloo.ins`). If both fail, the module does not load.
(The game archives name some banks differently, e.g. `rh.ins` next to `rh2.*.dum`; the reference
does not find those.)

### 1.5 Song, subsongs, title, length

- Songs: N = `word0 / 2`. The reference exposes **one** subsong to the host and always plays the
  **last** song, N − 1 (its own next/previous-song functions are never called by the host). All
  real files have N = 1.
- No title or instrument names are stored.
- The song never signals its end. Each voice stops when it reaches the end of its track (3.4);
  when all four have stopped, the player keeps writing volume 0 and period 0 every tick. A port
  may report song end when all four voices are stopped (the reference keeps running silently).

### 1.6 Tempo

The tick rate is set once when the song starts, by a CIA timer value:

- The player sums the song file as big-endian 32-bit words (`floor(len / 4)` whole words, modulo
  2^32). For `len mod 4 = r ≠ 0` the last partial word adds its `r` leading bytes in the high
  bytes, the rest zero. If the pair (sum, len) is in a 12-entry list of known songs kept in the
  player, the timer value is that entry's value (0x24FF for 11 entries; 0x1B66 for the entry with length 0xCA2).
  The list's exact bytes are in `data/tempo_table.bin` (12 × {u32 sum, u32 len, u32 value}).
- Otherwise the value is **0x1A00** (all available real files).
- (The player first asks the UADE host library for a configured "timer" value and uses it when
  given; the reference host does not provide it.)

Ticks are `(value + 1) × 5` cc apart: **33 285 cc** for 0x1A00 (verified over 12 787 ticks), 47 360
cc for 0x24FF (verified with a constructed file).

## 2. State and initialisation

Global: `speed` (header byte), `rowctr` (byte) = `speed`, master volume 63.

Per voice (4): track start and position, pattern position, `rows` (byte counter, 0),
`dur` (byte, 0), `transpose` (byte, 0), `period` (word, 0), `stopped` = false, `start` = true,
volume envelope A and pitch envelope B (both with no data selected, all fields 0).

Envelope state: `data` (the 12 segment bytes of the selected entry), `seg` (0, 3, 6, 9: byte index
of the current segment), `cur` (word), `base` (word), `step` (signed byte), `loop` flag, `pending`
flag, `done` flag, `tick` (byte), `rep` (byte).

Two fixed per-voice values are 63 in every reachable case (voice volume and target volume; the
player's volume-fade and sound-effect machinery is only reachable from game code, never from the
host), so the volume formula of 4.3 reduces to the envelope A value.

The 32-byte sample bank base is the instrument file + 4.

## 3. Events

### 3.1 The tick

Each tick:

1. `rowctr` = `rowctr − 1` (byte).
2. For voice 0, 1, 2, 3 in order: run the voice (3.2), producing a volume, a period and an optional
   sample to start; then write Paula for that voice (section 5).
3. If `rowctr` = 0: `rowctr` = `speed`.

So with speed S the row ticks (where `rowctr` is 0 during step 2) are ticks S, 2S, 3S, ...
(counting the first tick as 1); speed 0 behaves as 256.

### 3.2 Voice step

- **Stopped voice**: output volume 0, period 0, no sample. Nothing else.
- **Start** (first tick only, `start` set): clear `start`, set the track position to the track
  start, and go directly to *fetch the next track entry* (3.4) and read events (3.3). `rows` is not
  reloaded here.
- Otherwise: if this is a row tick (`rowctr` = 0), `rows` = `rows − 1` (byte). If `rows` ≠ 0, no
  events are read. If `rows` = 0: `rows` = `dur`, then read events (3.3).
- Then compute the output (4.3), unless the voice stopped while reading.

`rows` decrements only on row ticks, but the test for 0 happens on every tick that did not start
the voice. After a read `rows` = `dur`; if that is 0 (duration bytes 0x8D..0x8F, or no duration
command yet: initial `dur` is 0), events are read again on the next tick, row tick or not. A
non-zero `dur` gives one read every `dur` row ticks.

### 3.3 Reading events

Bytes are read from the pattern position, one after the other, until a note ends the row:

| byte | meaning |
|---|---|
| `0x00..0x7F` | **note** (ends the row, see 4.1) |
| `0x80..0x9F` | **duration**: `dur` = `rows` = DURATION[byte & 15] |
| `0xA0..0xBF` | **sample**: the sample to start this tick = byte & 31 (a later one in the same tick replaces it) |
| `0xC0..0xDF` | **volume envelope**: select table A entry byte & 31 (4.2) |
| `0xE0, 0xE8, 0xF0, 0xF8` | **transpose**: next byte -> `transpose` |
| `0xE1, 0xE9, 0xF1, 0xF9` | **pitch envelope, looping**: next byte = table B entry; select it with loop flag set |
| `0xE2, 0xEA, 0xF2, 0xFA` | **pitch envelope, one-shot**: next byte = table B entry; select it with loop flag clear |
| `0xE3, 0xEB, 0xF3, 0xFB` | no operation |
| `0xFF` | **end of pattern**: fetch the next track entry (3.4) and continue reading in that pattern |
| other `0xE4..0xFE` with low three bits 4..7 | undefined (the original jumps into data); never used |

DURATION (16 values): 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 8, 0, 0, 0.

A row can span a pattern boundary: commands before a pattern's `0xFF` stay in effect and reading
continues in the next pattern until a note.

### 3.4 Fetching the next track entry

Read the byte at the track position:

- not `0xFF`: advance the track position; the pattern position becomes the start of that pattern
  (`H + word at H + 16 + 2 × byte`).
- `0xFF` (end of track): the voice **stops**: `stopped` = true; output volume 0, period 0 for this
  tick. Event reading ends. (A sample selected earlier in the same tick is still started, 5.2.)
  (The player has a "loop the track" option that restarts the track instead; it is never set when
  the host plays a song.)

## 4. Notes, envelopes, output

### 4.1 Note

`n` = the note byte. The period index is `0` if `n` = 0, else `(n + transpose)` mod 256. `period` =
PERIODS[index] (104 words, `data/periods.bin`; indices ≥ 104 are outside the table in the original,
undefined, and never reached by real files). Index 0 is 27 340 (a "silent" period); the table runs
from 0x6ACC down to 0x0010 and ends with two zeros.

Then both envelopes restart (the selected data is kept): `seg` = 0, `cur` = 0, `base` = 0,
`tick` = 0, `rep` = 0, `pending` = true, `done` = false (the `loop` flag and `step` are kept).

A note does not start a sample; only a sample command does.

### 4.2 Envelope selection

Table A entry e (volume): `loop` = bit 7 of byte 0, `step` = byte 0 & 0x7F, `data` = bytes 1..12,
then `seg` = 0, `cur` = 0, `base` = 0, `tick` = 0, `rep` = 0, `pending` = true, `done` = false.

Table B entry e (pitch): `loop` as given by the command, `step` = 0, `data` = bytes 1..12 (byte 0 is
skipped), the rest as for A.

(A note played before any envelope of that kind was ever selected reads envelope data from an
undefined place in the original; no real file does that.)

### 4.3 Envelope step and output

The envelope step takes an input value `x` (word) and returns a value; it runs once per tick for A
(input 0) and then for B (input `period`), on every tick on which the voice is not stopped (also
the tick of a note, after the note).

Segment k (at `seg`): `count` = data[seg], `delta` = data[seg + 1] (signed), `ticks` = data[seg + 2].

1. If `pending`: clear it, `cur` = `cur` + `delta` (word).
2. `v` = `x` + `cur` − `base` (word). If bit 15 of `v` is set, `v` = 0. This is the result.
3. If `done`: stop here.
4. `tick` = `tick` + 1 (byte). If `tick` ≠ `ticks`: stop.
5. `tick` = 0, `rep` = `rep` + 1 (byte). If `rep` ≠ `count`: `pending` = true, stop.
6. `rep` = 0. If `seg` < 9: `seg` = `seg` + 3, `pending` = true, stop.
7. Last segment finished: if `loop` is clear, `done` = true, stop. Otherwise `base` = `base` + `step`
   (sign-extended, word), `seg` = 3 (the loop restarts at the **second** segment), `pending` = true.

So each segment adds `delta` `count` times, one step every `ticks` ticks (the first add happens at
the step after the note/selection, i.e. immediately on the note tick). `count` 0 or `ticks` 0 mean
256 (byte wrap).

Output:

- **volume** = (A result − (63 − min(63, master))) clamped at 0, low 8 bits. With master 63 this is
  the low byte of the A result. Values above 64 are written as they are (Paula clamps; the
  reference writes them, e.g. 80).
- **period** = B result (word, written as it is; 0 when it went negative).

## 5. Paula writes

### 5.1 Order

For each voice v (0..3) in turn, right after that voice's step:

1. AUDvVOL = volume
2. AUDvPER = period
3. if a sample s was selected for this voice in this tick:
   1. AUDvLC = bank base + entry[s].start (32-bit write: high word then low word, same time)
   2. AUDvLEN = entry[s].length
   3. DMACON = `1 << v` (voice off)
   4. wait for the audio DMA (5.3)
   5. DMACON = `0x8200 | 1 << v` (voice on)
   6. wait for the audio DMA
   7. AUDvLC = bank base + entry[s].repeat start
   8. AUDvLEN = entry[s].repeat length

Stopped voices still get VOL 0 and PER 0 every tick. There are no other writes (no filter, no
DMACON at start or stop).

### 5.2 Sample selection per voice

The selection is cleared at the start of each tick and after each voice's writes, so a sample
command only affects its own voice in that tick. Since every row ends with a note (or the voice
stopping), a sample start always comes with a note.

### 5.3 Audio DMA wait

The host's wait: from the current clock `t`, continue at `(floor(t / 227) + 10) × 227` (the start of
the 10th raster line after the current one).

## 6. Timing

### 6.1 Calls

- First tick: exactly one tick period after the end of the player's song start (initialisation).
  That initialisation costs 2 259 cc + 27.5 cc per whole 32-bit word of the song file for files
  that are not in the tempo list (it is dominated by the checksum of 1.6). This only fixes the
  absolute clock of the first tick; everything below is relative to each call.
- Then one call every tick period (33 285 cc).

### 6.2 Clock model

For a call at clock `C`, keep a cycle counter `T` = 0. A write is stamped `C + floor(T / 2)`, where
`T` includes the cost of the step that makes the write (listed as part of that step). At a DMA wait
(5.3) with `t = C + floor(T / 2)`, the new `C` is the wait's result and `T` restarts at 0. All costs
are fixed (no data-dependent variation besides the paths below).

### 6.3 Tick skeleton (cycles)

| step | cost |
|---|---|
| call entry to the first voice | 359 |
| per voice, before its step | 46 |
| voice step | see 6.4 |
| then up to and including the VOL write | 149 |
| PER write | 22 |
| after PER, no sample: to the next voice | 25 + 107 |
| after PER, with sample: up to and including the LC write | 25 + 162 |
| LEN write | 24 |
| DMACON off write | 16 |
| to the wait | 198 |
| after the wait, DMACON on write | 188 |
| to the second wait | 198 |
| after the second wait, repeat LC write | 208 |
| repeat LEN write | 24 |
| then to the next voice | 107 |

### 6.4 Voice step (cycles)

Entry: stopped voice **43** (total). Otherwise 121, then:

- start: + 54, then a track fetch;
- not start: + 21; + 16 on a row tick (counter decrement); + 23; if `rows` reached 0: + 20 and
  read events; else go to the output.

Reading (each byte read costs 41 first, including `0xFF`):

| byte | further cost |
|---|---|
| note | 51 + 25 (+ 12 if the note is not 0) + 52 + 282, then the output |
| duration | 51 + 19 + 19 + 74 |
| sample | 51 + 19 + 19 + 30 |
| volume envelope | 51 + 19 + 19 + 292 |
| transpose | 51 + 19 + 19 + 72 + 58 |
| pitch envelope (either) | 51 + 19 + 19 + 72 + 316 |
| no operation | 51 + 19 + 19 + 72 + 10 |
| `0xFF` | a track fetch |

Track fetch: 41; then an entry: + 122 and continue reading; end of track: + 25 + 107 + 16 and the
voice step ends (no output stage).

Output stage: 38 + envelope A + 37 + 21 + 78 + envelope B + 40.

Envelope step (6.4 numbers follow the steps of 4.3):

| path | cost |
|---|---|
| always | 73 |
| `pending` set | + 32 |
| then | + 43 |
| result clamped to 0 | + 4 |
| then | + 25; if `done`: + 16, end |
| tick counting | + 49; if `tick` ≠ `ticks`: + 16, end |
| segment repeat | + 65; if `rep` ≠ `count`: + 36, end |
| next segment | + 49; if not the last: + 48, end |
| last segment | + 21; no loop: + 36, end; loop: + 56 + 48, end |

### 6.5 Example (first tick of a voice with a sample, from the logs)

Gaps between consecutive writes of one voice with a sample: VOL -> PER 11 cc, PER -> LC 94 cc,
LC -> LEN 12, LEN -> DMA off 8, DMA off -> DMA on about 2 240..2 400 (10 lines plus the partial line),
DMA on -> LC 2 280 (10 lines + 104), LC -> LEN 12. Without a sample, VOL(v) -> VOL(v + 1) is about
700..730 cc on non-row ticks.

## 7. How to check

Compare DELI_WRITE_LOG output with the reference logs (LOGS.md) using tools/replay-cmp.py. The
instrument file in every log is synthetic (no real one was available); its load address is not in
the log header, so sample addresses differ from a port's by one constant, which replay-cmp accepts.
