# Richard Joseph (`RJ.*`, Eagleplayer "Richard Joseph") replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the "Richard Joseph" game-music container played by the UADE/Eagleplayer player
"Richard Joseph" (file prefix `RJ.`, made by Wanted Team), with the two variants of Richard
Joseph's game sound driver that were available for study: variant **C** (from the game Cadaver)
and variant **S** (from Speedball 2). Not covered: the unrelated "Richard Joseph Player" format
(`rjp.*` + `smp.*`, its own player), and Richard Joseph's ProTracker-style `MOD.*` files.

This is a behaviour description derived from register-write logs and execution traces of the 68k
reference and from reading the driver. It contains no code of the original. Cycle numbers are the
68000 cycle counts the reference machine charges (2 cycles = 1 colour clock, "cc").

**No real `RJ.*` module exists in the archive.** The three test modules are synthetic: RJ
containers built around the music driver and data of two DeliTracker "custom" rips (README,
LOGS.md). Everything below is verified on them only.

Status (checked by a private model that follows this text, against the reference):

| item | result |
|---|---|
| 3 container files, 10 runs (subsongs, a synthetic edit per variant), about 130 000 writes | address, value, order and absolute colour clock of every write equal the reference |
| host timeline (start-up, subsong stepping, tick grid) | computed by the model itself, equal to the reference |
| paths never executed | README |

Times are absolute: the model reproduces the reference clock of every write, not only
tick-relative times, because the driver waits on the raster beam (8.6).

## 1. File format

### 1.1 Container

An `RJ` file is an AmigaDOS load file (first long `0x000003F3`) with exactly one hunk, a code or
data hunk whose size long has the chip-memory flag (byte 20 of the file is non-zero). The hunk
holds one image: a 68-byte header followed by the game's sound driver, its data and its samples,
with a standard relocation table for every absolute pointer of the driver and of the header.
The image is loaded wherever the host's segment loader puts it (its address is called `base`
below); all pointers in it are made absolute by the relocation.

Header (offsets in the image, big-endian):

| off | size | meaning |
|---|---|---|
| 0 | 4 | `0x70FF4E75` (a "return -1" stub; the container is not meant to be run) |
| 4 | 8 | ASCII `R.JOSEPH` |
| 12 | 4 | Play: address of the driver's per-tick routine (relocated) |
| 16 | 4 | Audio: address of the driver's audio-interrupt routine (relocated; not called by the adapter, which emulates it, 5.4) |
| 20 | 4 | InitPlayer: address of the driver's hardware/voice initialisation (relocated) |
| 24 | 4 | InitSong: address of a start routine that starts subsong `n` (relocated; 5.3) |
| 28 | 4 | SampleInfo: address of the first instrument record (relocated) |
| 32 | 4 | EndSampleInfo: address after the last instrument record (relocated) |
| 36..60 | 7 x 4 | informational values handed to the host's module-info list (text pointers or numbers; zero in the test files except offset 48, the size of the original rip) |
| 64 | 4 | number of subsongs (non-zero) |

The longs at 12..32 are listed in the relocation table; the adapter uses them as absolute addresses.

### 1.2 Detection

All of these hold: first long `0x000003F3`; byte 20 of the file non-zero; at file offset 32 (the
first byte of the hunk data in a one-hunk file without names) the long `0x70FF4E75`, then the
longs `0x522E4A4F` and `0x53455048` (`R.JOSEPH`); the four longs Play, Audio, InitPlayer, InitSong
(file offsets 44, 48, 52, 56) are all non-zero.

The prefix `RJ.` also starts names of files of other formats in the archives (the `rjp.`/`smp.`
pairs of the Richard Joseph Player); those fail the header test.

### 1.3 Song information

- Subsongs: `1 .. count` (header offset 64). The host's subsong index `k` (0-based) is started as
  `n = k + 1`.
- Samples: `(EndSampleInfo - SampleInfo) / 14`.
- No title or author in the format; the length is unknown (the song end, 7, is the only event).

## 2. The driver data (layout parameters)

The image embeds the game's driver unchanged, so an open replayer must locate the driver's
tables in the image. The parameters below are byte offsets in the image (add `base` for an
address). They were found by reading the drivers; for the three test files they are:

