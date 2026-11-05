# Mark Cooksey (old player, "MarkCookseyOld", prefix `mco`): behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Eagleplayer "Mark_Cooksey_Old" (player name MarkCookseyOld, a DeliTracker-style player
"adapted by Andy Silva", with DMA-wait and song-end changes for UADE). It plays the 1988 Mark Cooksey
song/sample data on its own; the module file holds no code. The behaviour below was observed with
write logs, beam-read traces and module data, and is written in my own words. It contains no
instructions or code addresses of the original. "cc" means Amiga colour clocks; "cycles" means CPU cycles
(1 cc = 2 cycles).

The repository already has a spec and an open replayer for the newer player ("Mark_Cooksey": engines N-A,
N-B and O, see the markcooksey spec). This format is the data half of that spec's engine O. Section 1
lists the differences. Sections 2-6 are the full description, so a sibling replayer can be written from
this file alone.

A value and timing model written from this text reproduces the reference's write log exactly
(register, value, order and colour clock of every write) for all 16 test runs: 13 real subsongs from
4 modules and 3 synthetic subsongs, 119 652 writes in total (see LOGS.md).

## 1. Differences from the newer player's engine O (`mcr` + `mcs`)

| aspect | newer player, engine O | this player (`mco`) |
|---|---|---|
| files | `mcr.NAME` (driver code and period table) plus `mcs.NAME` (data) | one file, which is byte for byte an `mcs` file. The `mcs.*` files of the archive play unchanged when renamed `mco.*` |
| detection | the `mcr` header with its branch words and the init-code signature | weak header test on the data file (2.1) |
| period table | read from the `mcr` file (Overlander's differs) | fixed 40-entry table inside the player (3.3). Modules made for another table read past its start (3.3) |
| voice registers, volume | volume word AND 0x7F, then host volume/balance scaling | the raw volume word (4 × byte, up to 1020) is written unmasked and unscaled. The player has no volume or balance support |
| command 0x04 | writes AUDxPER | writes AUDxPER = table word and AUDxVOL = the *next* table word in one long write (same cc) |
| note start | DMA off always, LC, LEN, VOL, PER, 9-line wait, DMA on, 9-line wait, LC/LEN silence | only if the channel's DMA is running: DMA off, PER = 2, VOL = 0, 10-line wait, AUDxDAT = 0. Then always LC, LEN, VOL, PER, DMA on, 5-line wait, LC/LEN silence (4.2) |
| line wait | own primitive, 9 iterations | "wait for n changes of the vertical beam counter", n = 10 or 5 (5.3) |
| song end | per-voice flags cleared by stop (0x18) and loop-to-mark (0x34) | reported when all four voice counters are negative ("dead") after a tick (4.4). 0x34 never ends a song |
| initial volume / instrument | — | not reset at subsong start (3.2) |
| init writes | ADKCON = 0x00FF once | none |
| subsongs | subsong range given to the host | no range reported (the host shows 0..0). Subsong change goes through the player's next/previous-song functions, which wrap (2.3) |
| voice order, counters, command set, stack | channel 0..3, counter starts 1, commands 0x00..0x34 | same |

## 2. File format, detection, subsongs

### 2.1 Detection

The player's check works on the file data and its length S:
- byte 0 equals 0x60;
- the long at offset 2 and S agree in their upper 24 bits ((long AND 0xFFFFFF00) = (S AND 0xFFFFFF00)).
  In real files the long at 2 is S - 28 (an Atari-ST-style 0x601A program header whose text size covers
  the whole data), so the test passes only when (S AND 0xFF) >= 28;
- byte 32 is 0 (it is the top byte of the instrument-table offset, see 2.2).
Nothing else is checked: no file-name test, no magic beyond the first byte. This test is weak, so a port
should keep it behind the format's prefix `mco.` or put it after stronger detectors. The newer player's
engine O detector does not accept these files (they lack the branch words at 28..43).

### 2.2 Layout (all big-endian)

The 28-byte header (word 0x601A, long text size, then zeros) is skipped. "D" is file offset 28.
- long at D+0: offset from D of the pattern table (array of signed 16-bit words). Pattern selected by
  byte x = pattern table start + signed word at (pattern table start + x). x is a byte index, so
  pattern k uses x = 2k.
- long at D+4: offset from D of the instrument table (array of longs). Instrument selected by byte x =
  sample record at instrument table start + long at (instrument table start + x) (x = 4k for
  instrument k).
- from D+8: subsong table, 16 bytes per subsong: four longs (voices 0..3), each an offset from D of the
  voice's first track byte. No "absent voice" value exists. An offset of 0 makes the voice read the
  header longs as commands.
- sample record: one word = length in words, then the signed 8-bit sample data (played once, see 4.2).
- Track and pattern data: byte streams of commands (4.3).

### 2.3 Subsongs, title, length

- Subsong count = (L - 8) / 16, where L is the long at D+8 (the subsong table ends where the first track
  begins). The player keeps the highest number, (L - 8)/16 - 1, computed with 16-bit arithmetic.
  Aquablast 2, Overlander 5, A Question of Sport 3, Mike Read's Pop Quiz 3.
- The player reports no subsong range to the host (the reference shows "0..0"). Its next-song function
  increments the number and wraps from the highest to 0. Its previous-song function decrements and
  wraps from 0 to the highest. Each restarts the subsong at once (3.2). A port should expose
  0..count-1 directly.
- No title, author or length is stored. Song end: 4.4.

## 3. Player state and start

### 3.1 Voices

Four voices, channel = voice number. Per voice: a 16-bit counter, a track pointer, a period-table
pointer (byte offset relative to the table centre, 3.3), a volume word, a current sample record, and a
stack (3.4).

### 3.2 Subsong start

For each voice: counter = 1, track pointer = D + subsong table long, stack empty, period-table offset 0
(the centre). No hardware register is written, the audio DMA is not touched, and nothing else is reset:
- the volume word and the current sample record keep their values from the previous subsong. Before
  the first subsong after loading they hold the values of the player's image: volume words 0x0014,
  0x0014, 0x001C, 0x0030 for voices 0..3 (seen in the logs: a note before any volume command writes these
  values). The initial sample record is a stale pointer outside the module. A note before any instrument
  command is therefore undefined; none of the test modules does that.
- Song-end detection uses only the counters (4.4).

### 3.3 Period table

The player has its own table of 40 words:
856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254,
240, 226, 214, 202, 190, 180, 169, 160, 151, 142, 134, 127, 120, 113, 107, 101, 95, 90.
The centre (offset 0) is the third entry (762). A lookup with period-table offset t (signed bytes) and
command byte x (unsigned) reads the word at byte offset t + x from the centre, i.e. entry index
2 + (t + x)/2 (t + x must be even).

Out-of-range indices read whatever lies around the table in the player:
- indices -16..-1 (before the table) are player code words. Their exact values are in
  `data/period_below.bin` (16 big-endian words, index -16 first). Overlander (written for its own
  `mcr` period table) does reach indices -6, -5, -3 and -1 with transposition -24. The reference then
  writes these words as periods (e.g. 0x4E75 for index -1), and a port must use the same values to
  match the log;
- index 40 and up read voice 0's live state (counter, track pointer, ...). This is not reproducible,
  and no test module reaches it.
Command 0x04 also reads the entry after the indexed one (4.3).

### 3.4 Stack

A per-voice byte stack with room for 40 bytes. Pointer entries take 4 bytes (call return, mark), loop
entries take 6 bytes (a 16-bit count on top of a 4-byte loop pointer). Commands do not check the entry
type: "return" and "loop to mark" read 4 bytes from the top, "loop end" reads the count word on top and
the pointer under it. Properly nested data never mixes them. Overflowing the 40 bytes would overwrite
the voice's other state (not modelled, never seen).

## 4. Replay

### 4.1 One tick

Voices are processed in order 0, 1, 2, 3:
1. If the counter has bit 15 set, the voice is dead: skip it.
2. Counter -= 1 (16 bit). If it is not 0: done with this voice.
3. If it is 0: read commands from the track pointer until a command ends the step (4.3). Store the
   pointer.
After the four voices: song-end check (4.4).

Counter values: a duration byte d gives d ticks until the next event. d = 0 makes the voice dead after
two ticks (0 → 0xFFFF, then skipped forever). A rest with a byte >= 0x80 makes it dead at once.

### 4.2 Note start (command 0x00), hardware writes in this order

Let bit = 1 << channel and "R" be the channel's register block.
1. If the channel's DMA is enabled (bit set in DMACONR):
   DMACON = bit (off); AUDxPER = 0x0002; AUDxVOL = 0x0000; wait for 10 line changes (5.3);
   AUDxDAT = 0x0000.
2. AUDxLC = address of the sample record + 2 (high word, low word, same cc); AUDxLEN = the record's
   length word; AUDxVOL = the voice's volume word (raw); AUDxPER = period (3.3);
   DMACON = 0x8000 | bit (on).
3. Wait for 5 line changes (5.3).
4. AUDxLC = the player's silence buffer (a zeroed 4-byte chip buffer of the player; the reference run
   shows it at 0x00759F70, a host-specific address); AUDxLEN = 0x0002.
