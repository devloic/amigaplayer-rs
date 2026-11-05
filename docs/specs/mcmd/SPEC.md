# MCMD replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Amiga module format played by the Eagleplayer "MCMD" ("MCMD player module V1.0 (15 Feb 93)",
creator string "???, Testversion by Eagleeye of DEFECT"; UADE file name prefix `mcmd_org.`). This is a
description of observed behaviour (register-write logs, beam-read and call traces, module data) in my own
words. It contains no instructions or addresses of the original. "cyc" = 68000 cycles, "cc" = colour
clocks of the write logs (1 cc = 2 cyc).

No real module of this format was available (none in the archive index); everything below was checked
on 22 synthetic modules (see LOGS.md). A value-and-timing model written from this text reproduces every
audio register write of the reference (register, value and colour clock) for all 41 042 writes before
the song-end tick, and the timing of every write of the song-end tick (section 10.3 explains why some
values of that one tick come from host memory).

The replay is a 4-voice "sequence" tracker: each voice reads its own pattern list; a note selects an
instrument, which selects a volume sequence and an effect (arpeggio/sample) sequence; samples are started
from inside the effect sequence.

## 1. File layout

All values big-endian. Header (18 bytes):

| offset | size | meaning |
|---|---|---|
| 0 | 4 | magic "MCMD" (0x4D434D44) |
| 4 | word | nE: number of effect sequences (64 bytes each) |
| 6 | word | nI: number of instruments (64 bytes each) |
| 8 | word | nP: number of patterns |
| 10 | word | nO: number of positions (12 bytes each) |
| 12 | word | PS: pattern size in bytes (2 bytes per row, so PS/2 rows) |
| 14 | word | nS: number of subsong entries (8 bytes each) |
| 16 | word | nW: number of sample records (28 bytes each) |

Sections follow the header back to back, in this order, starting at offset 18:
effect sequences (nE x 64), instruments (nI x 64), patterns (nP x PS), positions (nO x 12),
subsongs (nS x 8), sample records (nW x 28), sample data (to the end of the file).
Quirk: each section length is computed as a 16-bit product and added as a signed 16-bit value
(lengths of 32768 bytes or more wrap and move every later section backwards). The pattern start for
pattern n is computed with a full 32-bit product (patterns base + n * PS).

Records:
- Position (12 bytes): four 3-byte entries, voice 0 at byte 0, voice 1 at 3, voice 2 at 6, voice 3 at 9.
  Entry = pattern number p, transpose t (added to the note index), control byte x (section 5.2).
- Subsong (8 bytes): four words: first position, last position, speed (ticks per row), restart position.
  Only the first subsong entry is ever used (the player offers one subsong, number 0).
- Instrument (64 bytes): byte 0 volume-sequence speed, byte 1 effect sequence number, byte 2 vibrato
  speed, byte 3 vibrato depth, byte 4 vibrato delay, bytes 5..63 volume sequence (section 7).
- Effect sequence (64 bytes): byte stream (section 6).
- Sample record (28 bytes): bytes 0..17 name (unused), long at 18 = offset of the sample in the sample
  data, word at 22 = length in words, word at 24 = loop start as a byte offset from the sample start,
  word at 26 = loop length in words. Sample data: signed 8-bit.
- Pattern: PS/2 rows of 2 bytes: byte 0 = note, byte 1 = instrument/flags (section 5.3).

## 2. Detection, subsongs, title, length

- Detection: the first four bytes are "MCMD". Nothing else is checked.
- Note: collections name Jochen Hippel modules `mcmd.*`; those are a different format (the Hippel
  player does not accept this layout and vice versa as far as tested); this player's own prefix is
  `mcmd_org.`.
- Subsongs: 1 (only the first subsong entry is played). No title, no author, no length in the file.
- Song end: section 10.

## 3. Host interface

- Initialisation (once, before the first tick), writes in this order: CIA-A port A bit 1 set (power LED
  off / filter off), DMACON = 0x000F, INTENA = 0x0780, then for channel 0..3: AUDxLC = the address of
  that channel's own register block (0x00DFF0A0 + 16*ch, i.e. hi word 0x00DF, lo word 0xF0A0 etc.),
  AUDxLEN = 0x0001, AUDxPER = 0x0000, AUDxVOL = 0x0000. Measured gaps (cc): the DMACON write is 401 cc
  after the host's init call, INTENA +8, channel 0 LC +373, LEN +6, PER +4, VOL +4, each next channel's
  LC +481 after the previous VOL, then LEN +6, PER +4, VOL +4. (The CIA write comes 45 cc after the init call.)
