# Mark Cooksey replayer: behaviour spec

> **Sanitised for publication.** This copy was reviewed before it was added to the repository. Removed or rewritten: assembler mnemonics in the description of the address-load scan, and scratch paths / database references in LOGS.md. Numbers, formulas and behaviours are unchanged. References to `logs/` files, scripts and traces name data that is not shipped (see `docs/specs/README.md`).

Scope: the Amiga music format of Mark Cooksey / Richard Frankish (1988-1990 game music, file name
prefixes `mc.` and `mcr.`/`mcs.`) as played by the reference (UADE player "Mark_Cooksey", i.e. the
Eagleplayer wrapper plus the 68k driver that is part of every module file). This is a description of
observed behaviour (write logs, beam-read traces, module data) in my own words. It contains no
instructions or addresses of the original. Numbers in "cc" are Amiga colour clocks (1 cc = 2 CPU cycles).

A value model written from this description reproduces the hardware write sequence (register, value,
order) of the reference exactly for 59 (module, subsong) runs of about 1500 ticks each (about 41 400
ticks, 58 of 59 runs exact to the end; the 59th differs only in the host's end-of-song tail), and for the
old engine 10 runs / about 2 600 ticks. Timing in cc is specified for the note-start sequences and the
wait primitives; see section 7 for what is and is not covered.

## 1. Three engines, two file kinds

| name here | files | header | engine | modules seen |
|---|---|---|---|---|
| N-A | one file `mc.NAME` | long 0xD040D040 | "new", variant A | Commando, Paperboy, Ghosts n Goblins, Grand National (title, ingame) |
| N-B | one file `mc.NAME` | long 0xD040D040, 5th branch slot present | "new", variant B (1990) | Gremlins 2, Mighty Bomb Jack, World Championship Soccer |
| O | two files `mcr.NAME` (code + tables) and `mcs.NAME` (song data + samples) | word 0x601A | "old" (1988) | Aquablast, Live and Let Die, Mike Reads Comp Pop Quiz, Overlander, A Question of Sport |

A third header kind is accepted by the Eagleplayer (file starting with a 0x6000 branch pair, then a
PC-relative address load and a reference to the custom chip base). No file of that kind was available
(the `fw.*` files in the same game directory are another format, "FWMP"); it is not specified here.
A second, older Eagleplayer ("Mark_Cooksey_Old", prefix `mco`) was not examined.

N-A and N-B are one engine with a different set of entry points and a hardware-init entry (N-B only).
Everything in sections 3-5 holds for both unless a variant is named. Engine O is described in section 6.

## 2. Detection, loading, info

### 2.1 Detection (what the Eagleplayer checks)

N (either variant), on the first bytes of the file:
- long at 0 equals 0xD040D040; word at 4 equals 0x4EFB (a jump through an index; the first four
  bytes make the code at offset 0 harmless);
- words at 8, 12, 16 and 20 each equal 0x6000 (branch slots, 4 bytes each);
- accepted if the word at 40 equals 0x43FA (variant A); otherwise the word at 24 must also be 0x6000
  and the word at 150 must equal 0x43FA (variant B).
Variant A has four branch slots (8..20), variant B has five (8..24). The word at 6 is 0x0002 in A and
0x0006 in B (it shifts the dispatch base: A's slot 0 is at 8, B's slot 0 is at 12, see 3.1).

O, on the `mcr` file: word 0 equals 0x601A; long at 2 is non-negative and even; word at 6 is 0; the
five longs at 8..27 are zero; four 0x6000 branch words follow at 28, 32, 36, 40, each with a
non-negative even displacement; the code at the target of the first branch begins with the long
0x48E780F0. File name prefix: `mcr.`; the sample/song file is the same name with `mcs.` (see 2.2).

### 2.2 Loading

N: one file, loaded as is, at an arbitrary address. All internal pointers in the file are relative
(see 4.1), the code is relocatable. The reference runs the module's own code; a port reimplements it.

O: `mcr.NAME` is the module; the Eagleplayer loads `mcs.NAME` from the same directory (the third
character of the prefix, case-insensitive R, is replaced by S; a name that does not start with
M C R `.` fails with "could not load additional files"). The `mcs` file has the same 28-byte 0x601A
header (word 0 = 0x601A); its content after the 28-byte header is the song/sample data of 6.1.

### 2.3 Subsongs, title, length

- N: subsong count = (distance in bytes between the subsong table and the following table)/16 (4.1).
  Commando 10, Paperboy 4, mc.title 1, mc.ingame 4, Ghosts n Goblins 15, Gremlins 2 8, World Champ.
  Soccer 5, Mighty Bomb Jack 12.
- O: subsong count = (L - 8)/16 where L is the long at offset 8 of the data region of the `mcs` file
  (the data region starts after the 28-byte header). Aquablast 2, Overlander 5, A Question of Sport 3.
- Subsongs are numbered from 0. There is no title string and no author string in the files (the
  reference shows the player name as author). No length is stored; a "length" shown by the reference
  for engine O is a count of the first voice's track bytes divided by 4 (display only); N shows none.
- Song end: see 4.9 and 6.4.

## 3. Host interface (what the reference does around the driver)

### 3.1 Calls into the driver (N)

The driver has a small dispatch at the file start: the entry number is multiplied by four and added to
a base (8 in A, 12 in B) to reach a 4-byte branch slot. Entries used by the player:
- entry 0: play one tick (called with D0 = 0 each tick);
- entry 1: start subsong (D0 = subsong number in the upper 16 bits, 1 in the lower);
- entry 3 (A) / entry 4 (B): set master volume word (unused by the replay path, see 4.7);
- B only, one extra branch slot at offset 8: hardware initialisation, called once when the player is
  loaded (before any subsong start), see 3.3.
The other slots (a second start routine for a sound-effect voice set, used by games, not by the
player) are never called; the sound-effect voice set (4.2) therefore stays inactive.

### 3.2 Tick and timing

The reference calls the play entry from a timer interrupt at a nominal 70 940 cc per tick (50 Hz CIA
timer). The moment of the first write inside a tick relative to the interrupt shifts slightly with the
raster waits; successive first-write times in the logs differ by 70 940 +- 130. Tick-to-tick spacing and
start phase belong to the host framework, not to this spec. A tick may write nothing at all (idle
voices); the logs then simply have no group for that tick.

### 3.3 Hardware writes that are not part of a tick

- N-B, at player load (before the first tick), in this order, about 6-8 cc apart: AUDxLEN = 0x007F for
  channels 0,1,2,3; AUDxPER = 0x0001 for channels 0..3; AUDxVOL = 0x0000 for channels 0..3; ADKCON =
  0x00FF; DMACON = 0x820F (all four audio channels on). N-A and O have no such writes except O's ADKCON
  = 0x00FF written once at init.
- Subsong start for a subsong other than the first one played: the host first stops the sound: DMACON =
  0x000F, then AUDxVOL = 0 for channels 0,1,2,3 (6 cc apart). The audio DMA state after this is "all off";
  for N-B it is therefore "off" before the first tick of such a subsong (and "on" for the first subsong
  played right after load). The first-tick note-start writes depend on this (4.6, first group).
- Volume/balance and voice-mask settings of the host scale every audio volume write (4.7). Muting a
  voice makes its volume writes 0.

## 4. Engine N: the replay

### 4.1 Module data and where it is

The module consists of driver code, tables, the song data and the samples. The tables are addressed from
the code with position-independent address loads (opcode words 0x41FA/0x43FA/0x45FA/
0x47FA followed by a 16-bit displacement; target = address of the displacement word + signed displacement). The Eagleplayer finds the
subsong, instrument and sample tables by scanning for these; the same occurrences give the others. For
all eight test modules the following holds (occurrence order in the play routine, i.e. the code reached
through the entry-0 branch slot, listing address loads in file order):

| occurrence | variant A | variant B |
|---|---|---|
| 1st | voice state block | voice state block |
| 2nd | sample area base | pattern table |
| 3rd | pattern table | instrument table |
| 4th | instrument table | sample area base |
| 5th | sample area base | volume curve table (not needed, see 4.7) |
| 6th | period table | period table |

The subsong table is the target of the first 0x43FA load found when scanning the file from offset 2
(it lies in the start-subsong routine); the table after it (target of the next 0x43FA) belongs to the
sound-effect voice set. Layout facts (hold for all test modules, useful as checks): the period table is
directly followed by the voice state block (A) or by the 256-byte volume curve table and then the
state block (B); the period table has 384 16-bit entries (768 bytes); the pattern table ends where the
period table begins; the sample area starts with at least 64 zero bytes.

Structures (all values big-endian):
- Subsong table: 16 bytes per subsong, four longs (one per voice 0..3). Each long is an offset relative
  to the address of that long; value 0 = voice absent; else track pointer = address of the long + value.
- Pattern table: array of signed 16-bit offsets; pattern n starts at (address of entry n) + entry n.
- Instrument table: array of signed 16-bit offsets; instrument n record at (address of entry n) +
  entry n. Record layout (30 bytes):
  | offset | size | meaning |
  |---|---|---|
  | 0 | long | attack sample start, offset from the sample area base; 0 = none |
  | 4 | long | loop sample start, offset from the sample area base; 0 = none |
  | 8 | long | volume envelope: offset relative to this field; 0 = none |
  | 12 | long | pitch (vibrato) table: offset relative to this field; 0 = none |
  | 16 | word | attack sample length in words |
  | 18 | word | loop sample length in words |
  | 20 | word | period scale for attack sample |
  | 22 | word | period scale for loop sample (used only when there is no attack sample) |
  | 24 | byte | envelope step interval (ticks) |
  | 25 | byte | pitch table step interval (ticks) |
  | 26 | word | instrument volume (full scale 255 in Commando; up to 0x100) |
- Period table: 384 unsigned words T[i], four entries per semitone, read from the module (it is data).
  It follows 65536*2^(-(i+12)/48) to within 1 for 366 of 384 entries (a few are hand-adjusted), e.g.
  55109, 54314, 53540, 52772 ... 228, 224, 221, 218 (last entry). Index i = 4*note (+0..3 fractional
  steps).
- Sample area: raw signed 8-bit samples, started by silence (the first bytes are 0).
- Envelope data: bytes, one per envelope step; value 0 ends the envelope. Pitch table data: signed
  bytes, one per step; value -128 ends it.
- Track data: byte stream per voice, commands in 4.4.

### 4.2 Voice state

Eight voice slots exist; slots 0-3 are the music voices (channel = slot), slots 4-7 the sound-effect
voices (always inactive here, their track pointer is 0). Each tick the sound-effect slots are visited
first (channel 3,2,1,0) and cost time but write nothing; then the music voices in channel order
3,2,1,0. A music voice's registers are only touched when no sound-effect voice owns the channel (always
true here).