So the sample plays once, then the channel loops on 2 words of silence.

"DMA enabled" is the DMACON state after the writes so far: it is off after load and after a note-off or
stop of that channel, and on after a note start. The first note of every channel after loading therefore
takes the short path, and so does the first note of a subsong whose previous subsong left the channel
off. Subsong changes in the reference write nothing to DMACON (the next-song function only reinitialises
the voices), so a channel still running from the previous subsong takes the long path.

### 4.3 Commands (one byte, always a multiple of 4)

| byte | args | effect | ends step |
|---|---|---|---|
| 0x00 note | x, d | period = table word (3.3) for x; counter = d; hardware sequence 4.2 | yes |
| 0x04 period | x, c | AUDxPER = table word for x, AUDxVOL = the following table word, both at the same cc (one long write); counter = c (unsigned) | yes |
| 0x08 instrument | x | current sample record = instrument x (2.2) | no |
| 0x0C volume | v | volume word = 4·v (16 bit, unmasked); write AUDxVOL = volume word | no |
| 0x10 rest | n | counter = n sign-extended (n >= 0x80 makes the voice dead) | yes |
| 0x14 note off | n | counter = n (unsigned); DMACON = bit (off) | yes |
| 0x18 stop | — | counter = 0xFFFF (dead); DMACON = bit (off) | yes |
| 0x1C call | x | push return pointer (after x); track pointer = pattern x | no |
| 0x20 return | — | pop the return pointer; period-table offset = 0 | no |
| 0x24 loop begin | n | push loop pointer (after n) and count n | no |
| 0x28 loop end | — | count -= 1 (16 bit); if 0: drop the loop entry and go on; else jump to the loop pointer | no |
| 0x2C call transposed | t, x | push return pointer (after both bytes); track pointer = pattern x; period-table offset = signed t | no |
| 0x30 mark | — | push the current pointer (after this byte) | no |
| 0x34 loop to mark | — | track pointer = pointer on top of the stack (no pop) | no |

