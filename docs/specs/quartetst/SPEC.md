# Quartet ST replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the "Quartet ST" music format (Rob Povey and Steve Wetherill, 1989-90; Atari ST music used in
Amiga ports such as "Projectyle"), score file prefix `qts.` with a companion sample-set file, as played by the
reference (the Eagleplayer "Quartet ST", Wanted Team adaptation V1.2, 29 Aug 2009). The Amiga "Quartet"
(`qpa.`) and "Quartet PSG" formats are different and are not covered. This spec describes observed
behaviour (register-write logs, cycle traces, module data) in my own words. "cc" means Amiga colour clocks.
"cycles" means CPU cycles of the reference host (1 cc = 2 cycles).

A model written from this description matches the reference's register writes exactly (address, value,
order and colour-clock stamp, first tick included) for 42 synthetic modules (about 975 000 writes). No
real Quartet ST module was available (see README).

Everything is big-endian.

## 1. Files

### 1.1 Score (`QTS.NAME`)

| offset | size | meaning |
|---|---|---|
| 0 | word | 1..16 (not used by the replay) |
| 6 | byte | 0..4 (not used) |
| 7 | byte | 4 |
| 8 | long | 0 |
| 12 | long | 0 (ST variant), or ASCII `WT` followed by a word *speed* (Amiga variant, 1.4) |
| 16 | 12 per command | four command streams (voices 0, 1, 2, 3) one after the other. Each ends with an `F` command |

A command is 12 bytes. Its first word is the code. The other fields are:

| code | name | fields (offsets inside the command) |
|---|---|---|
| 0x50 `P` | note | +2 word duration (ticks), +4 long frequency value *f* |
| 0x52 `R` | rest | +2 word duration |
| 0x53 `S` | slide | +2 word duration, +4 long target, +8 long delta (both signed) |
| 0x56 `V` | voice/sample | +8 long sample index x 4 (0, 4, ..., 0x4C) |
| 0x6C `l` | loop start | none |
| 0x4C `L` | loop end | +8 word repeat count *n* |
| 0x46 `F` | end of stream | none |

Unused fields may hold anything. In each stream, commands are read in order. `V`, `l`, `L` and `F` take no
time. `P`, `R` and `S` (and unknown codes, 4.4) end the reading for that tick.

### 1.2 Detection

The score is accepted when all of these hold:
- word 0 is 1..16; byte 7 is 4; byte 6 is at most 4; the long at 8 is 0;
- bytes 12-13 are `WT`, or else the long at 12 is 0;
- the long at 24 (the first command's sample index) is at most 0x4C and a multiple of 4;
- the word at 16 is 0x56 (the first command is a `V`).

Loading also fails ("too short") unless four `F` commands are found completely inside the file. The search
looks at the command codes at offsets 28, 40, 52, ... (the command at 16 is not looked at). The song size is
the offset just after the fourth `F`. Loading fails when the sample set is missing or corrupt (1.3).

### 1.3 Sample set (`SMP.NAME`, else `SMP.set`)

Companion name: the score's file name with its first three characters replaced by `SMP`. This applies when
the name starts with `QTS.` (each letter in either case). The file is looked for in the score's directory.
If it cannot be loaded, `SMP.set` in the same directory is used.

Set layout (size Z bytes):

| offset | size | meaning |
|---|---|---|
| 1 | byte | number of samples + 1 |
| 142 + 4 i | long | offset *o_i* of sample record i (i = 0..19), 0 = no sample |
| 222 | 4 | optional ASCII `2BIT` (named-sample sets) |

A sample record at offset o: word *loop* (signed) at o, word *length* L (bytes) at o + 4, and unsigned 8-bit
sample data from o + 8. The words at o + 2 and o + 6 are not used. The sample's address is (load address of
the set) + o + 8.

Slot table (i = 0..19, in order):
- o_i = 0: empty slot.
- o_i has bit 31 set: the set is corrupt (the module does not load).
- o_i > Z: empty slot.
- `2BIT` sets only: the three low bytes of the long at o_i - 120 must be ASCII `BIT`, otherwise the slot is
  empty. The record's 8-character name is at o_i - 116 (display only).

Conversion (i = 0..19, in order, for slots that are not empty). Let n = L with bit 0 cleared:
- If the record was already converted through an earlier slot (two slots may share a record), skip it. The
  reference recognises a converted record by the ASCII `WT` it writes into the word at o + 6. A file
  that already has `WT` there counts as converted.
