# AudioSculpture (`adsc.*`, `mod_adsc4.*`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: modules played by the UADE player "AudioSculpture" (the AudioSculpture / StarTrekker "AM"
replay, adapted for UADE, subsong scan added later). A module is a 4-channel 31-sample tracker module
whose synthetic ("AM") instruments live in a separate companion file. The format is ProTracker-like
but older and different in many details (section 6 and 7); do not reuse a ProTracker replayer.

This is a behaviour description derived from the player, register-write logs, DMA logs and execution
traces of the reference 68k player run through the host. It contains no code of the original.

Status (checked by a private model that follows only this text, against the reference):

| item | result |
|---|---|
| 5 real modules (with synthetic companions, see README), 12 runs in total with the synthetic modules, about 587 000 register writes | address, value, order and colour clock of every write equal the reference (0 differences), including the song-end tick and the ticks after it |
| beam-counter reads (line waits and noise generator), Muppet run | clock and horizontal position of all 777 reads in the first 14 ticks equal the execution trace |
| noise waveform content against Paula's DMA fetches (fx1, 20 s) | 4704 of 4706 fetched words equal; 2 differ (README) |
| code paths never executed | only the companion's user-code slots (section 1.4) |

Units: "cycles" are 68000 CPU cycles of the reference machine; 2 cycles = 1 colour clock ("cc").

## 1. Files

### 1.1 Module

A module is a plain big-endian file:

| offset | size | content |
|---|---|---|
| 0x000 | 20 | title, NUL padded |
| 0x014 | 31 x 30 | sample headers, instrument 1 to 31 |
| 0x3B6 | 1 | song length (number of order positions) |
| 0x3B7 | 1 | restart position |
| 0x3B8 | 128 | order list (pattern numbers) |
| 0x438 | 4 | tag `EXO4` or `FLT4` |
| 0x43C | npat x 1024 | patterns |
| after the patterns | | sample data, in instrument order |

Sample header (30 bytes): 22-byte name, then big-endian u16 length (words), u8 finetune, u8 volume
(0..64), u16 repeat start (words), u16 repeat length (words). The header of instrument `n` is at
`0x14 + 30 (n - 1)`; its length word is at `0x2A + 30 (n - 1)`.

`npat` = 1 + the largest of the 128 order-list entries (all 128 are scanned, not only the song
length; the comparison is signed, so entries 0x80..0xFF never count).

Sample `n` starts at `0x43C + 1024 npat + 2 * (sum of the length words of instruments 1..n-1)`.
A sample with length 0 takes no space (it has the same start as the next one).

Pattern = 64 rows x 4 channels x 4 bytes. Cell bytes `b0 b1 b2 b3`:

- instrument = `(b0 & 0xF0) | (b2 >> 4)` (0 = none);
- note period = `((b0 << 8) | b1) & 0x0FFF` (0 = none);
- command = `b2 & 0x0F`, argument = `b3`.

An instrument is an AM instrument when its companion record (1.3) starts with `AM`; otherwise it is
a sample instrument. Modules made with the program give AM instruments a sample header of length 0
and a non-zero volume; the volume still scales the AM output (7.2).

### 1.2 Detection

Both must hold:

- the 4 bytes at 0x438 are `EXO4` or `FLT4`;
- among 32 header slots `i = 0..31` at `0x2A + 30 i` (the length word of instrument `i + 1`), at least
  one has a zero length word and a non-zero byte at slot + 3 (the volume). Slot 31 lies past the 31
  real headers: it reads order-list bytes 20, 21 (as the length word) and 23 (as the volume).

`FLT4` is also a StarTrekker tag: an ordinary StarTrekker module with an empty, non-silent sample
header is accepted too. The reference refuses to start without a companion file (1.3), so the
companion's existence is effectively part of detection.

### 1.3 Companion file