Per music voice: track pointer (0 = inactive); 16-bit wait counter (starts 0); stack of sub-pattern
frames (10 bytes each: return pointer, pattern start pointer, 16-bit repeat count; it grows downward
inside the voice's own state block, a nesting deeper than 3 would overwrite other fields and is not
modelled); transposition (signed 16-bit, starts 0); track volume (byte, starts 0); portamento flag
(off) and portamento length; current instrument fields (sample pointers, lengths, scales, envelope
pointer and counter, pitch table pointer and counter, instrument volume); `pitch` (current table
index), `target` (index of the last note), `start` (index at note start / glide start), `slide` (signed
step in 1/16 index per tick); `loopmode` (true when the note has no attack sample).

### 4.3 One tick

For each music voice in order 3,2,1,0:
1. Decrement the 16-bit counter. If it was 0 or 1 before (result 0, or a borrow from 0): this is an
   event step. Otherwise it is a maintenance step (go to 3).
2. Event step: if the voice has no track pointer, nothing happens for it (no maintenance either).
   Otherwise read commands and notes from the track (4.4) until one of them ends the step:
   - a note ends the step normally (continue with 3);
   - a rest (0x60) or a stop (0x67) ends the step without maintenance (skip 3).
3. Maintenance (also done for a voice that has stopped, until its counter wraps; see below), in this order:
   a. envelope (4.7),
   b. portamento slide (4.8),
   c. pitch table / period (4.8).