| parameter | meaning | C (Cadaver files) | S (Speedball 2 file) |
|---|---|---|---|
| `tempo` | 16-bit word: ticks per duration unit | 0x0048 | 0x178C |
| `voices` | four voice records, stride `rec` | 0x004A, rec 0x92 | 0x12D4, rec 0x94 |
| `stack0..3` | top of each voice's call/loop stack (grows down) | 0x0312, 0x0392, 0x0412, 0x0492 | 0x15A4, 0x1624, 0x16A4, 0x12CE |
| `pattab` | pattern table (C: 32-bit absolute pointers; S: see `rbase`) | 0x0D48 | via `rbase` |
| `instab` | instrument records, 14 bytes each (= SampleInfo) | 0x40D62 | via `rbase` |
| `rbase` | S only: data block whose first long is the offset (from `rbase`) of the pattern table and whose second long is the offset of the instrument table; pattern-table entries and sample starts are offsets from `rbase` too | - | 0x17C6 |
| `instab0` | record read by the voice start (6.2) | = `instab` | 0x17A8 |
| `kill` | address written to the location register when a note is cut | = `instab` | 0x17A8 |
| `silent` | address written to the location register at init and voice start | 0x0D70 | 0x17B6 |
| `modtab` | modulation sets, 15 bytes each | 0x0938 | 0x03F2 |
| `envtab` | envelopes, 8 bytes each | 0x0966 | 0x054C (room for two) |
| `ptabs` | S only: the three period tables selected by an instrument | - | 0x0F0E, 0x0F6E, 0x0FCE |
| `prio` | S only: four priority bytes (one per voice) at `prio + 2 + v` | - | 0x178E |
| `songtab` | data of the start routine (5.3) | 0x08F8 | 0x032A |
| `clear` | number of longs the driver init clears | 0x112 | 0x114 |

How they can be found in another module of the same family: the tempo word is the first word of
the driver's variable block and the four voice records follow it immediately (C) or start at the
block of the second hunk of the rip (S); each voice record is addressed with a constant stride
(0x92 / 0x94) by every per-voice loop of the play routine. The pattern table, instrument table,
modulation-set table and envelope table are the base addresses that the command handlers 0xB0 /
0xD4, 0xD0, 0x9C and 0xC8 (5.6) add the scaled operand to. `kill` is the constant loaded as the
sample address when a note is cut; `silent` the one used by the init. The stacks are the four
values loaded into the voice records' stack field by the init. Expect manual work: no third
driver was available to test the description.

Variant differences (beyond the layout): S stores pattern and sample addresses as offsets from
`rbase`, chooses one of three fixed period tables per instrument (6.4), keeps a priority byte per
voice, writes a CIA register at init (6.1), and differs in a few costs (section 8).

## 3. Data structures

### 3.1 Instrument record (14 bytes)

| off | size | meaning |
|---|---|---|
| 0 | u16 | loop flag: bit 15 set (negative) = the sample loops |
| 2 | u16 | loop offset in bytes from the sample start (used when looping; 0 is special, 6.3/6.6) |
| 4 | u16 | length in words |
| 6 | u32 | C: sample start (absolute, relocated). S: offset of the sample from `rbase` |
| 10 | u32 | C: period table address (absolute). S: period-table selector: low byte 0 = table 0, 1 = table 1, other = table 2 |

### 3.2 Period tables

16-bit periods, four entries per semitone (fine steps), read at `table + 2 * index` with a
signed 16-bit index (6.4); a module may read far into the table (S notes 0x30..0x43 are common).
The three S tables are 0x60 bytes apart and overlap: they are one long array read from three
starting points. C instruments carry their own table address.

### 3.3 Patterns

Byte streams. A byte below 0x80 is a note (6.4). A byte `0x80 + 4j` (j = 0..21) is a command
(5.6); other values above 0x7F are invalid (the reference would jump through garbage).

### 3.4 Modulation set (15 bytes)

Fifteen signed bytes, each sign-extended to a 16-bit word when a set is selected (command 0x9C).
In order: volume counts (stages 0, 1); pitch counts (stages 0, 1, 2); volume steps (0, 1); pitch
steps (0, 1, 2); volume delay words (0, 1); pitch delay words (0, 1, 2). Volume modulation has two
stages, pitch modulation three (6.5).

### 3.5 Envelope (8 bytes, unsigned unless noted)

| byte | meaning |
|---|---|
| 0 | attack step (added to the volume) |
| 1 | attack delay (ticks between steps) |
| 2 | decay step (subtracted) |
| 3 | decay delay |
| 4 | sustain level |
| 5 | release point (signed; compared with the low byte of the remaining duration) |
| 6 | release step (subtracted) |
| 7 | release delay |