The companion's name is the module's file name with `.AS` appended (the reference tries `.NT` when
that fails; the host's lookup ignores case). Without either, the reference reports "could not load
additional files" and does not play.

Layout (big-endian):

| offset | size | content |
|---|---|---|
| 0x000 | 24 | header, not read by the player (`AudioSculpture10`, `ST1.3 ModuleINFO` in the wild; the synthetic files use the first) |
| 0x018 + 0x78 n | 0x78 | record of instrument `n` (n = 1..31; the slot n = 0 is never read) |
| 0xFA0 + 0x110 k | 0x110 | user-code slot `k` (k = 0..15), see 1.4 |

AM record (only the first 0x24 bytes are used; all words are 16-bit, signed unless noted):

| offset | name | meaning |
|---|---|---|
| 0x00 | magic | `AM` (0x41 0x4D) marks an AM instrument |
| 0x06 | start | envelope level at note start (also the initial volume, 5.2) |
| 0x08 | target1 | attack target level |
| 0x0A | rate1 | attack step per tick |
| 0x0C | target2 | decay target level |
| 0x0E | rate2 | decay step |
| 0x10 | sustain | sustain level |
| 0x12 | rate3 | step towards sustain |
| 0x14 | hold | sustain time: the hold stage lasts `hold + 1` ticks |
| 0x18 | release | release step per tick |
| 0x1A | wave | waveform 0..3 (unsigned): sine, ramp, square, noise (section 8) |
| 0x1C | fall | added to the period every tick (pitch slide) |
| 0x1E | vibdepth | vibrato depth (0 = no vibrato) |
| 0x20 | vibspeed | vibrato speed |
| 0x22 | octave | the note period is shifted left by this many bits at note start (unsigned; only the low 6 bits count; 16..63 give period 0) |

Levels run 0..256 for volume 0..64 (the volume uses `level >> 2`).

### 1.4 User-code slots (command E)

Command E with argument `xy` selects slot `x`. A slot whose first word is 0xABCD is empty and
command E then does nothing. A non-empty slot holds 68000 machine code that the original calls on the
row tick and on every following tick of that row (with `y` as a parameter and four player services:
voice DMA off, voice DMA on, note index to period, period to note index). An open replayer cannot run
it; see README. Everything else in this spec assumes empty slots (all validated modules have only
empty slots). The minimum companion size that keeps all slot reads in the file is 0x20A0 bytes.

### 1.5 Title, author, subsongs, length

- Title: module bytes 0..19 (up to the first NUL). No author field.
- Subsongs: scan the order positions `p = 0 .. songlen-1`. For each, look at the pattern
  `order[p]` cell by cell (row 0 channel 0, row 0 channel 1, ... row 63 channel 3) for the first cell
  with command B. Positions whose pattern has none are skipped. For the first B found remember its
  argument `a0`. Every found B whose argument differs from `a0`, and the first one, adds a subsong
  that starts at position `p + 1`. If a later found B has the argument `a0` again, the scan stops and
  the subsong count is set to 1 (whatever was found before). With no B at all the count is 1.
  Subsong 1 always starts at position 0; subsong `k >= 2` starts at the start position recorded by
  the `(k-1)`-th B. So `count` B commands give `count` subsongs, and the start recorded by the last B
  is never used. The host's subsong index 0 is subsong 1; indexes past the count play the last one.
- No length field. The song end is detected while playing (6.1).

## 2. State

Global: `speed` (ticks per row), `counter`, `row` (0..63), `pos` (order position), `break` flag,
`dmamask` (DMA bits of the current row), the visited table (one bit per order position and row; 128
positions x 64 rows; zero at load and never cleared), the noise buffer (32 bytes, zero at load).

Per channel (channel bit = 1, 2, 4, 8): `cell` (copy of the 4 cell bytes of the current row; can be
modified, 7.4/6.3), `start`, `length`, `loopstart`, `replen` (repeat length), `period`, `outper`
(period last written, base of the AM vibrato), `volume` (0..64), toneport `target`, `speed`, `dir`,
vibrato `cmd` (byte) and `pos` (byte), `am` flag, `inst` (last instrument number), envelope `level`,
`stage` (0..5), `hold` counter, AM vibrato `phase`. All zero at load. Channel state is not reset by
the song init.

## 3. Init (song start)

In this order:

1. Write 0 to AUD0VOL..AUD3VOL, then DMACON = 0x000F.
2. For instrument 1..31: compute the sample start (1.1), store 32-bit zero over the first 4 bytes at
   that address (this changes sample data, also of the following sample for length-0 headers and past
   the file end for trailing ones), clear the finetune byte of the header (finetune is never used).
3. Write the CIA-A port A byte with bit 1 set (LED/filter off; logged as `bfe001 0002`).
4. `speed` = 6, `counter` = 0, `row` = 0, `pos` = start of the selected subsong, `break` = 0.
5. The tick timer is set to 125 BPM: ticks are 70 940 cc apart.

Selecting a subsong other than the first makes the host stop and re-init the player, so its log
repeats steps 1 to 3 (twice for the third subsong).

## 4. Tick

Every tick: `counter += 1`. If `counter < speed`, it is an effect tick (section 7); otherwise
`counter = 0` and it is a row tick (section 6). After either, the AM pass (section 8.1) and the
noise generator (8.3) run. The first tick after init is therefore an effect tick, and the first row
is played on the 6th tick.

## 5. Constant data

### 5.1 Tables

Period table (36 entries, used by arpeggio):
856 808 762 720 678 640 604 570 538 508 480 453 428 404 381 360 339 320 302 285 269 254 240 226
214 202 190 180 170 160 151 143 135 127 120 113.

Command-4 vibrato sine (32 bytes): 0 24 49 74 97 120 141 161 180 197 212 224 235 244 250 253 255 253
250 244 235 224 212 197 180 161 141 120 97 74 49 24.

AM vibrato table: 180 signed 16-bit words, a half sine of amplitude 128 (entry 0 = 0, entry 90 =
128), not an exact formula; the bytes are in `data/am_vibrato.bin` (big-endian words).

### 5.2 Waveforms

Four 32-byte waveforms, contiguous in this order (an AM instrument's LC is the first byte of its
waveform, i.e. base + 32 x wave; the reference's base is player memory, logged as a non-module
address):

- 0 sine: byte `i` = `round(127 sin(2 pi i / 32))` (two's complement);
- 1 ramp: byte `i` = `0x80 + 8 i` (mod 256), i.e. -128, -120, ... 120;
- 2 square: 16 bytes 0x80 then 16 bytes 0x7F;
- 3 noise: the noise buffer, rewritten every tick (8.3).

A wave value above 3 points past these 128 bytes into the AM vibrato table; not supported.

## 6. Row tick

### 6.1 Song end and row fetch

If the visited bit of (`pos`, `row`) is already set, the song has ended: the player notifies the
host and continues playing normally. Set the bit. The reference render stops a few ticks later.

`dmamask` = 0. For channel 0..3, copy the cell of `row` in pattern `order[pos]` into the channel and
run 6.2 for it. Then:

1. wait (9.3); write DMACON = `dmamask | 0x8000` (always, also when `dmamask` is 0);
2. wait (9.3); then for channel 0..3: write AUDxLC = `loopstart` (32-bit), AUDxLEN = `replen`, and
   for sample (non-AM) channels AUDxVOL = `volume`. Exception, channel 3: an AM channel 3 gets only
   the LC write (no LEN); a sample channel 3 gets LC, LEN, VOL.
3. `row += 1`. If `row` = 64: `row` = 0 and advance the position. Otherwise, if `break` is set:
   `row` = 0 and advance the position.

Advance the position: `break` = 0, `pos = (pos + 1) & 0x7F`; if `pos` = song length, `pos` =
restart position. (Position B sets `pos` to its argument minus 1, so the advance lands on the
argument; argument 0 lands on 0. A position at or past the song length that is not equal to it is
played as it is.)

### 6.2 Channel note processing

1. If the cell has an instrument `n`:
   - `inst` = n; look up the companion record of `n`; `am` = record starts with `AM`;
   - `start` = sample start of `n`; `length` = header length; `volume` = header volume;
   - if the header repeat start `r` is non-zero: `loopstart` = `start + 2r`, `length` = `r` +
     header repeat length (16-bit); else `loopstart` = `start`;
   - `replen` = header repeat length;
   - write AUDxVOL = `((g * volume) & 0xFFFF) >> 6`, where `g` = 64 for a sample instrument and
     `(record.start as unsigned) >> 2` for an AM one.

   (`am` stays as it was when the cell has no instrument. An AM instrument without a note switches
   the channel's envelope to the new record but keeps level and stage.)
2. If the cell has no note period: row commands (6.3), done.
3. Sample channel (`am` clear) with a note:
   - if `length` = 0: write DMACON = channel bit (voice off), row commands, done;
   - if the command is 5 or 3 (tone portamento): `target` = note period, `dir` = 0. If the note
     period equals `period`: `target` = 0 and the channel is done WITHOUT row commands. Else, if the
     note period is less than `period`: `dir` = 1. Row commands, done.
4. Note start (sample channels with another command, and AM channels with any command, also 3/5):
   - `period` = note period; write DMACON = channel bit; vibrato `pos` = 0;
   - AM channel: let `w` = waveform address (5.2). Write AUDxLC = `w`, AUDxLEN = 16;
     `loopstart` = `w`, `replen` = 16; `level` = record.start; `phase` = 0; `stage` = 1;
     `period` = `(period << octave) & 0xFFFF` (0 for octave 16..63; only the low 6 bits of the
     octave word count); `outper` = `period`; write AUDxPER = `period`.
   - sample channel: write AUDxLC = `start`, AUDxLEN = `length`, AUDxPER = `period`.
   - `dmamask |= channel bit`; row commands.

### 6.3 Row commands (on the row tick, after 6.2)

| cmd | action |
|---|---|
| E | user-code slot `arg >> 4`; nothing for an empty slot (1.4) |
| D | `break` = 1 (next position starts at row 0; there is no row argument) |
| B | `break` = 1, `pos` = `(arg - 1) & 0xFF` |
| C | if `arg > 64`: `arg` = 64 and the channel's cell copy argument becomes 64; `volume` = `arg`; write AUDxVOL = `arg` (also on AM channels) |
| F | `speed` = `arg`, limited to 31; 0 gives 1. No tempo (BPM) command. |
| others | nothing on the row tick |

## 7. Effect tick (counter 1 .. speed-1)

For channel 0..3, by the channel's cell copy (command `c`, argument `a`):

| case | action |
|---|---|
| `c` = 0 and `a` = 0 | write AUDxPER = `period`; `outper` = `period` |
| `c` = 0, `a` != 0 | arpeggio (7.1) |
| 1 | `period -= a`; if the 16-bit result minus 0x71 is negative (signed), `period` = 0x71; write PER; `outper` = `period` |
| 2 | `period += a`; if the result minus 0x358 is not negative (signed), `period` = 0x358; write PER; `outper` |
| E | user code if a slot was ever called (never, with empty slots); no write |
| 3 | if `a` != 0: toneport `speed` = `a` and the cell copy argument becomes 0; tone portamento step (7.2) |
| 4 | if `a` != 0: vibrato `cmd` = `a` (the whole byte); vibrato step (7.3) |
| 5 | tone portamento step, then volume slide (7.4) |
| 6 | vibrato step, then volume slide |
| 7, 8, 9, A, B, C, D, F | sample channel: write AUDxPER = `period` (no `outper` update); then for A: volume slide |

### 7.1 Arpeggio

`k` = `counter mod 3` (counter is 1..30). `k` = 0: write PER = `period`, `outper` = `period`.
Otherwise `x` = `a >> 4` (k = 1) or `a & 15` (k = 2). Search the period table for the first entry
`i` (0..35) with `(period & 0xFFF) >= table[i]`; if none, `i` = 36. Write PER = `E[i + x]`, `outper`
= that value, where `E` is the period table followed by: [36] 0, [37] `speed * 256 + counter`,
[38] `row * 16`, [39] `pos * 256 + break` (break is 0 on effect ticks), [40] `dmamask` of the last
row (without 0x8000), then the 32-bit sample start addresses of instruments 1, 2, 3, ... as two words
each (high, low): [41 + 2(n-1)] and [42 + 2(n-1)]. These addresses are absolute: the reference uses
the module's load address (the `# module` line of a log); a port must use the same convention as for
its LC values. The table snaps: `x` = 0 at k = 1/2 writes `table[i]`, not `period`. A channel that
never played a note (period 0) reads `E[36 + x]`.

### 7.2 Tone portamento step

If `target` = 0: nothing (no write). Otherwise, `dir` = 0: `period += speed`; if not
(`target > period`, signed): `period` = `target`, `target` = 0. `dir` = 1: `period -= speed`; if
not (`target < period`): `period` = `target`, `target` = 0. Write PER = `period`; `outper` =
`period`.

### 7.3 Vibrato step

`d` = `(sine[(pos >> 2) & 31] * (cmd & 15)) >> 7`. Value = `period - d` if bit 7 of `pos` is set,
else `period + d`. Write PER = value (16-bit); `outper` = value. Then `pos += (cmd >> 2) & 0x3C`
(byte, wraps). `period` is unchanged.

### 7.4 Volume slide

The argument is the cell copy's. If `a >> 4` != 0: `volume += a >> 4`, then if `volume` >= 64:
`volume` = 64. Else `volume -= a & 15`, below 0 becomes 0. Sample channels: write AUDxVOL =
`volume`. AM channels: no write (the new volume scales the AM output from 8.1 on).

## 8. AM pass, noise

### 8.1 AM pass (every tick, after the row or effect processing)

For channel 0..3 with `am` set and `stage` != 0 (`rec` = companion record of `inst`):

1. Envelope (signed 16-bit compares):
   - stages 1, 2, 3 move `level` towards (target1, rate1), (target2, rate2), (sustain, rate3). If
     `level` = target: go to the next stage at once. If `level` > target: `level -= rate`; if now
     `level <= target`: `level` = target and next stage. If `level` < target: `level += rate`; if now
     `level >= target`: `level` = target and next stage. Entering stage 4 sets `hold` = rec.hold.
   - stage 4: `hold -= 1`; if negative: stage 5.
   - stage 5: `level -= release`; if negative: `am` = 0, `level` = 0, `stage` = 0, write DMACON =
     channel bit (voice off). (Level exactly 0 stays one more tick.)
2. Write AUDxVOL = `(((level as unsigned) >> 2) * volume & 0xFFFF) >> 6`.
3. `outper += rec.fall`, `period += rec.fall` (16-bit).
4. Vibrato offset `v`: 0 if rec.vibdepth = 0; else with `p` = `phase` (signed): if `p >= 360`,
   `p -= 360` and negate at the end; `v` = `(vibdepth * amvib[p / 2])` truncated to 16 bits as
   signed, arithmetic shift right 7; negated if flagged.
5. Write AUDxPER = `outper + v`.
6. `phase += 2 * rec.vibspeed`; if then `phase >= 720` (signed): `phase -= 720` (once).

So an AM channel gets, per tick, PER from its command (7) and then VOL and PER from this pass; the
row/effect PER writes are only visible for a few hundred colour clocks.

### 8.2 What the AM pass does not do

It does not touch LC/LEN; the 16-word waveform loops through the row tick's loop writes. Commands
1/2/3/4/arpeggio change `period`/`outper` of AM channels like sample channels; command C and volume
slides change `volume`, which scales step 2.

### 8.3 Noise generator (every tick, after the AM pass)

`n` = 0x7327 (16-bit). For byte `k` = 0..31: store `n & 0xFF` as noise byte `k`; read the beam's
horizontal position `h` (9.4); low byte of `n` += `h` (mod 256, the high byte unchanged); `n ^= 0x7C`;
rotate `n` left by 3 bits (16-bit rotate). Byte `k` is stored right before the `k`-th read (9.4 gives
the clocks), so a voice playing waveform 3 can fetch a mix of old and new bytes.

## 9. Timing

### 9.1 Clock, entries, init

All costs are in cycles. Tick `t` (t = 0, 1, ...) enters at colour clock `E0 + 70 940 t`; inside a
tick, keep a cycle counter `A` that starts at `2 * entry` and add each step's cost in the order
given. A register write is a step; it is logged at colour clock `floor(A / 2)` taken after adding
its own cost (a 32-bit LC write logs both words at the same clock).

Init (section 3): the four volume writes are 10 cc apart and DMACON 10 cc after the last; the LED
write follows `3676 + 2 m` cc after the first volume write, where `m` is the number of times an
order-list entry (all 128 in order, signed compare) is larger than every earlier one, starting from
a maximum of 0. `E0` = LED write + 71 281 cc. (When the host re-inits for a subsong, the last LED write
counts.) The absolute clock of the first init write is a host property (31 070 to 370 195 cc in the
logs).

### 9.2 Tick frame

| step | cycles |
|---|---|
| entry, counter step and test | 267 |
| effect tick: start | 18; then per channel 38 + channel steps (9.6); then 53 |
| row tick: start | 30; then 9.5 |
| AM pass and noise | 9.7 |
| return | 148 |

### 9.3 Raster wait (twice per row tick)

Lines are 227 cc long and start at multiples of 227 cc of the log clock. A beam sample is a 16-cycle
step whose value is the line number `floor(floor(A / 2) / 227)` at the end of the step. The wait
costs: 26; then five times: one sample (16) giving `L0`; then repeatedly one sample (16) and 9, until
the sample differs from `L0`; then 10. After the five line changes: 114.

### 9.4 Noise beam reads

Each noise byte (8.3) costs 8 (store), then a 16-cycle beam sample whose value `h` =
`floor(A / 2) mod 227` at the end of the step, then 30.

### 9.5 Row tick steps

| step | cycles |
|---|---|
| head (song-end test included) | 351 |
| song end detected (host notification) | +278 |
| | 120 |
| per channel: channel 0 / channels 1..3 | 38 / 46, then 9.5.1 |
| raster wait, 20, DMACON write 16, raster wait, 20 | |
| loop writes: per channel (channels 1..3 first add 8) | channels 0..2: LC 28, LEN 20, 21, VOL 20 (sample channels only). Channel 3: LC 28, 21, then for a sample channel LEN 20, VOL 20 |
| | 53 |
| row was 63 | 20, then advance |
| other rows | 27; if `break`: 30, then advance |
| advance the position | 113, + 24 if `pos` wrapped to the restart position, + 27 |

#### 9.5.1 Channel note processing (6.2)

| step | cycles |
|---|---|
| always | 87 |
| instrument present | 177; +38 for an AM record; 203; +98 if repeat start != 0, else +40; 110; VOL write 12 |
| always | 27 |
| no note | row commands (9.5.2) |
| note | 21 |
| sample channel | 23; if `length` = 0: DMACON write 24, 10, row commands |
| | 62; command 5 -> tone portamento setup; else 19; command 3 -> tone portamento setup |
| tone portamento setup | 69; equal period: 32, end (no row commands); else 11; +26 if the note is below `period`; row commands |
| note start | 36, DMACON write 24, 37 |
| AM channel | 172, LC write 12, LEN write 16, 146, PER write 12, 30 |
| sample channel | LC write 24, LEN write 20, PER write 20 |
| | 42, row commands |

#### 9.5.2 Row commands

| step | cycles |
|---|---|
| always | 37 |
| E | 271, then 116 (empty slot) |
| not E | 19; D: 36 |
| not D | 19; B: 68 |
| not B | 19; C: 25, +16 if the argument was clamped, 28, VOL write 12, 16 |
| not C | 19; F: 33, +4 if argument > 31, 13, +4 if it was 0, 32 |
| none of these | 16 |

### 9.6 Effect tick channel steps (section 7)

| step | cycles |
|---|---|
| always | 29 |
| command 0, argument 0 | PER write 20, 36 |
| otherwise | 33 |
| arpeggio | 39; k = 0: PER write 20, 36. k = 1: 57; k = 2: 41; then 40, 27 per table entry before `i`, 17, PER write 22, 38 |
| not 0 | 19; command 1: 61, +16 if clamped, PER write 20, 36 |
| not 1 | 19; command 2: 61, +16 if clamped, PER write 20, 36 |
| not 2 | 17; command E: 41 |
| not E | 19; command 3: 21, +28 if the argument != 0, tone portamento step |
| not 3 | 19; command 4: 21, +12 if the argument != 0, vibrato step |
| not 4 | 19; command 5: 18, tone portamento step, volume slide |
| not 5 | 19; command 6: 18, vibrato step, 10, volume slide |
| not 6 | 21; sample channel: PER write 20; then 19; A: volume slide; other: 16 |
| tone portamento step | 21; target 0: 16, end. Else 37, 49, +36 if the target is reached, PER write 20, 36 |
| vibrato step | 175; +4 if bit 7 of `pos` is set, else +14; PER write 12; 74 |
| volume slide | 39; up: 41 (+16 if clamped); down: 45 (+16 if below 0); then 21; sample channel: 16, VOL write 12; then 16 |

### 9.7 AM pass and noise

| step | cycles |
|---|---|
| start | 28 |
| per channel | 23; `am` clear: 26, next channel |
| | 135; `stage` 0: 26, next channel |
| stage dispatch | 25 x min(stage, 4) |
| stages 1..3 | 33; level = target: 26 (+20 when entering stage 4); else 21, 52 + (11 in stages 1/2, 9 in stage 3), and when the target is reached: 46, +10 if the level was rising, +20 when entering stage 4 |
| stage 4 | 25; +26 if it ends |
| stage 5 | 37; if the release ends: 40, DMACON write 24 |
| volume | 106, VOL write 12 |
| pitch | 65; vibrato depth != 0: 33, +12 if `phase` >= 360, 109, +4 if negated |
| | 12, PER write 12, 57, +20 if `phase` wrapped, 26 |
| noise | 24; 32 x (8, beam sample 16, 30); 16 |

## 10. Write order summary

- Init: VOL0..3 = 0, DMACON 0x000F, LED byte.
- Effect tick: per channel 0..3 the command's PER/VOL writes (7); then per AM channel (DMACON off at
  release end), VOL, PER.
- Row tick: per channel 0..3: [VOL (instrument)] [DMACON off] [LC LEN PER] [VOL (command C)];
  DMACON on; loop writes for channel 0..3; then the AM pass writes.