The counter keeps counting even for a stopped voice (a stopped voice's counter is 0 or 0xFFFF, so it
counts down from 0xFFFF; maintenance runs for those ticks too, which lets an unfinished envelope keep
writing volumes after a stop; this was seen in the logs). The event step of a stopped voice is a no-op.

### 4.4 Track bytes

A byte b: if (b AND 0x7F) < 0x60 it is a note (below). Otherwise command number c = (b AND 0x7F) - 0x60:

| c | byte | arguments | effect |
|---|---|---|---|
| 0 | 0x60 | duration word (hi, lo), 1 unused byte | rest: counter = duration; write AUDxVOL = 0 (raw, not scaled); ends step, no maintenance |
| 3 | 0x63 | byte v | track volume = v; continue reading |
| 4 | 0x64 | pattern byte p, transposition byte t | call pattern p once (see below); continue reading in the pattern |
| 5 | 0x65 | p, t, repeat byte r | call pattern p, r+1 times in total |
| 6 | 0x66 | - | end of pattern: if the frame's repeat count (decremented) is >= 0 jump to the pattern start, else pop the frame, continue after the call, transposition = 0 |
| 7 | 0x67 | - | stop voice: track pointer = 0; hardware: sample pointer = start of sample area, length = 0x20 words, AUDxVOL = 0 (raw); ends step, no maintenance; song-end event (4.9) |
| 8 | 0x68 | signed word d | jump: new pointer = address of this command byte + d; continue reading; song-end event (4.9) |
| 9 | 0x69 | byte n | portamento on, length n ticks |
| 11 | 0x6B | - | portamento off |
| others | | | not used by any module; the reference would loop on them forever |