### 3.6 Voice state

Per voice: hardware channel `v` (registers `0xDFF0A0 + 16v`), DMA bits `dmaclr = 1 << v`,
`dmaset = 0x8000 | dmaclr`, interrupt bits `ibit = 0x80 << v`, `iset = 0x8000 | ibit`;
`E` (a counter used by the interrupt emulation); `ptab`, `env` (envelope address or 0), `stack`;
`smp`, `loopflag`, `loopoff`, `len` (from the instrument); `note`, `vol`, `trig` (trigger flag),
`ecnt` (envelope delay counter, byte), `eact` (envelope active), `eph` (envelope phase); `pstart`
(pattern start), `ptr` (pattern pointer, 0 = voice idle), `dur` (ticks left), `durset` (duration
of the next event), `basevol`, `baseper`, `outper`; the modulation words of 3.4 plus the current
volume delays/counts (2 each) and pitch delays/counts (3 each); `voff`, `poff` (current volume
and pitch offsets), `mflags` (bit 0 = volume modulation loops, bit 1 = pitch modulation loops),
`transp` (transpose, signed word). All 16-bit unless noted.

The voice records live in the image; the adapter's start-up clears every field except the
channel constants (`E` is set to 2 by the driver init, 6.1). `tempo`, the S song table and the S
priority bytes are module memory too: the player changes them in place and they keep their
values across a subsong change.

## 4. Song start data

The start routine (InitSong) is module code. In the test files it is one of three shapes:

- C, title tune (one subsong): starts voices 0..3 with patterns 1, 2, 4, 7.
- C, table: `songtab + 4n` holds four pattern numbers `p0..p3`; voice 0, 1, ... is started with
  `p0, p1, ...`, and the walk stops after the first zero (a voice given pattern 0 is still
  started; later voices are not touched).
- S: entry `songtab + 6n` = priority byte, flag byte, four pattern numbers. The start routine
  writes the priority 15 into the first byte; if the flag byte has bit 7 set nothing is started;
  else (the priority check against `prio` passes in all test runs) each voice `v` whose pattern
  number is non-zero gets `prio[v] = 15` and is started with that pattern; voices with 0 are
  left alone.

## 5. The adapter (reference host behaviours that appear in the output)

The Eagleplayer adapter loads the container, runs the driver, and emulates the hardware audio
interrupt in software. Its behaviours are part of the output.

### 5.1 Patched writes

At start-up the adapter rewrites a few places in the driver so that its writes to the interrupt
enable (INTENA) and request (INTREQ) registers go to two 16-bit software words `IE` and `IR`
instead of the hardware (each such write *replaces* the word; it does not set or clear bits),
and so that specific writes are followed by a call of the adapter's Audio emulation (5.4). These
patched writes are named below where they occur. INTENA/INTREQ themselves are never written.

### 5.2 Volume stage

The driver's volume writes go through the adapter: `AUDxVOL = (((vol & 0x7F) * gain) & 0xFFFF) >> 6`,
gain 64 for both sides by default (identity). Channels 0 and 3 use the left gain, 1 and 2 the
right.

### 5.3 InitSound (start of subsong `n`)

1. Clear the adapter's per-song state; song-end flags `F[0..3] = 1`.
2. Run the driver init (6.1).
3. Run the start routine with `n` (4).
4. Scan the start routine's code from its first 16-bit word, word by word, until the first word
   that equals `0x4E75` or `0x41F9`. On `0x41F9`, the long that follows it is a table address;
   for `i = 0..3` the byte at `table + 4n + i` is read and if it is zero `F[i] = 0` (a voice that
   does not take part counts as finished). On `0x4E75` nothing more. (In the C table file the
   table is `songtab`; in the other two files the scan meets `0x4E75` first.)
5. Copy the flags to the restore copy `R = F`.

### 5.4 Audio emulation and DMA wait

The driver's audio interrupt (one per sample loop at the hardware level) is emulated: whenever
the driver has started a sample (the patched sites), the adapter waits nine raster lines (8.6)
and then, for channel `k = 0..3` in order, if `IR & IE` has bit `7 + k`, runs **Main(k)**:

- `E = E - 1` (16-bit).
- If the voice's loop flag is negative: AUDxLC = `smp + loopoff` (32-bit), AUDxLEN = `len -
  (loopoff >> 1)` (16-bit), `IE = ibit`.
- Else, if `E == 0`: `IE = ibit`, DMACON = `dmaclr` (the one-shot sample is switched off).
- Else: AUDxLC = `base` (the image start; the adapter clears the image's first long at
  start-up, so this is two zero bytes of chip memory), AUDxLEN = 1.
- Then `IR = ibit`.

After the four channels: `IR = 0`.

### 5.5 Song-end test

Commands 0x88 (restart) and 0xAC (stop) of voice `v` are patched to run this test: `F[v] = 0`;
if all four `F` are zero, the host's song-end notification is called and `F = R`. The driver's
own action of the command is kept (6.3).

### 5.6 EndSound

DMACON = `0x000F`, then AUDxVOL = 0 for channels 0..3.

## 6. The driver

### 6.1 Driver init (InitPlayer of the header, called by InitSound)

Writes, in order: (S only: CIA-A port A, address `0xBFE001`, = `0x02`, the LED/filter bit;
this write appears in the logs), DMACON = `0x000F` (patched site), `IE = IR = 0x0780` (patched),
ADKCON = `0x00FF`. It clears `clear` longs of the variable block (all voice state), sets the
channel constants for each voice and sets `E = 2`, `smp = silent`, `len = 8` (every other field 0:
`ptr = 0`, `outper = 0`, `vol = 0`, `trig = 0`, no envelope), then writes AUDxVOL = 0 (channels 0..3),
AUDxLEN = 1 (0..3), AUDxPER = 1 (0..3), AUDxLC = `silent` (0..3, 32-bit), AUDxLEN = 8 (0..3),
AUDxPER = 0x100 (0..3), DMACON = `0x000F`. The voice stacks are set to `stack0..3`. `tempo` is
not touched.

### 6.2 Voice start (pattern `p` on voice `v`, called by the start routine)

1. Clear the voice's state (everything of 3.6 except the channel constants, `E` and `transp`).
2. `IE = ibit`, `IR = ibit` (patched), DMACON = `dmaclr`, AUDxVOL = 0, AUDxLC = `silent`,
   AUDxLEN = 1, AUDxPER = 1 (patched site: followed by) DMACON = `dmaset` and the Audio
   emulation (5.4).
3. `pstart = ptr =` address of pattern `p`; `stack = stackv`.
4. Instrument: C: the record at `instab0` (record 0). S: the record at `instab0` but with
   `smp = silent`, and `ptab` chosen by its selector (3.1).
5. `baseper = outper = 0x100`. AUDxLC = `smp`, AUDxLEN = `len`, AUDxPER = 0x100.
6. C only: DMACON = `dmaclr` (patched site, no Audio call).

### 6.3 The tick (Play)

Once per tick: for voice `v = 0..3`: if `ptr == 0` nothing; else if `dur == 0` the event fetch
(6.4) runs, and then (whatever `dur` was) the modulation step (6.5). After the four voices: the
envelope pass (6.6), then the writer pass (6.7).

### 6.4 Event fetch

At entry, if the voice's loop offset is 0 (current instrument), the **silence sequence**: DMACON
= `dmaclr`, `IE = ibit`, `IR = ibit`, AUDxVOL = 0, AUDxLC = `kill` (32-bit), AUDxLEN = 1,
AUDxPER = 1 (patched site: followed by) DMACON = `dmaset` and the Audio emulation.

Then bytes are read from `ptr` until a note, a rest (0x90) or a stop (0xAC):

- Note `b < 0x80`: `note = b`; **note cut**: DMACON = `dmaclr`, `IE = ibit`, `IR = ibit`,
  AUDxVOL = 0, AUDxLC = `kill`, AUDxLEN = 1, AUDxPER = 1 (these are plain writes, no Audio call).
  If `env != 0`: `eact` active (0xFF00), `eph = 0`, `vol = 0`, `ecnt = 0`. Period:
  `i = (b * 4 + transp)` (16-bit), `baseper = word at ptab + 2i` (index signed 16-bit). `trig = 1`.
  Volume and pitch modulation restart (6.5: delays := delay words, counts := count words);
  `voff = poff = 0`.
- Commands (operand bytes follow the command byte; "n" = unsigned operand):

| cmd | operand | action |
|---|---|---|
| 0x80 | n | `basevol = n` |
| 0x84, 0xA0, 0xA4 | n | nothing (operand skipped) |
| 0x88 | - | `ptr = pstart` (restart the pattern) and the song-end test of 5.5 |
| 0x8C | n | `durset = n * tempo` (16-bit) |
| 0x90 | - | rest: ends the fetch |
| 0x94 | n | `tempo = 750 / n` (integer; n = 0 is a divide-by-zero in the reference, not specified) |
| 0x98 | k, k bytes | `durset = (sum of the k bytes, 16-bit) * tempo`; k = 0 means 65 536 bytes |
| 0x9C | n | select modulation set n (`modtab + 15n`, 3.4): its 15 words replace the voice's modulation words; clear `mflags` bits 0 and 1, the current volume delays, the current pitch delays, `voff`, `poff` (the current counts are kept; the set's count and delay words are copied into them only at a note start or a loop restart) |
| 0xA8 | n | `mflags |= n` |
| 0xAC | - | stop: `pstart = 0`, `IE = ibit`, song-end test (5.5), `IR = ibit`, DMACON = `dmaclr`, `trig = 0`; S also clears `prio[v]`; ends the fetch with `ptr = 0` (the voice is idle from the next tick) |
| 0xB0 | n | call: push the pointer after the operand on the voice stack; `ptr` = pattern n |
| 0xB4 | - | return: pop the pointer |
| 0xB8 | n | `transp += signed n`; n = 0 sets `transp = 0` |
| 0xBC | n | `transp = signed n` |
| 0xC0 | n | loop start: push a frame (pointer after the operand, count n); n = 0 pushes (0, 0) |
| 0xC4 | - | loop end: if the frame's pointer is non-zero: `ptr` = it, count -= 1, and if the count reaches 0 the frame's pointer is cleared; if the pointer is zero the frame is popped and reading continues. The body runs n + 1 times (once for n = 0) |
| 0xC8 | n | `env = envtab + 8n` |
| 0xCC | - | `env = 0` |
| 0xD0 | n | instrument: record n (C: `instab + 14n`; S: `rbase + (long at rbase + 4) + 14n`, sample start `rbase +` field, `ptab` from the selector) |
| 0xD4 | n | jump: `ptr` = pattern n |

Pattern n is: C: the long at `pattab + 4n`; S: `rbase + long at (T + 4n)` with
`T = rbase + long at rbase`.

At the end of a note, rest or stop: `ptr` = position after the byte (0 for a stop), the stack is
kept, `dur = durset`.

### 6.5 Modulation step (every tick of an active voice, after the fetch if any)

`dur = dur - 1` (16-bit). Then:

- Volume (only when `env == 0`): look at stage 0 then stage 1: if the stage's current delay is
  non-zero, decrement it and stop looking; else if its count is non-zero, decrement the count,
  `voff += step` (16-bit), delay := the stage's delay word, stop looking; else go to the next
  stage. If neither stage did anything: `voff = 0`, and if `mflags` bit 0 is set the volume
  modulation restarts (delays := delay words, counts := count words).
- Pitch: the same over stages 0..2, with `poff += signed low byte of the step word`; no reset of
  `poff` when nothing happened; restart if `mflags` bit 1.
- If `env == 0`: `vol = (basevol + voff) & 0x3F`.
- `outper = baseper + poff`.

### 6.6 Envelope pass (after the voice loop, voices 0..3)

Skip the voice if `ptr == 0` or `env == 0`. If `ecnt != 0`: `ecnt -= 1`, and if it is still non-zero
skip. Otherwise one envelope step (`e` = the 8 envelope bytes, `lo` = low byte of `vol`):

- `eact == 0` (released): `eph = 0`; if `vol != 0`: `ecnt = e7`; `vol` = 0 if `lo < e6`, else
  `lo - e6` (high byte kept).
- phase 0 (attack): `ecnt = e1`; `x` = `vol` with low byte `(lo + e0) & 0xFF`; if `x >= basevol`
  (16-bit unsigned): phase 1 and `x = basevol`; `vol = x`.
- phase 1 (decay): `ecnt = e3`; `lo' = (lo - e2) & 0xFF`, 0 if `lo' >= 0x40`; if `lo' >= e4`:
  `vol` = old high byte with low byte `lo'`, done; else phase 2 and continue with sustain:
- phase 2 (sustain, also the fall-through above): `vol = e4`; if the signed low byte of `dur` is
  not greater than the signed `e5`: `eact = 0` (release from the next step).

### 6.7 Writer pass (voices 0..3)

AUDxPER = `outper`; AUDxVOL = `vol` through the volume stage (5.2). If `trig`: `trig = 0`,
AUDxLC = `smp` (32-bit), AUDxLEN = `len`, `E = 2`, DMACON = `dmaset`, `IR = ibit`; then
- loop flag clear: `IE = iset`, Audio emulation;
- loop flag set, loop offset 0: `IE = ibit`, Audio emulation;
- loop flag set, loop offset non-zero: `E = 1`, `IE = iset`, Audio emulation.

With `IR = ibit`, the Audio call always runs Main for this channel (5.4): a looped sample gets its
loop registers, a one-shot sample gets the two-byte silent loop (`E` 2 -> 1) and is switched off
at the next Main of that channel (`E` -> 0).

## 7. Song end

The song-end notification (5.5) is the only end event; the driver keeps playing. What the host
does then is host policy. The reference host used for the logs stops a WAV render at the next
audio-buffer boundary after it; when the subsong ended within its first 2 seconds without being
heard, it starts the next subsong instead: EndSound, the volume settings, InitSound with `n + 1`
at a clock it chooses (LOGS.md gives the observed value).