- If o + 8 + n > Z, the record is not converted (its slot still points at it, with unconverted data).
- Otherwise: each of the n data bytes becomes (byte - 0x80) (unsigned to signed). In the ST variant (not
  Amiga mode, 1.4) the data is then halved in place: for k = 0 .. n/2 - 1, byte k = floor((s[2k] +
  s[2k+1]) / 2) of the signed values, written to the start of the data. The other bytes are left as they are.
  Each converted record counts one.

The count of converted records must equal (byte 1) - 1, otherwise the set is corrupt. An ST-variant
record with n = 0 makes the reference halve without end (not supported).

The **empty sample** is a 4-byte zero buffer of the player in chip memory. It has loop word -1 and length 4.
It is used by `V` commands whose slot is empty, by rests, and as the repeat of non-looping samples.

### 1.4 Variant, tick rate, info

- Amiga mode (no halving of lengths and data) is on when the score has `WT` at 12. It is also on when the set
  is at least 83 230 bytes long and holds ASCII `HfR0` at offset 760 and `MP7E` at offset 13 588.
- Tick period: 5 x (timer + 1) cc. timer = 3547 (about 200 Hz) when the score has no `WT`, or has `WT` with
  speed 0. Otherwise timer = floor(709 350 / speed); for speed <= 10 that quotient overflows and timer =
  53 990. Example: speed 50 gives 70 940 cc, speed 100 gives 35 470 cc.
- Sample indices in `V` commands are resolved once at load time. Index / 4 gives the slot; an empty slot gives
  the empty sample.
- Info: no title or author, one subsong. Sample count = (byte 1 of the set) - 1. Length = floor(B / 192) + 1,
  where B is the byte length (`F` included) of the longest of the four streams; the first longest wins.
  Position = floor((current command offset - stream start) / 192) of that voice.

## 2. Hardware written

The player writes AUDxLC as one 32-bit write (two 16-bit writes, high word first, same clock), AUDxLEN,
AUDxPER, AUDxVOL, DMACON and INTREQ. It reads INTREQR. It writes nothing at initialisation.

Volume is fixed: every tick writes AUD0VOL = left gain, AUD1VOL = right gain, AUD2VOL = right gain and
AUD3VOL = left gain. gain = (balance x master volume) >> 6 (64 by default). A voice switched off by the host
writes 0. There is no volume in the format.

## 3. State

Per voice c (0..3): restart flag, loop-pending flag, current command code, duration counter (16-bit), sample
(address, length in bytes), repeat (address, length in bytes), frequency value f (32-bit), slide target and
delta (32-bit), stream start, stream position and a loop stack.

Initial values: code `R`, duration 1, sample = repeat = empty sample with length 2, f = 0, flags clear,
empty loop stack.

Default stream: count the commands with code `P` in each stream (scanning 12-byte commands up to its `F`).
A stream with none is replaced by a built-in stream: rest for 10 000 ticks, then `F`.

## 4. Replay

### 4.1 Period function

per(f): d = f >> 3 (unsigned 32-bit).
- If d = 0: per = 1.
- Otherwise let v = d & 0xFFFF and q = floor(3 579 546 / v). If q > 65 535: per = 0x9E9B. Otherwise
  per = q + 1.
- If v = 0 while d != 0 (e.g. f = 0x80000), the reference stops with a division-by-zero exception (not
  supported).

So f is about eight times the sample rate in Hz.

### 4.2 One tick (exact write order)

1. Volumes: AUD0VOL, AUD1VOL, AUD2VOL, AUD3VOL (2).
2. Hardware stage, for c = 0, 1, 2, 3:
   - If the restart flag is set, clear it. Then:
     - If the current code is `R`: AUDxLEN = 1, then AUDxLC = empty sample.
     - Otherwise: AUDxLC = sample address; AUDxLEN = sample length >> 1; set loop-pending;
       INTREQ = 0x80 << c; DMACON = 0x8000 | (1 << c).
   - Otherwise read INTREQR. If its bit 0x80 << c is set, write INTREQ = 0x80 << c. Then, if loop-pending
     is set, clear it and write AUDxLC = repeat address, then AUDxLEN = repeat length >> 1.
   - AUDxPER = per(f).