Call: transposition = signed t; a frame is pushed (return pointer = after the arguments, pattern start,
repeat count r, 0 for 0x64); the track pointer becomes the pattern start. Pattern start = address of
pattern table entry p plus that entry. The transposition stays in force until the matching 0x66 pops
the frame (not restored to the outer value, just cleared to 0).

Note (byte b, bits 0..6 below 0x60): note index n = (b + transposition) mod 65536; table index
idx = 4*n mod 65536. Arguments follow: instrument byte i, duration byte d, one unused byte.
- Without portamento: start = pitch = target = idx, slide = 0.
- With portamento: start = pitch = previous target (the pitch the previous note ended at), target = idx,
  slide = trunc((idx - start) * 16 / portamento length) (signed division, toward zero).
- counter = d (a duration of 0 makes the voice dead for 65535 ticks: next tick the counter is 0xFFFF).
- Load the instrument record (4.1); envelope and pitch-table counters = 0 (so both step on the first
  tick of the note); `loopmode` = (attack sample is 0).
- track volume level L0 = ((instrument volume * track volume) AND 0xFFFF) >> 8.
- Hardware sequence (4.6). Then maintenance runs in the same tick (envelope may overwrite the volume
  written by the note start in the same tick, and the period write follows when due).

### 4.5 Scalings

Volume written to AUDxVOL (all volume writes except rest/stop which write 0):
`hw = min(64, level >> 1)` where `level` is L0 or the envelope level (4.7); then the host's volume and
balance scaling `((hw AND 0x7F) * gain >> 6) AND mask`, gain 64 and mask 0xFFFF by default (identity),
gain per channel = left volume for channels 0 and 3, right volume for channels 1 and 2, mask 0 for a
muted voice.
Note: the reference replaces the module's own "level >> 2" scaling (and in B the master-volume curve
table) with this halving; that is the behaviour to reproduce.

