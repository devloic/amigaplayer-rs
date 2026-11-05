# Thomas Hermann (1989 game driver) - behaviour specification

*Sanitised for publication: behaviour only.*

Module prefix `thm.`, companion sample file prefix `smp.`. Author string
"Thomas Hermann". Games seen: Beam, Blue Angel 69, Nightdawn (plus a
module of 10112 bytes called "kramer" that is not in the corpus).
All numbers are decimal unless written `0x..`. Multi-byte values are
big-endian. "cc" is a colour clock (the host counts 3546895 per second).

Everything below describes observable behaviour: which hardware
registers are written, with which values, in which order and at which
time. Section 9 gives the timing figures measured from the reference
logs.

## 1. File layout

The module is a single block whose internal pointers are absolute
addresses relative to an *origin* word stored in the file. To use a
pointer, subtract the origin and add the module's start.

| offset | size | meaning |
|---|---|---|
| 0, 4, 8, 12 | 4 longs | pattern pointer list for voice 0..3 (pointing at offsets 64, 1088, 2112, 3136) |
| 16, 20, 24, 28 | 4 longs | transposition table for voice 0..3 (pointing at offsets 4160, 4416, 4672, 4928) |
| 32 | byte | loop position (song restart) |
| 33 | byte | last position (end position) |
| 34 | byte | first position; if bit 7 is clear the driver treats it as 0xFF |
| 35 | byte | number of instruments (samples), 1..31 |
| 36 | long | pointer to the instrument pointer table (32 longs, at offset 5184 in all corpus modules) |
| 41 | byte | rows per pattern minus one (15 everywhere in the corpus) |
| 42 | word | timer word (0x4000 or 0x4400 in the corpus). Drives the replay tick, see section 9 |
| 44 | byte | speed (3 in subsong 0 of the corpus modules) |
| 46 | long | origin |
| 50..53 | 4 bytes | per-voice "disable" flags (0 in all corpus modules); a non-zero byte makes that voice ignore note/command rows |
| 64 | 4 x 1024 bytes | for each voice a list of 256 pattern pointers (4 bytes each, indexed by song position 0..255) |
| 4160 | 4 x 256 bytes | transposition bytes for voice 0..3, indexed by song position |
| 5184 | 32 longs | pointers to the 48-byte instrument records |
| 5312 + 48 n | 48 bytes | instrument record n (n = 0 is a placeholder; samples are 1..N) |
| 6848 | ... | pattern data |

A pattern is `rows` x 3 bytes: note byte `b0`, instrument byte `b1`,
flags byte `b2`. Pattern pointers may be shared between positions and
voices.

### Instrument record (48 bytes)

| offset | size | meaning |
|---|---|---|
| 0 | word | sample length in bytes (the two bytes before the record act as the high word of the length; they are zero in every corpus module) |
| 2 | byte | volume (0..0x7F) |
| 3 | byte | mode: 1 = single-octave sample, anything else = multi-octave sample (see 5.3) |
| 8 | word | loop start offset in bytes |
| 10 | word | loop length in bytes |
| 12 | word | alternate period (used for the 0xCF "fixed pitch" row) |
| 14 | 22 bytes | sample name (NUL padded) |
| 0x23 | byte | envelope flag: 0 = none/one-shot, 1 = looping without envelope, 0xFF = looping with envelope |
| 0x24 | byte | envelope attack time (ticks); 0 = skip the attack |
| 0x25, 0x26, 0x27 | bytes | envelope stage times 1, 2 and release time |
| 0x28, 0x29, 0x2A | bytes | envelope levels 1, 2, 3 (0..0x7F, scale 0x40 = full) |
| 0x2D | byte | portamento slide step (period units per slide tick) |

Other bytes were not needed to reproduce the register writes.

### Sample data

Sample data is NOT in the module. It lives in a companion file whose
name is the module name with prefix `smp.` instead of `thm.` (the
Eagleplayer maps the extension), containing samples 1..N back to back,
each as long as its record's length field. Sample 1 starts at the
beginning of the file. If the companion file is missing the driver
tries one file per sample (the record's name, in an `Instruments/`
directory, optionally an IFF container whose 104-byte header is
skipped); this fallback does not change replay behaviour.
The address written to the Paula location registers is simply the
memory address of the sample data; it is a free choice of the
implementation as long as it is consistent (the logs record the
reference host's addresses; compare per-instrument differences, not
absolute values).