- One play call per tick from the host's 50 Hz timer (70 940 cc per tick in the reference).
- Volume/balance: two gains, left = (left balance * master volume) >> 6 and right = (right balance *
  master volume) >> 6 (each 0..64, 64 when the host never changes them). Channels 0 and 3 use the left
  gain, channels 1 and 2 the right gain (section 8).
- Voice mask: a voice switched off by the host has every volume write replaced by 0; switching a voice
  off also writes its AUDxVOL = 0 at once.
- Host stop: DMACON = 0x000F and AUDxVOL = 0 for all four channels.
- Song end: the host's song-end routine is called (section 10).

## 4. State

Global: tick counter (starts 1, so the first tick is a row tick), speed (subsong word 2), restart offset
R = (restart - first) * 12 (16-bit), stop flag (off), restart mask M (per tick).

Per voice (channel ch = voice number):
- position base = positions + first*12 + 3*ch, position end = positions + (last+1)*12 + 3*ch, position
  offset (16-bit, starts 12); current pattern (starts at the pattern of the first position's entry),
  row offset (starts 0); transpose t and instrument offset (from the first position's entry; its control
  byte is used directly as the instrument offset, even if it is 0x80 or more);
- note n, last byte 1 `b1`, "previous byte 1" `pb` (starts 0), current sample `cs` (starts 0xFF = none);
- effect sequence pointer, effect position `ep` (starts 0), effect wait `ew` (0), arpeggio value `a` (0);
- volume sequence pointer, volume position `vp` (0), volume counter `vc` and reload `vr` (both 1), volume
  wait `vw` (0), volume value `v` (0), voice percentage `pct` (100);
- vibrato speed, depth, current value, delay (all 0), direction (down);
- portamento accumulator (32-bit, 0);
- loop start `ls` and loop length `ll` (the values written by the DMA phase, section 9).
- Before the first note both sequence pointers point at a built-in default sequence of 8 bytes:
  value 1, six values 0, then the hold command 0xE1 (used as both the effect and the volume sequence).

State that the player does not initialise and that therefore starts with the values stored in the
player file (these matter: they show up in the logs):
- note n: voice 0 = 75, voice 1 = 0, voice 2 = 12, voice 3 = 0;
- loop length ll: voice 0 = 0x0010, voice 1 = 0, voice 2 = 0x0001, voice 3 = 0; loop start ls = 0 for all;
- byte 1 `b1` and the vibrato direction start with no effect (no portamento, direction down).
These hold for the first song after the player is loaded (the reference plays one song per load).

## 5. One tick

Order of work in a tick:
1. If the stop flag is set: write DMACON = 0x000F, then AUDxVOL = 0 for channels 0,1,2,3, and nothing
   else (every tick, forever).
2. Decrement the tick counter; if it reaches 0, reload it with the speed and do the row step (5.1) for
   voices 0,1,2,3 in that order. (Any DMA-off writes made by the row step are written, but the restart
   mask is cleared after the row steps: M = 0.)
3. For voices 0,1,2,3 in that order: effect sequence step (6), volume sequence step (7), period (8),
   write AUDxPER = period, then AUDxVOL = volume (8).
4. DMA phase (9) if M is not 0.

### 5.1 Row step of one voice

Let the row be at (current pattern + row offset), b0 = its byte 0.
1. If (b0 AND 0x7F) = 1 (a break) or the row offset equals PS: advance the position (5.2); the row read
   below is then row 0 of the new pattern (it is not checked for a break again).
2. Read the row: b0, b1 (byte 1).
3. If (b0 AND 0x7F) = 0: go to step 5 (byte 1 is ignored).
4. Otherwise: note n = b0 AND 0x7F; portamento accumulator = 0; `b1` = byte 1. If byte 1 has bit 6 or
   bit 7 set: `pb` = byte 1 of the previous row of the same pattern (for row 0: of the last row of this
   pattern). If b0 bit 7 is set (a "legato" note) go to step 5. Else trigger the note:
   - if `cs` is 0xFF (bit 7 set): write DMACON = (1 << ch) (DMA off; this is not added to M);
   - instrument number = ((byte 1 AND 0x3F) + instrument offset) AND 0xFF; load the instrument record:
     `vc` = `vr` = byte 0; `vp` = 0; vibrato speed = byte 2, depth = current = byte 3, delay = byte 4,
     direction down; volume sequence = record bytes 5..;
   - effect sequence number = instrument byte 1, or `pb` if byte 1 bit 7 is set; `ep` = 0; `ew` = 0;
     `vw` = 0.
   (The volume value `v`, the arpeggio value and the sample are not changed by a note.)
5. Row offset += 2 (always: empty rows, legato notes and trigger notes alike).

### 5.2 Position advance

Next entry address = position base + position offset. If it equals the position end, the song wraps:
the position offset becomes R, the host's song-end routine is called, and the entry used for this
advance is NOT the restart position's entry but three bytes of host memory (10.3). Otherwise the entry
at that address is used. With the entry (p, t, x):
- transpose = t;
- if x < 0x80: instrument offset = x;
- if x >= 0x80, high nibble 0xF: `pct` = 100 if the low nibble l is 0, else (16 - l) * 6 (90, 84, ... 6);
- if x >= 0x80, high nibble 0x8: set the stop flag (takes effect at the next tick, 5 step 1);
- any other x >= 0x80: no effect (instrument offset unchanged);
- row offset = 0; current pattern = patterns base + p * PS; position offset += 12.
Each voice advances on its own (a break in one voice desynchronises it from the others). The test is
equality: a voice whose position offset passes the end without hitting it runs on into the data that
follows.

### 5.3 Row bytes summary

byte 0: 0 = empty; 1 (or 0x81) = break; 2..0x7F = note with trigger; 0x82..0xFF = legato note (new
pitch, no trigger). byte 1: bits 0..5 instrument, bit 6 portamento (speed = `pb` as signed byte),
bit 7 effect sequence taken from `pb`.

## 6. Effect sequence step (once per voice per tick)

If `ew` > 0: `ew` -= 1 and the step ends. Otherwise read bytes starting at `ep`, with a read cursor that
starts at `ep` and advances by the bytes consumed; the stored position `ep` changes only as listed:

| byte | args | effect | `ep` change |
|---|---|---|---|
| 0x00..0xDF | - | arpeggio value a = byte; step ends | +1 |
| 0xE0 | x | jump: cursor = x; continue | = x |
| 0xE1 | - | hold: step ends, nothing changes | none |
| 0xE2 | s | `cs` = 0xFF; M |= (1 << ch); write DMACON = (1 << ch); set sample s (below); `vp` = 0, `vc` = 1; continue | +2 |
| 0xE3 | a, b | vibrato speed = a, vibrato depth = b (current value unchanged); continue | +3 |
| 0xE4 | s | set sample s (below); continue | +2 |
| 0xE5 | s | if s = `cs`: only `vp` = 0, `vc` = 1. Else `cs` = s; M |= (1 << ch); write DMACON = (1 << ch); set sample s; `vp` = 0, `vc` = 1; continue | +2 |
| 0xE6 | - | consumed, nothing else; continue | none |
| 0xE7 | x | effect sequence = number x, `ep` = 0, cursor = 0; continue reading at once (no wait test) | = 0 |
| 0xE8 | x | `ew` = x; then the step starts again from the wait test (so x > 0 waits x ticks counting this one; x = 0 continues reading at `ep`) | +2 |
| 0xE9..0xFF | | not supported (the reference jumps into unrelated code) | |

"Continue" means: read the next byte at the cursor. Because `ep` and the cursor can differ (0xE6 adds
nothing, so a following command's increment is applied from the 0xE6 position), the next tick may start
inside an argument; reproduce the two counters exactly. A sequence that loops without reaching a value,
0xE1 or a wait hangs the reference.

Set sample s: record = sample records + 28*s. start = sample data base + offset (record long at 18).
Write AUDxLC = start (hi word, lo word, same clock), then AUDxLEN = record word at 22. Remember
`ls` = start + loop start (word at 24), `ll` = loop length (word at 26).

## 7. Volume sequence step (once per voice per tick, after 6)

If `vw` > 0: `vw` -= 1, step ends. Otherwise `vc` = (`vc` - 1) AND 0xFF; if it is not 0 the step ends.
Otherwise `vc` = `vr` and read at `vp` (an offset from record byte 5):
- 0x00..0xDF: `v` = byte, `vp` += 1, step ends;
- 0xE0 x: `vp` = x - 5 (x is an offset in the 64-byte instrument record; signed 16-bit), read again;
- 0xE1..0xE7: hold (step ends, `vp` unchanged; the same byte is read at the next step);
- 0xE8 x: `vw` = x, `vp` += 2; then start again from the `vw` test (x > 0: this tick counts as the first
  waited tick; x = 0: the counter is decremented again right away and may read the next byte now);
- 0xE9..0xFF: not supported.
Speed 0 behaves as 256 (8-bit counter).

## 8. Period and volume (once per voice per tick, after 7)

Period table T (73 words, index 0..72); index 73..127 reads past the table and gives 0 in the reference:

    1712 1616 1524 1440 1356 1280 1208 1140 1076 1016  960  906
     856  808  762  720  678  640  604  570  538  508  480  453
     428  404  381  360  339  320  302  285  269  254  240  226
     214  202  190  180  170  160  151  143  135  127  120  113
     113  113  113  113  113  113  113  113  113  113  113  113
    3424 3232 3048 2880 2712 2560 2416 2280 2152 2032 1920 1812
     598

1. Index: if a >= 0x80: idx = a AND 0x7F; else idx = (a + n + transpose) AND 0xFF AND 0x7F. P = T[idx].
2. Vibrato: if delay > 0: delay -= 1, no vibrato this tick. Else: if direction is down: c = current -
   speed; if c < 0 then c = 0 and direction = up. If up: c = current + speed; if c > depth then c = depth
   and direction = down. current = c. P = P + floor(((c - (depth >> 1)) * P) / 1024) (P taken as a signed
   16-bit value in the product; result kept to 16 bits). Vibrato runs every tick once the delay is over
   (with depth 0 it adds 0).
3. Portamento: if `b1` bit 6 is set: s = `pb` as a signed byte; accumulator += |s| << 11 (32-bit; |-128|
   = 128); if s >= 0: P = P - (accumulator >> 16) else P = P + (accumulator >> 16) (16-bit wrap).
4. Volume: vol = (`v` * `pct`) div 100; hardware value = (vol * gain) >> 6 (gain from section 3:
   channels 0, 3 left, 1, 2 right; 0 for a masked voice). No clamping (values above 64 are written as is).
5. Write AUDxPER = P (16 bits), then AUDxVOL = hardware value.

## 9. DMA phase (end of a tick with M != 0)

Wait for 5 beam line changes three times (W5, 11.3), write DMACON = 0x8000 | M, W5 once more, then for
channel 0, 1, 2, 3: AUDxLC = `ls` (hi, lo), AUDxLEN = `ll`. All four channels are written, also those not
in M (with whatever `ls`/`ll` they hold, including the initial values of section 4).

## 10. Song end

1. A voice that wraps its position list (5.2) calls the host's song-end routine. The reference host ends
   the song there: it plays the rest of that tick and one more tick, then stops.
2. Stop command (control byte 0x8n): from the next tick on only the stop writes of 5 step 1; no song end
   is reported (the reference host plays silence until its time limit).
3. Wrap quirk: at the wrap the player takes the next position entry from the three bytes at the start of
   the host's song-end routine instead of from the position list (the address of that routine takes the
   place of the entry address). In the reference host these bytes are an emulator trap opcode: pattern
   number 0xA0 or more, transpose = trap number low byte, control byte 0x4E (so instrument offset 78). The
   pattern then lies far past the module, in host memory, so the values written in the song-end tick and
   after cannot be reproduced; the timing still follows the rules (the row bytes read there were 0 or
   notes). After this the position offset is R + 12, i.e. playback continues with the position after the
   restart position. A port should end the song at the wrap (as the reference host does); for looping
   playback, using the restart position's own entry is the evident intent.

## 11. Timing (exact clock model)

### 11.1 Clock

The reference emulator charges a fixed number of cycles per executed instruction (no data-dependent
costs). The clock in cc is floor(cycles/2) of a running cycle count. The count is re-based to 2 * (the
host's call clock) when the host calls the play routine, and floored to an even count when the player
calls the host's song-end routine (10.1). There is no other re-base inside a tick. Every cost below is in
cycles; a write happens at the clock reached after adding the cost written before it. A long address
write (AUDxLC) logs the hi and lo words at the same clock.

In the reference setup the play calls came at C_k = 73 889 + 70 940 * k cc (k = 0 for the first tick);
that phase is the host's, not the player's.

### 11.2 Costs of one tick

Tick start: 283. If stopped: DMACON at +16, AUDxVOL ch0 at +88, ch1, ch2, ch3 each at +12. Else +33.

Row tick: +42 before voice 0's row step, +26 before each of voices 1..3. Row step of one voice:
- +77; if b0 is not a break: +23 more (row offset test);
- advance: +63; if it wraps: +92, then the host call (re-base, 11.1), then +16; then +57 (entry read);
  then x < 0x80: +12; x >= 0x80: +51 and then high nibble F: +17 and (+22 if low nibble 0, else +50);
  any other high nibble: +17 (and +30 more if it is 8); then +130;
- row read: +35. If the note byte is 0: skip to the end (+32).
- note: +53 (+8 more when the row offset is 0, i.e. row 0), +37; +20 if byte 1 has bit 6 or 7; +13;
  legato: skip to the end. Trigger: +21; DMA off (if `cs` is none): the DMACON write at +36; then +267,
  +12 if byte 1 bit 7, +82.
- end of row step: +32.

After the row steps (or at once on a non-row tick): +34, then for each voice:
- before voice 1, 2, 3: +26;
- effect step (6): if waiting: +47, done. Else +53, then per byte read: +31 and
  - value: +28, done;
  - command: +40, then 0xE0: +58; 0xE1: done; 0xE3: +58; 0xE6: +10; 0xE7: +124; 0xE8: +42 and back to the
    wait test (+47 if now waiting, else +53 and reading goes on);
  - 0xE2: DMACON at +52, then +162, sample set (below), +82;
  - 0xE4: +162, sample set, +54;
  - 0xE5: +33; same sample: +54; new sample: DMACON at +48, +150, sample set, +28, +54.
  - sample set on channel ch: AUDxLC at +12; + (53, 76, 99, 89 for ch 0..3) + 16; +58; AUDxLEN at +24;
    + (49, 72, 95, 85 for ch 0..3) + 16.
- volume step (7): waiting: +47, done. Else +46; counter not 0: done. Else +52, then per byte read: +29
  and value: +28, done; command: +40, then 0xE0: +70 (and read again); 0xE1..0xE7: done; 0xE8: +42 and back
  to the wait test (+47 if now waiting, else +46 ...).
- period (8): +21; +24 if a < 0x80; +59; vibrato delay > 0: +26; else +79, then down: +13 (+26 more when
  it turns), up: +17 (+18 more when it turns), then +118; portamento test +25; with portamento +33 and
  +74 (s >= 0) or +68 (s < 0); then +242.
- voice 0: +46, voices 1..3: +58; AUDxPER at +28; masked voice +4; AUDxVOL at +117; then +40.

After voice 3: +17. If M != 0: +120; three times (+18, W5); DMACON (on) at +12; +18, W5; +8; then for
ch 0..3: AUDxLC at +32, AUDxLEN at +24.

### 11.3 W5 (wait for 5 line changes)

+28. Then five times: +16 and read the beam line L; then repeatedly: +16, read line r, +11 (the +11 is
charged after every such read, including the one that sees the change), until r != L; then +10.
At the end +44. A read happens at the clock after the +16 before it. Line = vertical beam position at that
clock (the reference: frames of 313 lines of 227 cc, frame origin at clock 0, so line = (clock mod 71 051)
div 227; only its low 8 bits are compared). One W5 takes about 5 lines (1 100-1 140 cc); the four waits
put about 5 400-6 900 cc between a voice's DMA off and on.

### 11.4 Examples (from the logs)

- Non-row tick of the "basic" module: AUDxPER ch0 648 cc after the play call; then VOL +58/59, the next
  voice's PER +445..565 later depending on vibrato and arpeggio paths.
- Tick 1 of the "basic" module: DMACON off writes for ch 0..3 355 cc apart (row step of a trigger note
  when `cs` is none), then per voice: DMACON off, LC, LEN, PER, VOL; then DMACON 0x800F about 3 460 cc
  after VOL3, and LC/LEN for ch 0..3 (LC0 1 156 cc after DMACON, then LEN +12, next LC +16).