A loop with n = 0 runs 65 536 times. Bytes above 0x34 or not a multiple of 4 are invalid (the original
jumps into unrelated code), and no module uses them. The transposition is not stacked: a return always
sets offset 0, also after a plain call inside a transposed pattern.

Values in the logs: periods are table words (up to 856 or a code word, 3.3). Volume words are 4·v, and
values above 64 do occur: the reference writes them as they are and the Paula model decides what they
mean. Command 0x04 writes a period table word as the volume.

### 4.4 Song end

After the four voices of a tick, if all four counters have bit 15 set, the host is told "song end". The
check repeats every tick while it stays true, and the reference stops at the first report: its log ends
with that tick. A song whose voices loop (0x34, or no stop) never ends. Checks (tick numbers from 0, the
first tick played): Overlander subsong 2 at tick 448, 3 at 768, 4 at 368; A Question of Sport subsong 1 at
120; Mike Read's Pop Quiz subsong 1 at 204, 2 at 156; the other test subsongs do not end within 3000
ticks.

## 5. Timing

### 5.1 Clock

The reference emulator charges a fixed number of CPU cycles per instruction (conditional branches
always cost the same whether taken or not) and advances the colour clock by cycles/2, carrying the odd
half. Model: keep a cycle counter H. A write's cc is floor(H/2) at the moment the write completes. A
beam read samples line = floor(cc/227) at the end of the reading instruction (lines of 227 cc,
counted from clock 0; only the change of line matters).