## 2. Detection

Reject unless all of these hold:
* file size greater than 6848;
* origin (long at 46) is non-zero, positive and even;
* long 0 minus origin equals 64; longs 1..4 each exceed the previous by
  1024 (so they equal 1088, 2112, 3136, 4160 after subtracting the origin);
* longs 5, 6 and 7 each exceed the previous by 256 (4416, 4672, 4928).

After loading, every pattern pointer (the 1024 longs at 64) must lie
within the file; the module "song size" is the largest pattern pointer
(as an offset in the file) plus 64 and must not exceed the file size
(otherwise "module too short").

## 3. Subsongs, title, length

* Title: none. Author: "Thomas Hermann".
* Number of subsongs: 5 if the song size is exactly 11792 (Blue Angel
  69), 3 if exactly 10112 ("kramer"), else 1.
* The subsong overrides four values: loop position, end position,
  first position and speed. The timer word at 42 is not changed.
  For single-subsong modules these are the module's own header bytes
  (loop at 32, end at 33, first at 34 with the 0xFF rule above, speed
  at 44). The multi-subsong modules use these (loop, end, first, speed):

  * 3 subsongs: (0x00, 0x6F, 0xFE, 3), (0x80, 0x87, 0x7F, 3), (0x89, 0x89, 0x88, 3)
  * 5 subsongs: (0x11, 0x5A, 0xFF, 3), (0x60, 0x85, 0x5F, 5), (0x90, 0xB5, 0x8F, 5), (0xC0, 0xC7, 0xBF, 3), (0xD5, 0xD6, 0xCF, 3)
* Song length in positions = 256 - first + end + 1 (byte arithmetic on
  the values in use). Number of replay ticks of one pass =
  length x (speed + 2) x rows. Duration is that tick count times the
  tick period (section 9), converted to seconds.
* The song end is signalled when the last row of the end position has
  been played and the position wraps to the loop position.

## 4. Replay state and the tick scheduler

Two periodic interrupts exist:

* **Play tick**, period P = (timer word + 1) x 5 cc (81925 cc with 0x4000,
  87045 cc with 0x4400). Handles the speed counter, rows and note
  starts.
* **Slide/envelope tick** ("TB tick"), period 137240 cc (the timer is
  loaded with the two bytes of the default host timer value written
  swapped, giving 27447 + 1 timer counts of 5 cc). Handles portamento
  slides and, on every 4th call, envelopes.

Both start when the sound is initialised, from the write that starts
the second timer (call its time Ts). Nominal due times:

* TB tick n (n = 1, 2, ...): floor(Ts / 5) x 5 + 137240 x n.
* Play tick k (k = 1, 2, ...): D0 + P x k, with D0 = Ts + 155 in the Beam,
  Beam-unused and Nightdawn runs, Ts + 190 in the Blue Angel 69 runs
  (153 in its subsong 4) and in two of the synthetic runs. The 35 cc
  spread between runs was not explained; it is the same 35 cc that
  appears in the entry-to-first-write offsets (section 9).

Order of the handlers is by nominal due time. A handler always runs to
completion; a handler that comes due while another runs starts when the
other has ended (its own writes are then late by that amount, but the
next nominal due time is unchanged). When a Play tick and a TB tick have
the same due time the TB tick runs first. The same rule decides audio
interrupts: an interrupt that Paula raises while a handler runs is served
right after that handler ends; one raised while the processor is idle is
served at once (265 cc later the first write, section 9).

Four **audio interrupts** (one per Paula channel, served by a single
shared handler) run in addition, see section 7.

Per-voice state: loop-flag/envelope flag, release flag, irq stage (0..2),
current volume level (16-bit, high byte = level), level rate (signed 16),
envelope stage (0..5), max volume, slide-active flag, current period,
target period, loop address/length, pointer to the voice's instrument
record. A 32-bit "leftover" value (section 5.4) is shared between voices.

### Speed counter

`speed` is the byte from the subsong. A counter starts at `speed`. Each
Play tick:

1. counter == 0: **load tick** (5.1); counter becomes 0xFF.
2. counter == 0xFF (bit 7 set): **start tick** (5.2); counter becomes
   `speed`; the row counter advances (5.5).
3. otherwise: **idle tick** (nothing is written); counter decreases by 1.

One row therefore takes `speed + 2` Play ticks: `speed` idle ticks, one
load tick, one start tick. The first row shown by the load tick is
position `first`, row 0.

## 5. Rows

### 5.1 Load tick (reads the next row, stops channels)

For each voice 0..3 in order, the row at the current position and row is
fetched: pattern pointer from the voice's list, transposition byte from
that voice's table. Transposition applies to `b0` when `b0 < 0xC0`: the
note number (high nibble) plus the signed transposition is formed; if the
result is negative, 4 is subtracted from the nibble result and 1 is
subtracted from the octave (low nibble); if it is 12 or more, 4 is added
and 1 is added to the octave. Then the stored `b0` = result nibble << 4 |
octave (low nibble, 8-bit wrap). Rows with `b0 >= 0xC0` are stored as is.

Then, unless bit 1 of `b2` is set: if `b0 < 0xC0` (after transposition)
or `b0 == 0xCF`, the voice is stopped now: write DMACON with the voice's
single bit (clear), then INTENA with the voice's interrupt bit (a clear
write). The voice's active flag is cleared. Voices not stopped write
nothing. No disable flag is checked here.

### 5.2 Start tick (executes the rows)

First the shared leftover value is set to 0. Then for each voice 0..3 in
order with a zero disable byte:

1. The low byte of the leftover value is replaced by the voice's `b1`
   (the high bytes keep whatever the previous voice left, section 5.4).
2. Dispatch on `b0` (after transposition):
   * `0xFF`: nothing.
   * `0xDF`: set the release flag of the voice (envelope release, 6.2);
     no register is written here.
   * `0xEF`: stop the voice: DMACON clear of its bit, then INTENA clear
     of its bit; the voice's active flag is cleared.
   * `0xCF`: start a note at the record's alternate period (5.3).
   * other `b0 < 0xC0`: start a note (5.3).
   * `b0` in 0xC0..0xFE not listed: nothing is written, but the
     leftover value is updated as in a rejected note (5.4).
     (The generator treats every value below 0xC0, 0xCF, 0xDF, 0xEF and
     0xFF as defined; 0xC0..0xCE, 0xD0..0xDE, 0xE0..0xEE are ignored.)

### 5.3 Note start

The record number is the low 16-bit word of the leftover value (the
voice's instrument byte in its low byte plus whatever the previous
voice left in its high byte, section 5.4). The driver does not check it.
The record address is the longword found at (start of the pointer
table + 4 x record number), where the offset is formed from the high
word of the leftover value (times 65536) plus (4 x the record number
modulo 65536). For record numbers 0..31 this is the normal table.
For larger numbers it reads the module's own following bytes (the
instrument records, whose offset 4 field holds the memory address of
that instrument's sample data once loaded), or past the end of the file.
Whatever longword results is used as a memory address and 48 bytes at
that address are read as the record. Observed cases:

* The longword is the sample-address field of instrument n (offset
  4 of record n): the "record" is the first 48 bytes of that sample's data.
  The record's address, length and loop fields are then taken from
  sample bytes; this was verified in the logs (the written location
  and length registers equal those sample bytes).
* The longword is in unmapped memory (all-zero reads), such as an address
  past the sample data: all-zero record (length 0, volume 0, mode 0,
  flag 0, address 0).
* A small address in the host's low memory: the reference host's bytes
  there are not part of the format. For address 0xFF the reference run
  gave a record of length 0x0400, volume 0, mode not 1, location
  0x04000004, loop start 0x0400, loop length 4 and envelope flag 1.
  For address 0 the record was all zero.

An implementation inside the reference host sees the same memory; a
stand-alone one should reproduce the first two cases exactly and treat
the third as unknowable.

The record gives length, volume, flag, loop fields and alternate period.