## 8. Timing

### 8.1 Clock model

All costs are 68000 cycles. A routine called at colour clock `t0` keeps a cycle counter `T`
from 0; a register write is stamped `t0 + floor(T / 2)` where `T` includes the cost of the step
that makes the write (the write is the last part of the step's cost). Both words of a 32-bit
location write (AUDxLC high, low) get the same stamp. Costs are static except the raster wait.

Re-basing: when the song-end notification is called (5.5), the clock re-anchors: from then on
`t0' = t0 + floor(T / 2)` and `T` restarts from 0.

In the tables, `+W` after a number means "this cost ends with the named write".

### 8.2 Host timeline

| event | colour clock |
|---|---|
| adapter check | 100 .. 232 |
| adapter InitPlayer starts | 232 |
| (allocation call) | 257 |
| module-change notification | `257 + floor((893 + 21 s) / 2)`, where `s` = number of 16-bit words from the start routine's first word up to and including the first word `0x287B` |
| InitPlayer returns | that + 48 |
| GetInfos, SubSongRange, SetVolume, SetVolume | + 12, + 18, + 118, + 118 |
| InitSound(1) | starts then |
| subsong stepping, for host subsong k > 0 | at the end of InitSound(m), m < k + 1: EndSound starts at once, then + 236 (two volume settings), then InitSound(m + 1) |
| ticks | grid `G + j * 70940`, `G` = end of the last start-up InitSound + 70940; a tick starts at `max(grid time, end of the host call that was running)` |

The grid is kept after a song-end restart (LOGS.md: the overdue tick runs as soon as the
restart's InitSound returns, the next ones on the grid).

Observed scan lengths `s`: C title file 132 393, C table file 42, S file 1 035.

### 8.3 InitSound and EndSound

InitSound: 1124, 32, driver init (8.4), 64, start routine (8.8), 12, scan (below), 52. The call
ends there (its end clock is `t0 + floor(T/2)`).

Scan cost: per word examined 21; a word that is neither marker adds 21 + 9; a `0x4E75` word ends
the scan (21 in all for it); a `0x41F9` word costs 21 + 21 + 20 and then, for `i = 0..3`, 17 plus
(when the byte is zero) 20 for `i = 0` or 24 for `i > 0`. Test files: C title 1653, C table 406,
S 531 (constant per file).

EndSound: 28 +W(DMACON), 4, then 12 +W(AUDxVOL = 0) for each channel, 16.

### 8.4 Driver init

| step | C | S |
|---|---|---|
| entry | 148 | 148 |
| CIA write | - | 24 +W, then 56 |
| variant step | 12 | - |
| DMACON 000F | 36 +W, 16 | same |
| IE/IR | 40, 20, 16 | same |
| | 12 | 12 |
| ADKCON 00FF | 16 +W | same |
| clear | 44 + 30 x `clear` | same |
| | 48 | 112 |
| | 16 | 16 |
| AUDxVOL = 0, x4 | 12 +W each | same |
| | 4 | 4 |
| AUDxLEN = 1, x4; AUDxPER = 1, x4 | 12 +W each | same |
| | 128 | 128 |
| AUDxLC = silent, x4 | 16 +W each | same |
| | 52 | 52 |
| AUDxLEN = 8, x4 | 12 +W each | same |
| | 60 | 60 |
| AUDxPER = 0x100, x4 | 12 +W each | same |
| DMACON 000F | 16 +W | same |
| tail | 616 | 600 |

### 8.5 Interrupt emulation

**Patched IE or IR assignment** (a driver site that wrote INTENA/INTREQ): 60, no write.

**Audio(entry)**: `entry` (10 or 18, given at each call site), the DMA wait (8.6), 84, then for
`k = 0..3`: 43, and if `IR & IE` has bit `7 + k`: 30 (k = 0) or 34 (k > 0), then Main(k). Then
60, 20 (`IR = 0`), 16.

**Main(k)**: 53, then
- looping: 50 +W(LC), 36 +W(LEN), 24, 10;
- one-shot, `E` reached 0: 21, 24, 20 +W(DMACON), 10;
- one-shot, `E` non-zero: 21, 24 +W(LC), 16 +W(LEN);
then 24, 16.

### 8.6 DMA wait (raster-line wait)

`line(T) = floor((t0 + floor(T / 2)) / 227)` (the beam line counted from clock 0 of the run).
Cost: 46; then the following block nine times: {16, then `r = line`; then repeatedly {16, then
`l = line`, then 9} until `l != r` (the 9 is paid on the exit pass too); then 10}. Finally 44. (The line is sampled at the end of each 16; the loop of 25 cycles samples
every 12.5 cc, so the wait ends 0 .. 13 cc after each line change.)

### 8.7 Volume-stage helpers

With `c` the channel:

- CV(c), the volume stage with its write: `24 + 17 (c + 1) + 74 + (10 unless c = 2) + 18 + 12`, the
  AUDxVOL write being the end of the 12, then 28. Up to the write: 155, 172, 179, 206 for
  c = 0..3.
- SV(c): `20 + [23, 54, 85, 93][c] + 36`, no write.
- SA(c): `20 + [23, 54, 85, 93][c] + 92`, no write.

### 8.8 Start routines (test files)

- C title: 128, then for each of the four voices 36 and a voice start; then 148.
- C table: 168, then per voice started 12 + 18, the voice start, 14; then 148.
- S: 166, then the request: 180, 8, 23; if the flag byte has bit 7: 148 (request end), then the routine's own 148, and done; else 17; if the
  priority byte (now 15) is non-zero: 18, and for `i = 0..2`: 21, plus (if `pattern[i] != 0`) 23
  for `i < 2` or 21 for `i = 2`; then 24; then for each voice `v = 0..3`: 21, and if its
  pattern is non-zero: 60, the voice start, 36; and 22 (every voice). Then 148 (request end) and 148.

**Voice start** (6.2):

| step | C | S |
|---|---|---|
| entry and clear | 892 | 892 |
| IE, IR | 60, 60 | same |
| DMACON dmaclr | 20 +W, 16 | same |
| AUDxVOL 0 | 12 +W | same |
| AUDxLC silent | 24 +W | 20 +W |
| AUDxLEN 1 | 16 +W | same |
| AUDxPER 1 | 36 +W | same |
| DMACON dmaset | 16 +W | same |
| Audio | entry 10 | entry 10 |
| | 20 | 16 |
| pattern | 30 | 54 |
| | 62 | 62 |
| instrument | 108 | 100, 18, SEL |
| periods | 32 | 32 |
| AUDxLC smp | 28 +W | 24 +W |
| AUDxLEN len | 20 +W | same |
| AUDxPER 0x100 | 16 +W | same |
| DMACON dmaclr | 40 +W, 16 | - |
| end | 168 | 168 |

SEL (S period-table selector): `37`, plus `41` if the selector is not 0, plus `24` if it is
neither 0 nor 1, plus 16.

### 8.9 Tick

| step | C | S |
|---|---|---|
| interrupt entry, adapter | 439 | 463 |
| play entry | 60 | 60 |
| per voice: head | 51 | 55 |
| per voice with `ptr != 0` | 23, [event fetch if `dur == 0`], modulation step | same |
| per voice: end | 26 | 30 |
| after voice 3 | 18, envelope pass, 18, writer pass, 36 | same |

(The tick continues with 320 cycles without writes.)

### 8.10 Event fetch

Entry: 21. If the loop offset is 0, the silence sequence: 20 +W(DMACON), 60 (IE), 60 (IR), 16,
16 +W(VOL), 24 +W(LC) (S: 20), 16 +W(LEN), 36 +W(PER), 16 +W(DMACON), Audio(entry 10), 12
(S: 8). Then 27, 4.

For each byte read: 21. A note: 12, then the note cut (20 +W(DMACON), 60, 60, 16, 16 +W(VOL),
24 +W(LC) (S: 20), 16 +W(LEN), 16 +W(PER)), 25, plus 64 if an envelope is set, then 62, 16, 18,
VM (140), 16, 18, PM (198), 16, then the end of the fetch.

A command: 38, then:

| cmd | cycles | C / S differences |
|---|---|---|
| 0x80 | 30 | |
| 0x84, 0xA0, 0xA4 | 18 | |
| 0x88 | 62, song-end test (8.11), 14 | |
| 0x8C | 96 | S: 92 |
| 0x94 | 166 | S: 162 |
| 0x98 | 24 + 22 k + 88 | S: 24 + 22 k + 84 |
| 0x9C | 850 | |
| 0xA8 | 34 | |
| 0xAC | 12, then (S: 94, 8; C: 8), 36, 24, song-end test, 8, 60 (IR), 20 +W(DMACON), 16, 10, end of the fetch | |
| 0xB0 | 68 | S: 92 |
| 0xB4 | 22 | |
| 0xB8 | 47 (n != 0) or 39 (n = 0) | |
| 0xBC | 34 | |
| 0xC0 | 47 | |
| 0xC4 | 27, then 39 if the frame pointer is non-zero (plus 26 when the count reaches 0), else 18 | |
| 0xC8 | 62 | |
| 0xCC | 26 | |
| 0xD0 | 192 | S: 252 + SEL |
| 0xD4 | 56 | S: 80 |
| 0x90 | 0, end of the fetch | |

After every command except 0x90 and 0xAC the next byte is read (21 again). End of the fetch: 52.

VM (volume-modulation restart): 140. PM (pitch-modulation restart): 198. No writes.

### 8.11 Song-end test

32, then 33 for each `i = 0..v`, then 22, 26, 26, 16 for `v = 0, 1, 2, 3`, then 21. If all flags
are now zero: 68, the notification (re-base, 8.1), 16. Then 44.

### 8.12 Modulation step

41. If `env == 0` (otherwise the whole volume part, including its 13 and 16/25, is skipped): 12, then for each volume stage looked at: 21, and a delay found: 30 (stop);
else 21, and a count found: 78 (stop); else 18. Then 13; if nothing was found: 16, 25, and if
`mflags` bit 0: 18 + VM.

Pitch: 12, per stage 21, delay found 30 (stop); else 21, count found 82 (stop); else 18. Then 13;
if nothing was found: 25, and if `mflags` bit 1: 18 + PM.

Then 25; if `env == 0`: 44. Then 36.

### 8.13 Envelope pass

Per voice `v`: 12, plus 18 for `v < 3`; then 27; `ptr == 0`: 16 (next voice). 25; `env == 0`: 16.
25; if `ecnt != 0`: 25, and if still non-zero: 16. Then 18, the step, 16.

Step: 21; released: 16, 21, and if `vol != 0`: 20, 24, 9, plus 4 if `lo < e6` (the result is clamped to 0; an exact
zero `lo = e6` does not pay it), then 12; then 16 (always). Active: 21; attack: 20, 24 (S: 20), 21, plus 28 if the base volume is reached, 12, 16.
Not attack: 17; decay: 20, 24, 17, plus 4 if `lo' >= 0x40`, 21; staying in decay: 12, 16; leaving
it: 26 and the sustain part. Sustain: 28, 35, then 16 if the release point is not reached, else
16, 16.

### 8.14 Writer pass

Per channel `c`: 12, plus 18 for `c < 3`; 16; 40 +W(PER); 42 and CV(c) (the VOL write inside; CV's own
trailing 28 is the only cost after the write); 18 and SV(c); 28; 12; 21. If `trig`: 16; 28 +W(LC) (S: 24); 40 +W(LEN); 10 and
SA(c); 28; 20 +W(DMACON) (S: 16); 60 (IR); 21; then

- loop flag clear: 44, Audio(entry 10), 16;
- loop flag set: 21, then loop offset 0: 44, Audio(entry 18), 24; else 16, 44, Audio(entry 10), 16.

If not `trig`: 16.

## 9. How to check

Compare logs with `tools/replay-cmp.py REF.log PORT.log` or exactly (address, value, colour
clock). Values of the location registers are absolute addresses: load the image at the same
`base` as the reference (LOGS.md) or compare offsets. LOGS.md lists the reference logs.