### 5.2 Tick entry

The host calls the player every 70 940 cc. In the reference the first tick enters at 71 761 cc for
subsong 0: the start call ends at 821, and the first tick comes one period later. Each next-song call made
before playing (one per subsong number) adds 621 cc, so subsong s enters at 71 761 + 621·s, then
+70 940 per tick. H at entry = 2 × entry cc. These host constants belong to the framework. The costs
below are the player's.

### 5.3 Costs in cycles

Add these to H in order. "W" marks a write, stamped after adding the number before it.

- tick prologue (entry to the start of voice 0): 247.
- between two voices: 16.
- voice visit, dead: 57. Counting down (counter not 0): 84. Event: 84, then for each command read,
  36 + the command's cost:

| command | cost (cycles) |
|---|---|
| volume | 32 W(VOL), then 22 |
| instrument | 74 |
| call | 100 |
| call transposed | 152 |
| return | 78 |
| loop begin | 74 |
| loop end | 63 if it jumps back, 71 if it exits |
| mark | 54 |
| loop to mark | 38 |
| rest | 66 (ends the visit) |
| note off | 44 W(DMACON), then 42 |
| stop | 40 W(DMACON), then 80 |
| period | 60 W(PER and VOL, same cc), then 66 |
| note | 103, then the sequence below |

Note sequence (cycles before each write):
- running channel only: 20 W(DMACON off), 16 W(PER=2), 16 W(VOL=0), line wait n=10 with lead-in 46,
  then 16 W(DAT=0);
- always: 40 W(LC high and low, same cc), 12 W(LEN), 20 W(VOL), 16 W(PER), 32 W(DMACON on), line
  wait n=5 with lead-in 56, 28 W(LC silence, both words), 16 W(LEN=2), then 42 to the end of the visit.

Line wait (n changes, lead-in L): add L. Then n times: add 16 and sample the line (reference). Then
repeatedly add 16 and sample again, then add 9, until the sample differs from the reference. Then add 10.
After the n-th change add 44. The time to the write that follows is the number listed after the wait.
Each change waits for the next line boundary after the previous one (the first change waits for the
boundary after the start). The 10-line wait therefore lasts 9 lines plus the part of a line left at its
start, plus overhead: 2133..2333 cc in the logs, from the VOL=0 write to the DAT write. The 5-line wait
lasts 1029..1167 cc from the DMACON write to the silence LC write.

Typical gaps in cc (from the logs, these follow from the table): LC→LEN 6, LEN→VOL 10, VOL→PER 8,
PER→DMACON 16, silence LC→LEN 8, DMACON off→PER 8→VOL 8, DAT→LC 20. A volume command followed directly
by a note: 100/101 cc from the VOL write to LC.

## 6. Validation

The model built from sections 2-5 (and `data/period_below.bin`) matches every write (address, value,
cc) of all 16 reference runs (13 real subsongs: Aquablast 0-1, Overlander 0-4, A Question of Sport 0-2,
Mike Read's Pop Quiz 0-2; up to 3000 ticks each or to song end; plus 3 synthetic subsongs that cover
commands 0x04, 0x10, 0x14, 0x18, 0x24/0x28, 0x30/0x34, 0x1C/0x2C/0x20, initial volumes, duration 0 and
negative rests). The repository's replay-cmp tool reports "no difference" for them. Not covered: indices
past the end of the period table, invalid commands, stack overflow, and notes before an instrument
command (all undefined in the original).