Note byte: high nibble = note 0..11, low nibble = octave code. Base
periods per note: 0x1AC, 0x194, 0x17C, 0x168, 0x154, 0x140, 0x12E,
0x11E, 0x10E, 0xFE, 0xF0, 0xE2.

*Mode 1 (single sample)*, `b0 != 0xCF`: octave code 1 uses the base period
as is; code 2 halves it (rejected if the halved period is 0x82 or less);
code 0 doubles it; code 15 multiplies it by 4; any other code rejects
the note. For `0xCF` the period is the record's alternate period.
The sample address and length are used as in the record.

*Mode other than 1 (multi-octave sample)*: the sample holds the octaves
one after another. Octave code 3: base period halved (rejected if 0x82 or
less), address and length as in the record. Code 2: base period, as
record. Code 1: the sample address is advanced by the record length,
the length is doubled, loop start offset and loop length are doubled.
Code 0: address advanced by 3 x length, length x 4, loop fields x 4.
Code 15: like 0 but the period is doubled first. Other codes reject.
`0xCF` uses the code-1 layout with the alternate period.
For codes 0/1/15/0xCF the high word of the scaled loop start becomes the
high word of the leftover value afterwards.

A **rejected** note writes nothing and leaves the voice as it was. The
leftover value keeps its high word and its low word becomes: record
number x 4 when the row byte is 0xC0 or above (not 0xCF); the halved
period when a halving was rejected for being too small; the base
period when the octave code is unsupported.

An accepted note writes, in this order:

1. ADKCON (0x9E) <- 0x00FF.
2. Location high and low words of the sample address.
3. Length register = record length / 2 (words).
4. Period register <- the note period (alternate period if the row was
   0xCF) unless bit 0 of `b2` is set (portamento row). With portamento
   the period is not written; the target period is set to the new note
   period and the slide flag is switched on (6.1). Without it the slide
   flag is cleared and the current period equals the new period.