Period written to AUDxPER: `P = (T[index] * scale) >> 16` truncated to 16 bits, where scale = attack
scale unless `loopmode`, in which case it is the loop scale (only used when there is no attack sample).

### 4.6 Hardware write sequence of a note (per voice, exactly this order)

Let ch be the channel, "R" the register block of channel ch. Writes of a long address count as two
16-bit writes (high word first, same cc).

1. AUDxVOL = vol(L0) (4.5).
2. If the instrument has no attack sample (`loopmode`): AUDxLC = loop sample address (sample area base +
   the loop offset), AUDxLEN = loop length, AUDxPER = P (period computed with the loop scale, 4.5).
   End of sequence. (No test module exercised this branch with a non-trivial case; the order LC, LEN,
   PER is taken from the module code's behaviour and matched in the logs where it occurred.)
3. Otherwise (attack sample):
   a. wait for the start of a raster line, then until the beam is past colour clock 22 of that line
      (primitive W in 7.1);
   b. AUDxPER = 0x0001;
   c. INTREQ = (running ? (1<<(7+ch)) : 0), then DMACON = (running ? (1<<ch) : 0), where `running` is
      "the channel's DMA bit is currently set in DMACONR"; then wait until the channel's audio
      interrupt flag is set again if it was running (nothing to wait for otherwise);
   d. AUDxLC = attack sample address, AUDxLEN = attack length, AUDxPER = P;
   e. wait for the start of a raster line, then until beam colour clock >= 22 (primitive W);
   f. INTREQ = 1<<(7+ch), DMACON = 0x8000 | (1<<ch) (starts the channel);
   g. wait until the channel's audio interrupt flag is set, then W once more;
   h. AUDxLEN = loop length, AUDxLC = loop address (if the instrument has a loop sample), otherwise
      AUDxLEN = 0x0020 and AUDxLC = sample area base (the silence block), in that order for the loop
      case (length, then address) and the silence case (length, then address).
   The interrupt-enable register is not written by the reference (the module's INTENA writes are removed).

Example (Commando subsong 0, tick 0, channel 3): VOL 0x25, PER 0x0001, INTREQ 0x0000, DMACON 0x0000,
LC hi/lo, LEN 0x06B3, PER 0x00FF, INTREQ 0x0400, DMACON 0x8008, LEN 0x0020, LC hi/lo (silence).

### 4.7 Envelope

If the voice has an envelope pointer: decrement the 8-bit envelope counter; if it was 0 or 1 before:
reload it with the instrument's envelope interval; take the next envelope byte e. If e = 0 the envelope
pointer becomes 0 (ended, no write). Otherwise advance; level = ((((e + 1) * track volume) AND 0xFFFF) *
instrument volume) >> 16 (16-bit result); write AUDxVOL = vol(level) (4.5). The counter is 0 after a note
start, so the first tick of a note steps the envelope at once. A voice with a stopped track keeps stepping
its envelope until it ends.

### 4.8 Slide, pitch table, period write

Slide: if `slide` is not 0: v = ((pitch << 4) AND 0xFFFF) + slide, 16-bit; if bit 15 set, v = 0; idx =
v >> 4. If slide > 0: if idx >= target (unsigned) then pitch = target, slide = 0, else pitch = idx. If
slide < 0: if idx <= target then pitch = target, slide = 0, else pitch = idx.

Pitch table: if the voice has a pitch-table pointer: decrement the 8-bit counter; if it was 0 or 1:
reload with the instrument's pitch interval; take the next signed byte b. If b = -128 the pitch table
ends (pointer 0), fall through to the plain rule below. Otherwise advance and write the period for index
(pitch + b) (not stored). If the counter did not step, fall through to the plain rule.

Plain rule: if `start` differs from `target` (a glide has been set up at some time since the note; this
stays true after the glide has finished) write the period for index `pitch` (so a voice that played a
portamento note re-writes its period every tick); otherwise no period write.

Period write = AUDxPER = P for the chosen index (4.5). Maintenance writes come in the order: envelope
volume, then the one period write.

### 4.9 Song end

Each of the four voices has a flag, all set at subsong start. A stop command (0x67) or a jump command
(0x68) executed by a voice clears that voice's flag. When all four flags are clear, the host is told
"song end" once, and all flags are set again. A voice that has no track at all never clears its flag, so
a subsong using fewer than four voices never reports a song end. The reference stops playing a few ticks
after the song-end report (its log ends within 2-4 ticks).
Checks against logs: Commando subsong 0 reports at tick 576 (log ends tick ~580), subsong 5 at tick 504,
Mighty Bomb Jack subsong 10 at tick 216, Paperboy subsong 3 at tick 184. Other test subsongs loop through
1500 ticks without reporting.

## 5. Differences A / B that matter

- B has the hardware-init call (3.3) and its DMA state starts "on"; A's starts "off".
- B and A differ in the straight-line costs between writes (section 7), not in values.
- In B the initial state of the portamento flag and transposition are explicitly cleared at subsong
  start; in A they are zero because the state block is zero. Same result.

## 6. Engine O (1988): `mcr` code + `mcs` data

### 6.1 Data

`mcs` (after its 28-byte header; "D" = start of that region):
- long at D+0: offset from D of the pattern table: a table of signed 16-bit offsets, entry x (byte
  index) gives a pattern start = pattern table start + signed word at (pattern table start + x);
- long at D+4: offset from D of the instrument table: a table of longs, entry at byte index x gives a
  sample record at (instrument table start + long at (instrument table start + x));
- from D+8: subsong table, 16 bytes per subsong, four longs (voices 0..3): offset from D of the voice's
  first track byte (the subsong table ends where the first one-voice track begins; subsong count 2.3);
- sample record: first word = length in words, then the signed 8-bit sample bytes (looped by the player
  only through the silence buffer, i.e. the sample plays once).
`mcr`: the code and (read from the file) the default period table: words of the usual ProTracker period
table (856, 808, 762, 720, 678, ... 113); the "centre" the player indexes from is the entry 762 (the third
entry) so that byte indexes may be negative after a transposition (the two entries before it, 856 and
808, are reachable). The table pointer is found like the others from the position-independent load that
the init routine (first branch slot) uses first for a 0x45FA address-load opcode. The reference uses
a 4-byte host buffer as silence loop (its address appears in the logs as 0x0075A630 in this setup; any
4 zero bytes will do).

### 6.2 Voice state and tick

Four voices, processed channel 0,1,2,3 each tick: 16-bit counter (starts 1), track pointer, stack
(return pointers and loop frames), default period table pointer, period table pointer (starts at
default), current volume word, current sample record pointer. A voice with a negative counter (bit 15
set) is skipped. Otherwise counter -= 1; if non-zero, nothing else. When zero: read commands until
one ends the step.

### 6.3 Commands (byte value; always a multiple of 4)

| byte | arguments | effect |
|---|---|---|
| 0x00 note | period byte index x, duration byte d | period = word at (period table pointer + x); counter = d; hardware (6.5); ends step |
| 0x04 period | byte x, counter byte c | write AUDxPER = word at (period table pointer + x); counter = c; ends step |
| 0x08 instrument | byte x | current sample record = record addressed by instrument table entry x; continue |
| 0x0C volume | byte v | volume word = 4*v (16-bit); write AUDxVOL = (4*v AND 0x7F) scaled by host volume (as 4.5) ; continue |
| 0x10 rest | byte n | counter = signed n (sign-extended: n >= 0x80 makes the voice dead); ends step |
| 0x14 note off | byte n | counter = n (unsigned); write DMACON = channel bit (DMA off); ends step |
| 0x18 stop | - | counter = 0xFFFF (dead); write DMACON = channel bit; clear the voice's active flag; song-end event; ends step |
| 0x1C call | byte x | push return pointer; track pointer = pattern table start + signed word at (pattern table start + x); continue |
| 0x20 return | - | pop return pointer; period table pointer = default; continue |
| 0x24 loop begin | byte n | push loop start pointer (after this byte), push count n; continue |
| 0x28 loop end | - | count -= 1; if 0 pop both and continue; else track pointer = loop start; continue |
| 0x2C call with transposition | byte t, byte x | push return pointer; pattern as 0x1C with index x; period table pointer = default + signed t (bytes); continue |
| 0x30 mark | - | push the current track pointer; continue |
| 0x34 loop to mark | - | track pointer = top of stack (no pop); song-end event; continue |

Notes about stack: loop/mark/call frames share one stack; 0x20 pops a return pointer only, 0x28 pops a
count and a pointer.

### 6.4 Song end (O)

Per-voice flags as in 4.9, cleared by 0x18 (stop) and 0x34 (loop) executed by that voice; the same
"all four clear => report once, set all again" rule.

### 6.5 Hardware sequence of a note (O), exactly in this order

DMACON = channel bit (stop); AUDxLC = sample record address + 2 (hi, lo; same cc); AUDxLEN = record
length word; AUDxVOL = vol (volume word AND 0x7F, host-scaled); AUDxPER = period; then 9 raster-line
waits (primitive D, 7.3); DMACON = 0x8000 | channel bit; 9 raster-line waits again; AUDxLC = host
silence buffer (hi, lo); AUDxLEN = 0x0002. Ends the voice's step.

## 7. Timing in colour clocks (as measured; "gap" = cc from the previous write)

### 7.1 Clock and beam

The reference emulator charges fixed 68000 cycle counts per instruction and advances the clock by
cycles/2, carrying the odd half (so gaps of 10.5 cc alternate 10, 11). The beam position at clock c is
line = floor(c/227), h = c mod 227 (lines of 227 cc, no offset). A beam read returns line and h at the
instruction's time.

Primitive W (N engine, "wait raster line start, then h >= 22"): first beam read at t; second read 6 cc
later; then reads every 10.5 cc until a read shows a different line than the first read ("exit read");
then the first h-read 12 cc after the exit read; further h-reads every 12.5 cc (25 cycles) until a read
has h >= 22 ("last h-read"). The write after the wait comes a fixed time after the last h-read (below).

Interrupt-flag poll (N): when the sequence waits for the audio interrupt flag of a channel, the loop
period is 14.5 cc; the flag is set by the audio hardware, so the wait is whatever the Paula model gives.
Observed gaps after the DMACON "stop" write (running channel): 164 or 165, 179, 193/194, 208, 222/223,
237, 251, 266, 280, 295, ... = 150 + 14.5*k (k poll iterations, k >= 0; 150 when nothing to wait for).
After the DMACON "start" write the flag is found at the first poll for channels 3 and 2 (165 cc to the
next beam read) and at the second poll for channels 1 and 0 (180 cc).

### 7.2 N engine, per-channel constants of a note start with attack sample (cc)

Measured from beam-read traces of the first tick (every channel starts a note), A = Commando, B = Mighty
Bomb Jack (channel 3 only has a rest in B's first tick; channel 3 B values are taken from later logs).

Segment lengths:

| segment | ch3 | ch2 | ch1 | ch0 |
|---|---|---|---|---|
| A: VOL write (4.6 step 1) to first beam read | 193 | 189 | 174 | 158 |
| B: same | - | 226 | 210 | 195 |
| last h-read of wait 1 to PER=1 write | 12 | 12 | 12 | 12 |
| PER=1 to INTREQ write | 26 | 26 | 26 | 26 |
| INTREQ to DMACON | 6 | 6 | 6 | 6 |
| DMACON (stop) to LC write, nothing to wait for | 150 | 149 | 150 | 150 |
| A: LC to LEN | 95 | 92 | 76 | 60 |
| B: LC to LEN | - | 92 | 76 | 61 |
| A: LEN to PER (attack) | 158 | 153 | 138 | 123 |
| B: LEN to PER | - | 125 | 110 | 94 |
| A: PER to first beam read of wait 2 | 103 | 100 | 84 | 68 |
| B: same | - | 100 | 84 | 69 |
| last h-read of wait 2 to INTREQ write | 37 | 37 | 37 | 37 |
| INTREQ to DMACON(start) | 6 | 6 | 6 | 6 |
| DMACON(start) to first beam read of wait 3 | 165 | 165 | 180 | 180 |
| last h-read of wait 3 to next write, loop sample present (LEN) | - | 75 | 75 | 75 |
| ... then LEN to LC | - | 100 | 84 | 68 |
| last h-read of wait 3 to next write, no loop sample (LEN 0x20) | 63 | - | - | - |
| ... then LEN to LC | 14 | - | - | - |

(Entries marked - were not exercised by the traced tick; the "loop sample present" row was taken from
channels 2, 1, 0 of the traced tick, the "no loop sample" row from channel 3.) The wait times themselves
come from primitive W with the exit conditions of 7.1; consecutive waits start 6 cc after a write.

A note start without attack sample (4.6 step 2): VOL write, then 3 writes (LC, LEN, PER); gaps not
measured separately.

After a note start, the same voice's maintenance continues: the following write (envelope volume or
period) comes 300-460 cc after the last write of the sequence; the next voice's first write follows at
about 740-1130 cc (they depend on the number of commands read).

### 7.3 Steady-state gaps (N engine, maintenance only)

Gap between consecutive writes of the same tick when no voice reads track bytes, keyed by what the two
voices wrote and which voices were passed in between (examples, all exact):
- env volume ch1 then env volume ch0, nothing between: 417/418 (A).
- env volume ch2 then ch1: 441/442 (A); ch3 then ch2: 449 (A).
- env volume ch2, one plain voice between, then ch0 env volume: 526 (A); then period write of ch0 (pitch
  table stepped): 457 (A); ch3 then ch1 with one plain voice between: 539 (A); one envelope-waiting
  voice between instead of a plain one: 538.
- B: env volume ch2 -> ch1 538, ch1 -> ch0 514; period after volume of the same voice 307 (ch2), 292
  (ch1), 277 (ch0); env volume ch3 -> ch2 -> ch1 -> ch0 etc. as in the timeline file.
Full per-write gap tables for several modules are in logs/*.gaps.tsv; per-path costs for command
parsing and the other maintenance combinations were not extracted (see README).

### 7.4 Engine O timing

Constants in cc (from write logs of Aquablast, Overlander, A Question of Sport; Overlander and the
others agree):

| segment | ch0 | ch1 | ch2 | ch3 |
|---|---|---|---|---|
| DMACON stop to LC write | 18 | 18 | 18 | 18 |
| LC to LEN | 56/57 | 72 | 87/88 | 91/92 |
| LEN to VOL | 141 | 165 | 184 | 201/202 |
| VOL to PER | 72/73 | 88 | 103/104 | 107/108 |
| PER to first beam read of wait D1 | 79 | 95 | 110 | 114 |

Primitive D (raster-line wait of O): first beam read at t; second read 8 cc later; further reads every
10.5 cc until a read shows a different line than the first read; the next iteration's first read is 18 cc
after that exit read; 9 iterations in total. After the 9th exit read: 47 cc to the DMACON(start) write
(first wait) and 44 cc to the LC write (second wait). From DMACON(start) to the first beam read of the
second wait: 31. The first wait lasts 1964-2195 cc, the second 2036/2037/2049 cc (depends on the line
phase). LC (silence) to LEN: 8 cc.
A volume command writes AUDxVOL about 140-420 cc after the previous write; the first write of a tick
depends on the host phase. Example start of a tick with two volume commands: 398 between them (Aquablast).

## 8. Validation summary

See LOGS.md. Values/order: N 58 of 59 (module,subsong) runs exact for the whole log (the exception is
the reference's end-of-song tail), O 10 of 10. Timing: the segments above were checked against the
beam-read traces; a complete cycle model for all voice-state combinations was not built.