3. Slide stage, for c = 0..3: if the current code is `S`:
   f = f + delta (32-bit wrap). Then compute X = ((f >> 3) & 0xFFFF0000) | per(f). This is the value the
   reference compares; it does not compare f itself. X = 1 when f >> 3 = 0.
   The slide has reached its end when delta >= 0 and X >= target, or when delta < 0 and X <= target.
   Both comparisons are signed 32-bit. When it has, f = target and the code becomes `P` (the slide stops).
   Consequences: a downward slide (delta < 0) to any normal target ends after its first step, jumping to the
   target. An upward slide ends at once only when the target is at most the period, otherwise it runs until
   the next command. Nothing is written here; the new f is written by step 2 of the next tick.
4. Command stage, for c = 0..3: duration = duration - 1 (16-bit). If the result is not 0, go to the next
   voice. Otherwise read commands from the stream position. Each command read becomes the current code:
   - `P`: DMACON = 1 << c (voice off). duration = its duration, f = its frequency value, set the restart
     flag. Stop.
   - `R`: DMACON = 1 << c. duration = its duration, set the restart flag. Stop. (f is unchanged.)
   - `S`: duration, target and delta from the command. Stop. (No restart, no DMA change; the note goes on
     sounding.)
   - `V`: sample address A from the slot (1.4), length ℓ = L of the record (halved, rounding down, in the ST
     variant; the empty sample gives 4, or 2 in the ST variant). If the record's loop word is negative:
     repeat = empty sample with length 2. Otherwise let p = loop word (halved in the ST variant): repeat
     length = p, repeat address = A + ℓ - p (ℓ and p added as signed 16-bit). Continue.
   - `l`: push (position of the next command, count 0). Continue.
   - `L`: pop the top count k. If k = 0, k = n + 1. Then k = k - 1. If k = 0, drop the entry and continue
     after the `L`. Otherwise store k back and continue at the entry's position. So the loop body is played
     n + 1 times; n = 0 plays it once.
   - `F`: position = stream start, note the song-end mark for voice c (4.3), and continue reading there. The
     loop stack is not cleared.
   - Any other code: stop. The position moves on by only 2 bytes and the duration stays 0, so the next tick's
     decrement gives 65 535: the voice waits 65 535 ticks, then reads from the middle of that command.
   The restart flag and loop-pending flag stay as they are unless changed above. For example, after a `P`
   whose interrupt has not come yet, a following `R` leaves loop-pending set, and the repeat is written when
   the interrupt bit is seen.

The first tick therefore writes period 1 on every voice and reads each stream's first commands. Notes start
sounding in the second tick. A note's DMA off (command stage) and DMA on (next tick's hardware stage) are
one tick apart.

INTREQR is read from Paula as it is at that clock (5.1). Paula sets a voice's bit when its DMA fetches the
first data after being turned on, and again each time it reloads the location registers at the end of a
block. A port must model this with the same Paula model to produce the same INTREQ and repeat writes.
After a reset INTREQ is 0.

### 4.3 Song end

Each voice has a pending mark, all set at the start. An `F` on voice c clears c's mark. When all four are
clear, the host is told "song end" and all four are set again. The reference host stops playing then. Voices
running the default stream reach their `F` only after 10 000 ticks.

## 5. Timing

### 5.1 Clock model

Ticks are called every period (1.4). For a tick called at clock C, keep a cycle count T starting at 0. A
write gets the stamp C + floor(T / 2), where T already includes the step that makes the write. The INTREQR
read is timed the same way. All costs are fixed: no cost depends on data or branch direction except as listed.
The song-end call to the host rounds T down to even.

K(f) = 43 if f >> 3 = 0, else 183 (the period computation).
Q(c) = 53, 94, 135, 143 for voices 0..3. SP(c, f) = Q(c) + 70 + K(f) (bookkeeping of a period).
E(c) = 55, 92, 125, 148 for voices 0..3 (song-end bookkeeping).

### 5.2 Tick ledger (cycles)