5. Volume register (see below).
6. A wait of 9 raster lines (see 9).
7. INTENA set (0x8000 | the voice's bit).
8. DMACON set (0x8200 | the voice's bit).

Volume: if the record flag is 0, write the record volume. If it is
nonzero, loop address = sample address + loop start and loop length (in
words) = loop length / 2 are remembered for the audio interrupt, the
envelope stage is cleared, the maximum volume is the record volume;
for flag 1 the level starts at 0x4000 and the record volume is written;
for flag 0xFF the volume 0 is written unless the attack time (0x24) is
zero: then the stage becomes 1, level = level 1 << 8, and the volume
written instead is scaled level 1 (formula in 6.2). All volume values
written are masked with 0x7F. The voice's audio-interrupt stage is reset
to 0 and the release flag to 0.

### 5.4 Leftover value (instrument-index pollution)

One 32-bit value is shared across the voices of one start tick (it is
cleared at the start of the tick; disabled voices do not touch it).
Each voice overwrites only the low byte of it with its `b1`. After a
voice's row:

* accepted note, flag 0 or 1: low word = the written volume (0..0x7F);
* accepted note, flag 0xFF with non-zero attack time: 0;
* accepted note, flag 0xFF with attack time zero: low word 0, high word =
  the high word from the loop rule in 5.3;
* rejected note: see 5.3 (the low word keeps a non-zero high byte);
* rows 0xFF, 0xDF, 0xEF: unchanged.

Because the next voice only replaces the low byte, a high byte left by a
rejected note (periods are 0x0E2..0x35E, so the high byte is 1..3, or
the record number x 4 with a high byte for large instrument bytes) turns
the next voice's record number into 256 or more: that voice then reads
a "record" through the out-of-table rule of 5.3 (usually an all-zero
record: address 0, length 0, volume 0, DMA started on a silent empty
channel; sometimes sample bytes). This was reproduced exactly in the
synthetic modules, where it is frequent; real modules in the corpus
hit it rarely.

### 5.5 Row and position advance

After each start tick the row counter increases; when it passes the last
row it wraps to 0 and the position advances by 1 (byte wrap), except
when the position equals the last position: then the position becomes the
loop position and the song-end event is raised.

## 6. Timer-B tick

Each call runs, in channel order 0..3, the slide step for each channel,
and then every fourth call (the 4th, 8th, ...) the envelope step for each
channel, again in order 0..3.

### 6.1 Slide

Only when the voice's slide flag is on. Compare the current period with
the target (unsigned). Below target: add the record's slide step; at or
above target the slide is finished. Above target: subtract the step;
finished when it falls to the target or below. Equal: finished at once.
On finishing the current period becomes the target and the flag is
cleared. In every case the new current period is written to the Paula
period register (even if the step is 0).

### 6.2 Envelope

Only for voices whose active flag is non-zero (the flag byte copied
from the record at note start). A release (flag set by 0xDF) takes
priority. Without release, only records with flag 0xFF run an envelope;
stages 1..3 below, stage 4 holds, stage 5 is finished.

Volume write helper: `(max volume x level byte) >> 6`, masked 0x7F.

* Stage 0 (first call after a start with attack time n != 0): rate =
  (level 1 x 256) / n (integer, 16 bit); level = rate; stage 1; write
  the volume of the high byte of the level.
* Stage 1, 2, 3 each use (target T, next level N, time D):
  stage 1 (T = level 1, N = level 2, D = time 1), stage 2 (T = level 2,
  N = level 3, D = time 2), stage 3 (T = level 3, N = 0, D = time 3).
  A zero D counts as 1. The level is increased by the signed rate. The
  level byte is the high byte. If the rate is negative the stage is
  reached when the byte (as signed 8 bit) is <= T, else when >= T.
  Not reached: write the volume of the level byte. Reached: level =
  T << 8; write the volume of T; the new rate = ((N - T) << 8) divided by
  D as a signed 16-bit division of a 32-bit value; if the quotient does
  not fit signed 16 bits the rate is instead the low 16 bits of the
  dividend; stage + 1.
* Stage 4 (sustain): nothing, until a release.

*Release*: when the stage is not yet 4: stage 4 and rate = -(level)
divided by release time (D, 0 counts as 1) with the same overflow rule.
Then, and on every following call, one step of the generic step as above
with T = 0, N = 0, D = 1. When the stage reaches 5 the voice becomes
inactive (no more envelope calls). Release also acts on voices with flag
1.

## 7. Audio interrupt handler

Each Paula channel's audio interrupt is served by one handler: with the
set of pending enabled audio interrupts, the highest channel number is
chosen (channel 3 > 2 > 1 > 0). It always begins by writing INTREQ with
the channel's bit (a clear write).

* If the voice's record flag is non-zero (looping): the first
  interrupt only advances the voice's stage counter. The second writes
  the loop: location high and low words of the loop address, length
  register = loop length in words, then INTENA clear of the bit. (The
  Paula latched the first buffer, so this re-points the channel to the
  loop.)
* If the flag is 0 (one-shot): the first and second interrupts only advance
  the stage; on the third: INTENA clear of the bit, stage reset to 0,
  DMACON clear of the bit.

Interrupts that are not raised stay unserved: a serviced interrupt
requires that its pending flag came up while the host was able to
dispatch it; if several channels come up within the same dispatch window,
the handler runs once per raised channel (highest pending first), and
some raises are lost. Host-specific rules are in section 9.

## 8. Song end

Raised once each time the last row of the end position finishes. The
reference host just records the event; replay continues at the loop
position.

## 9. Timing

All times in cc, measured from the logs. "Due" is the nominal due time of
the tick (section 4).

### Play tick

* Idle tick: occupies 150 cc, writes nothing.
* Load tick: the first stopped voice (index k) has its DMACON write at
  due + 499 + 240 x k, plus 34 more when k > 0. (Offsets vary, see the
  end of this list.) For each stopped
  voice: DMACON clear, then INTENA clear 15 cc later. The next stopped
  voice's DMACON write comes 305 cc after the previous INTENA write, plus
  240 cc for each voice skipped (not stopped) in between. The tick ends
  20 cc after the last write (249 cc after due if nothing was
  written).
* Start tick: first write (ADKCON of the first note start) at due + 492;
  if the first write is a stop (0xEF) it comes at about due + 385 (one
  observation, unverified). Per voice:
  * stop (0xEF): DMACON clear, INTENA clear 15 cc later, then 175 cc
    before the next voice starts;
  * note start: ADKCON write; 48 cc later the two location writes
    (same cc); then the length write after a voice dependent gap g0;
    then the period write after g1; then the volume write after g2.
    g0, g1 and g2 per voice 0..3: (80, 134, 251), (92, 141, 284),
    (105, 153, 313), (109, 163, 335). With a portamento row the period
    write is absent and g1 + g2 elapses before the volume. After the
    volume write (time v) the driver waits for the raster line to change
    9 times. With lines of 227 cc starting at multiples of 227, let
    B = (floor((v + 53) / 227) + 9) x 227 and N = the smallest integer
    with v + 5 + 12.5 x N >= B. The INTENA write falls at
    v + 46 + 12.5 x N (rounded; the reference differs by 1 cc from this
    in about half of the cases for channels 1..3, undetermined). This
    matched 7000 waits to within 1 cc except for about 1.3 percent where
    it is 12 or 225 cc later. Then DMACON is written 21 cc after the
    INTENA write.
    The next voice starts 299 cc after the DMACON write.
  * The tick ends 20 cc after the last write.
* A voice whose row writes nothing adds no time to the start tick; a
  start tick in which no voice writes anything occupies 150 cc.
* The entry-to-first-write offsets above are the most frequent values in
  the Beam and Nightdawn logs and are exact there. In the Blue Angel 69
  logs the processor is slowed by audio DMA: the start-tick offset
  (relative to 492 at D0 = Ts + 155, i.e. 527 at D0 = Ts + 190) took the
  values 457, 517, 533, 556 and 676, 517 whenever channel 0's DMA was on,
  533 when channel 1's was on without channel 0, 676 with channels 0, 1
  and 3 on, and 457/533/556 (not predictable from the enabled set) when
  neither channel 0 nor 1 was on. Writes that follow are shifted by the
  same amount. This is the one source of timing error I could not remove;
  it decides near-ties between a Play write and an audio-interrupt write
  (see README).

### TB tick

Slides are handled for channels 0..3 in turn, then (every 4th call)
envelopes. Time marker tau starts at due + 166.5. A slide channel that
writes nothing advances tau by 37.5; a channel that writes puts its
period write at tau and advances tau by 112. If envelopes run this call:
a channel without a write advances tau by 56; a channel with a write
puts the volume write at tau + 261.5 and advances tau by 456. The
handler ends about 100 cc after the final tau (a guess; Play ticks
that were delayed by a TB tick were too few to measure it). Checks:
the first period write of channel 3 at due + 279 (Beam, Blue Angel,
Nightdawn: 279, 280 or 289), channel 1 at 204 (Nightdawn 214), the first
volume write of channel 3 at due + 746 (Beam) and of channel 2 at 694.

### Audio interrupt

Entry 265 cc after the dispatch time. INTREQ write at entry. Loop
reload: the three loop register writes 70 cc after the INTREQ write (all
at the same cc), the INTENA write 10 cc after them. One-shot end:
INTENA 10 cc after the INTREQ write, DMACON 10 cc after that. A stage
advance writes only the INTREQ. The handler ends 113 cc after its last
write; the next raised channel, if any, is served immediately.

### Host dispatch and Paula behaviour (needed to reproduce the interrupts)

* Paula audio: a DMA start request is taken at the next video-line
  boundary (227 cc lines); the first sample word is fetched about 227 cc
  later. Each word lasts the period value. A new INTREQ is raised at the
  start of the last word of the buffer (the end of the first buffer is
  the first interrupt), and the loop registers are re-read at the end of
  the buffer.
* The host evaluates interrupts only at its dispatch rounds: whenever
  a timer is due, and about every output sample (3546895 / 48000 = 73.9
  cc). The interrupt flags raised since the previous round are served
  in the order described in section 7; a flag still pending when its
  enable bit becomes set in the meantime counts as newly raised.
* If a Play tick and a TB tick have the same due time the TB tick runs
  first; otherwise the earlier due time runs first (section 4).

## 10. Implementation notes

* Hardware writes use the full 16-bit values given above. Length is in
  words.
* Both tick routines run to completion without being interrupted by
  anything but the audio interrupt.
* The sample memory is the implementer's; location registers are
  written as the sample address plus offsets (all offsets above).