| step | cost |
|---|---|
| tick entry | 250 |
| each volume write | 56 -> write (four times) |
| start of hardware stage | 42 |
| per voice | 45 |
| restart set | 25; then `R`: 16 -> AUDxLEN, 24 -> AUDxLC, 10; otherwise: 28 -> AUDxLC, 32 -> AUDxLEN, 40 -> INTREQ, 36 -> DMACON, 10 |
| restart clear | 12 -> INTREQR read, 21; bit set: 20 -> INTREQ, 29; loop pending: 28 -> AUDxLC, 32 -> AUDxLEN |
| period | 34 + K(f) + 12 -> AUDxPER, then 18 |
| end of hardware stage | 16 |
| slide stage, per voice | 51; not a slide: 16. Slide: 68 + SP(c, new f) + 52; end reached: + 62 + SP(c, target); then 16 |
| command stage, per voice | 53; duration not 0: 16. Otherwise 32, then per command read 20 plus: |
| `P` | 41 -> DMACON, then 58 + SP(c, new f) + 38 |
| `R` | 58 -> DMACON, then 54 |
| `S` | 199 |
| other code | 125 |
| `V` | 145, + 16 in the ST variant, + 18 + Q(c) + 72 + 21; then no loop: 50; loop: 21, + 8 in the ST variant, + 62 |
| `l` | 131 |
| `L` | 160; + 8 if the popped count was 0; + 13; then jump back: 30, or done: 18 |
| `F` | 113 + 32 + E(c) + 21; if this completes the song: + 68, round T down to even, + 16; then 54 |
| after the stopping command (`P`, `R`, `S`, other) | 48 |

The volume writes are at T = 306, 362, 418 and 474 (stamps C + 153, 181, 209, 237).

### 5.3 First tick

The host calls the player's start-up routines one after the other, starting at clock 100. Each routine's
duration is its cycle count halved and rounded down. "|" marks a call into the host (a separate rounding
point; the 16 after it is the host stub's cost).
- check: 261, + 25 without `WT`;
- companion load: 60 | 284 + 17 x (D + 1) + 21 x (N + 1) + X | 45 (the 284 and the 45 include the 16 of the
  host stub before them). D = length of the score's
  directory string as the host gives it (with its trailing `/`). N = length of the score's file name.
  X = 21 if the first letter is lower case, + 25 if the second is, + 25 if the third is;
- initialisation: 36 | 16 + 247 + 48 x J | 16 + 322 + U + I | 16. Here J = index of the fourth `F`, counting
  the command at offset 16 as 0. U = 16 with `WT`; + 12 for a `2BIT` set; for a set of at least 83 230
  bytes + 33, + 33 more if `HfR0` is present, + 16 more if `MP7E` is present as well. I is the sample
  pass below;
- module info: 24; volume: 236; balance: 236;
- sound init: 2634 + V + R + P, where V = 192 with `WT` and speed != 0, else 14.

I (sample pass) = 36 + first pass + 16 + second pass + 16. In each pass the 20 slots are taken in order.
- First pass: o = 0: 43. o > Z: 116. Otherwise 143; for `2BIT` sets + 55 when the mark is present, or 196
  in total when it is absent (the slot becomes empty).
- Second pass: empty slot: 31. Record already converted: 60. Record beyond the file: 126. Converted:
  214 + 26 n, + 8 + 59 x n / 2 in the ST variant, + 4 if the record's end (data start + L) is beyond the
  largest end so far (always for the first). Add 4 if L is odd, for both the beyond-the-file and the
  converted case.

R (stream set-up) = 101 + s0 + 72 + s1 + 75 + s2 + 57 + 75 + 164 + 16 + 12 x (number of times stream 1, 2
or 3 is strictly longer than the longest before it) + the sum over the four streams of (34 + r_v).
- s_v (streams 0..2): 43 per command before the `F`, + 49.
- r_v: per command before the `F`: `V` 150 (+ 24 if its slot is empty), others 87; then 37.

P (P-count scan) = 184 + the sum over the four streams of (18 + 109 x (commands before the `F`) + 4 x
(number of `P` commands) + 34 + (32 if it has a `P`, else 64)).

The first tick is called one period after sound init returns. Example: the `basic` log (a 520-byte score
named `QTS.basic` in a 26-character directory, 6 samples) starts its first tick at clock 73 099.

## 6. Data not in the module

- The empty sample (1.3): 4 zero bytes in chip memory, loop word -1, length 4 bytes.
- The built-in default stream (3): one rest of 10 000 ticks, then the end of the stream.
- The constant 3 579 546 of the period function (4.1).

No player tables are needed; there is no data/ folder.
