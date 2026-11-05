# Howie Davies ("hd.*") - behaviour specification

*Sanitised for publication: behaviour only.*

Format of Gary Antcliffe and Howie Davies, used for Hi-Tec / PAL Developments
games. Player name "Howie Davies", file prefix `hd.`, slug `howiedavies`.
This document is written for an implementer who has never seen the original
player. Everything here was measured on the reference 68000 player run through
the host with DELI_WRITE_LOG (see LOGS.md); numbers in sections 10 and 11 are
the authority for timing.

## 1. What the music is

A module is a complete Amiga executable (HUNK format) that contains the game's
own sound driver, the music data and the samples. The replay routine is a
small step sequencer driving the four Paula voices directly with raw sample
pointers (no mixing, no vibrato, no portamento). One interrupt per frame
(70940 colour clocks) runs two routines: first a "DMA-on" routine, then the
"tick" routine. Music advances one **step** every S ticks (S, the speed,
starts at 6 and is changed by pattern commands). A note is a period value, a
duration, and an instrument (sample, loop, start volume, envelope). The
envelope is a list of (volume delta, tick count) pairs; every envelope in the
three known modules is the single "no change forever" pair, so the volume is
simply the instrument volume.

There are three driver variants (A, B, C) that differ in small details; they
are all described together, differences are marked. The known modules:

| variant | module | subsongs |
|---|---|---|
| A | Blazing Thunder | 1 |
| B | Future Bike Simulator | 3 |
| C | Yogi's Great Escape | 8 |

(There are no other `hd.` modules in the archive. The archive's other
Howie Davies files, Insector Hecti and Yogi's Big Clean Up, are different
formats with other prefixes and are not covered.)

## 2. File format

### 2.1 Container

An Amiga HUNK executable with exactly one code hunk loaded into chip memory
(the memory flag is set in the size long), followed by a relocation hunk and
the end hunk:

- long 0x000003F3, long 0, long 1 (table size), long 0, long 0,
  long (size in longs | 0x40000000);
- long 0x000003E9, long (size in longs), then the code image (file offset
  0x20);
- long 0x000003EC relocation hunk (hunk 0 only), then 0x000003F2.

All 32-bit pointers inside the image are stored **hunk-relative** (offset from
the start of the code image). The relocation hunk lists the longs to which the
load address must be added. After loading, a pointer is `base + stored value`.
Where this document gives "offset", it means hunk-relative offset (equal to
file offset - 0x20). Null pointers (stored 0) are not relocated.

### 2.2 Detection

A file is an `hd.` module when:

1. the first long is 0x000003F3, and the byte at file offset 0x14 is non-zero
   (the memory flag);
2. the long at file offset 0x20 is 0x70FF4E75 (the image starts with a
   "return -1" stub, so a caller treating it as code fails safely);
3. followed by the text `H.DAVIES` (0x482E4441, 0x56494553) at offset 0x24;
4. followed by three non-zero longs (header pointers 0, 1, 2 below).

### 2.3 Header (hunk offsets)

Image offset 0x00: the 4 stub bytes, 0x04: `H.DAVIES`, then longs from 0x0C
(each a hunk-relative pointer unless noted):

| offset | meaning |
|---|---|
| 0x0C | tick routine (called once per interrupt, after the DMA-on routine) |
| 0x10 | DMA-on routine (called first in each interrupt) |
| 0x14 | init routine (called with the subsong number) |
| 0x18 | start of the sample (instrument) table |
| 0x1C | end of the sample table |
| 0x20 | module title (zero-terminated text) |
| 0x24 | author text ("Howie Davies") |
| 0x28 | information text (copyright, ripper credit) |
| 0x2C..0x38 | four plain numbers (not pointers, not relocated; their use was not determined and they are not needed) |
| 0x3C | subsong table |

You do not need to execute the module's code. The three code pointers matter
only for the start-phase scan (section 11.1), which looks at the code around
them.

### 2.4 Subsong table and count

The subsong table at header long 0x3C holds, for subsong n, four longs: the
order-list pointers of channels 0..3 (stride 16 bytes per subsong; variant A
scales the subsong number by 4 instead of 16, harmless because the only
variant A module has one subsong). The order lists follow the table in
ascending address order, so the first long of the table is the address of the
first order list and

    subsong count = (first long of the table - table address) / 16.

Values: Blazing Thunder 1, Future Bike Simulator 3, Yogi's Great Escape 8.
In variants B and C the subsong table starts right where the sample table
ends (header longs 0x1C and 0x3C are equal in those modules; in variant A the
subsong table lies far after the samples).

### 2.5 Title, author, length

Title: zero-terminated text at header long 0x20 ("Blazing Thunder", "Future
Bike Simulator", "Yogi's Great Escape"). Author: text at 0x24 ("Howie
Davies"). Information text: at 0x28 (copyright line, then ripper credit;
contains newline characters).

There is no stored song length. The song-end condition is in section 9.4. The
model reaches the first song end (seconds of log time) at: Blazing Thunder
subsong 0 330 s; Future Bike subsong 0 232 s (then every 213 s), subsong 1
3.3 s and subsong 2 7.7 s (these two end by stopping all channels, 7.2, and do
not repeat); Yogi's subsong 0 269 s, subsong 7 88.6 s (then every 61.4 s).
Yogi's subsongs 1..6 never report a song end: all four channels park in the
dead end of 7.2 (after 7.7 s for subsongs 1, 4, 5, 6; 9.0 s for subsong 2;
11.6 s for subsong 3) and the song is silent from then on.

### 2.6 Where the driver's tables are (how to locate them)

All tables live in the module; nothing is needed from the player. Listing
them in the order they appear (hunk offsets of the three known modules are in
2.7):

1. **Period table**: 86 words (86 notes, descending, first long
   0x35803280, i.e. periods 0x3580 and 0x3280 are the first two). Find it by
   scanning the image on even offsets for that long. Note byte b in a
   pattern selects word b of this table.
2. **Channel DMA-bit table**: 4 bytes immediately after the period table:
   08 04 02 01. Entry (3 - ch) is the DMACON bit of channel ch.
3. **Variant B only**: 4 more bytes F7 FB FD FE (mask-clear table, entry
   (3 - ch) is the complement of the channel's bit).
4. **Pattern-command table**: longs, one per pattern command in the order
   $FF, $FE, $FD, ... (counting down), 5 entries (variants A and C) or 4
   (variant B); and **order-command table**: 2 entries (A), 3 (B and C). Their
   contents (addresses of the module's own code) are not needed; only their
   count matters, to tell the variants apart.
5. **Envelope list**: 6 bytes before the sample table is a one-entry list: a
   long pointing at the envelope data, which is the 2 bytes 00 FF located
   immediately before the sample table. (Entry index = the instrument's
   envelope number, always 0.)
6. **Sample table**: from header long 0x18; entries of 16 bytes (section 3.2);
   count = (header long 0x1C - header long 0x18) / 16.
7. **Subsong table** (2.4), then the order lists (section 7.2).
8. **Pattern pointer table**: a table of longs immediately after the last
   order list. The last order list is the one with the highest address in the
   subsong table; it ends after its terminating command (a $FF or $FD byte, or
   $FE followed by one operand byte). The first long of the pattern pointer
   table is the address of pattern 0, which directly follows the pattern
   pointer table; so entry count = (first long - table address) / 4. Pattern
   index b of an order list selects long b.
9. **Pattern data** (section 7.1), then the samples.

**Telling the variants apart** (from the distance between the DMA-bit table
(item 2) and the envelope list (item 5)):

- distance 32 bytes (4 bytes, 5 + 2 longs): variant A;
- distance 36 bytes (4 bytes, 4 bytes of mask table, 4 + 3 longs) and the 4
  bytes after the DMA-bit table are F7 FB FD FE: variant B;
- distance 36 bytes (4 bytes, 5 + 3 longs) without that mask table: variant C.

### 2.7 Layout of the known modules (hunk offsets)

| item | Blazing Thunder (A) | Future Bike (B) | Yogi (C) |
|---|---|---|---|
| period table | 0x534 | 0x524 | 0x598 |
| DMA-bit table | 0x5E0 | 0x5D0 | 0x644 |
| mask-clear table | - | 0x5D4 | - |
| pattern-command table | 0x5E4 (5) | 0x5D8 (4) | 0x648 (5) |
| order-command table | 0x5F8 (2) | 0x5E8 (3) | 0x65C (3) |
| envelope list | 0x600 | 0x5F4 | 0x668 |
| sample table | 0x606 | 0x5FA | 0x66E |
| sample entries | 14 | 13 | 17 |
| subsong table | 0x15236 | 0x6CA | 0x77E |
| pattern pointer table | 0x15398 | 0x7D2 | 0x964 |
| pattern 0 | 0x154D8 | 0x8F6 | 0xB38 |

Load addresses in the reference runs (so pointer values written to Paula can
be compared): code image at 0x1B240 (Blazing Thunder), 0x16860 (Future Bike),
0x1CFE8 (Yogi's). A pointer written to a voice equals load address + stored
offset. Silent samples are explained in 9.1.

## 3. Working state

Global (all variants):

- S: speed, a 16-bit value, initial 6. Only its low byte is ever changed.
- C: tick counter, 16 bits, initial 6.
- mask: 16-bit channel-enable bit mask, initial 0x000F. Only variant B ever
  clears bits (section 7.2).
- Variant C only: volume cap, constant 0x40.
- Song-end flags (section 9.4): four flags, initially all set.

### 3.1 Channel record

Per channel (0..3), all 16-bit unless noted:

| field | meaning |
|---|---|
| sample ptr (32) | start address of the current sample |
| period | current period |
| length | sample length in words |
| volume | current volume (before clamping) |
| initial volume | the instrument's volume |
| max volume | A and C only; initial 0x40; changed by pattern command $FB |
| loop ptr (32) | loop start address, 0 = none (A and C use it; B ignores) |
| loop length | loop length in words |
| env ptr (32) | position in the envelope data |
| env start (32) | start of the instrument's envelope data |
| env delta | signed 8-bit value stored as 16 bits |
| env count | remaining ticks; 0xFFFF = envelope finished |
| pattern ptr (32), pattern start (32) | read position / start of current pattern |
| repeat | 16-bit remaining-repeats counter |
| order ptr (32), order start (32) | read position / start of the channel's order list |
| tick counter (8-bit) | steps until the next event |

### 3.2 Sample table entry (16 bytes)

| offset | size | meaning |
|---|---|---|
| 0 | long | sample start (hunk-relative; 0 for entry 0, see 9.1) |
| 4 | word | length in words |
| 6 | long | loop start (0 = no loop) |
| 10 | word | loop length in words |
| 12 | word | initial volume (0..0x40) |
| 14 | word | envelope number (index into the envelope list; 0) |

Entry 0 is the silent instrument (length 1). In the known modules only
Blazing Thunder's instrument 1 has a loop; every other entry has loop start 0
and loop length 1.

## 5. Initialisation (InitSound, called with the subsong number)

Before it the host's wrapper clears its own state. The routine then:

1. sets S = 6, C = 6, mask = 0x0F (C: cap 0x40);
2. gives each channel its starting order pointer (from the subsong table; order
   ptr = order start), pattern pointer = pattern start = **pattern 0**,
   repeat = 0, tick counter = 0, sample ptr = silent address S0 (section 9.1),
   length 1, volume = 0x40, env ptr = the envelope data, env count = 1, env
   delta = 0. A and C also set max volume 0x40, loop ptr = S0, loop length 1,
   initial volume 0 and env start = the envelope data. B does **not** reset
   the initial volume and env start: they keep the values of the previous
   song (0 at the very first start). This only matters if a pattern plays a
   note before its first instrument command, which none of the modules does.
3. performs the Paula writes of the variant (section 8.1).

Because S is 6 and C is 6, and the first tick increments C, the **first tick
runs a step**, and the tick counter 0 then underflows, so every channel
executes its pattern 0 on that first step (all three modules start pattern 0
with either an instrument command or an end-of-pattern command, section 7.1).

## 6. One interrupt

Every 70940 colour clocks (the first one 70940 colour clocks after the end of
the last InitSound call):

1. wrapper prologue (cost 256 cycles; 10.1);
2. **DMA-on routine** (6.1);
3. wrapper middle (32 cycles);
4. **tick routine** (6.2);
5. wrapper epilogue (168 cycles).

### 6.1 DMA-on routine

1. Write DMACON = 0x8000 | mask (set the bits of all enabled channels).
2. Wait: A and C: until 9 raster-line changes have been seen; B: until the
   raster line number (low 8 bits) equals the line seen at the start plus 8
   (the original module waited 2 lines; the reference wrapper patches it to 8,
   as A and C effectively wait 9 changes). The exact costs are in 10.4. The
   wait is about 1850 colour clocks.
3. For every channel that is enabled in mask, in order 0..3, write the voice's
   pointer (long, high word then low word, same colour clock) then its length:
   - A and C: the channel's loop pointer (S0 if it is 0) and loop length;
   - B: S0 and length 1 (no loop, the voice repeats the silent word).

These writes happen on **every** tick (12 writes for four channels).

### 6.2 Tick routine

1. C = C + 1. If C >= S: run the **step** (6.3) and set C = 0.
2. For each channel 0..3 that is enabled in mask: run the **envelope tick**
   (6.4). (There is also a "hold" word per channel that suppresses a channel
   while non-zero; it is never non-zero in any module and is ignored.)

### 6.3 Step

For each enabled channel in order 0..3: decrement the 8-bit tick counter; if
the result is negative as a signed byte (i.e. it just went from 0 to 0xFF) run
the channel's **events** (section 7); otherwise nothing.

A note with duration byte d therefore occupies d + 1 steps.

### 6.4 Envelope tick (per enabled channel, every tick)

A and C:

1. If env count is 0xFFFF: nothing at all.
2. Otherwise volume = volume + env delta (16-bit wrap). Compute the output
   volume: if volume >= max volume, use max volume (unsigned compare); in C
   then if that is >= 0x40, use 0x40. The stored volume is **not** clamped.
   Write AUDxVOL = output & 0x7F.
3. env count = env count - 1. If it reached 0: read the next pair at env ptr,
   env ptr += 2: delta = first byte (signed), count = second byte; a count
   of 0xFF means "finished" and env count becomes 0xFFFF.

B:

1. If env count is not 0xFFFF: volume += env delta; env count -= 1; if it
   reached 0: read the next pair as above; if the count is 0xFF set env count
   to 0xFFFF and **end here without a volume write**; otherwise env count =
   count.
2. Write AUDxVOL = volume & 0x7F. (No maximum clamp. This happens every tick
   once the envelope is finished.)

After a note start the envelope is reset to: env ptr = env start, volume =
initial volume, env count = 1, env delta = 0; so the first tick after a note
loads the pair (00, FF) and the envelope is finished. Net effect in the known
modules: A and C write the volume at the note start and once on the next tick
(same value) and then never again until the next note; B writes it at the
note start and then on every tick.

## 7. Channel events (pattern and order list)

### 7.1 Pattern data

A pattern is a byte string read at the pattern pointer:

- byte b < 0x80: a **note**. Followed by one duration byte d.
  Note start (all steps in this order):
  1. period = period table word b; tick counter = d; pattern pointer += 2.
  2. Write DMACON = DMA bit of the channel (1, 2, 4, 8 for channels 0..3; bit
     15 clear = switch the voice's DMA off).
  3. Reset the envelope (end of section 6.4).
  4. Write voice registers in this order: pointer (long: high then low word,
     same colour clock) = sample pointer; period (offset 6); length (offset
     4).
  5. Write the volume (AUDxVOL = output & 0x7F): A, C: volume clamped as in
     6.4 step 2; B: volume as is.
  The voice then starts at the **next** DMA-on routine (6.1).
- byte b >= 0x80: a command; its number is index = 0xFF - b (so $FF is
  command 0, $FE command 1, ...), with the pattern pointer advanced past b:
  - $FF (0) **end of pattern**: repeat = repeat - 1 (16-bit). If bit 15 of
    the result is clear: pattern pointer = pattern start and continue reading
    commands/notes in the same step. If set: fetch the next order list entry
    (7.2) and continue reading.
  - $FE (1) **instrument** n (one operand byte): copy sample table entry n:
    sample pointer (entry 0: S0), length, loop pointer, loop length; volume
    and initial volume = entry volume; env ptr = env start = envelope list
    entry (entry env number). Continue reading.
  - $FD (2) **speed** (one operand byte): the low byte of S = operand.
    Continue reading.
  - $FC (3) **wait** (one operand byte): tick counter = operand; end the
    channel's events for this step (the next event happens operand + 1 steps
    later).
  - $FB (4) (A and C only) **max volume** (one operand byte): low byte of the
    channel's max volume = operand. Continue reading.

Events keep reading until a note or a wait command ends them.

### 7.2 Order list

Per channel a byte string of (pattern index, repeat) pairs and commands.
Reading an entry (done whenever a pattern ends):

- byte b < 0x80: pattern pointer = pattern pointer table entry b; pattern
  start = pattern pointer; repeat = the next byte (so a pattern is played
  repeat + 1 times); order pointer advances past both bytes. Return to pattern
  reading.
- byte b >= 0x80: command, index = 0xFF - b:
  - $FF (0) **loop**: order pointer = order start (song-end hook, 9.4); read
    the next entry.
  - $FE (1) **jump back** (one operand byte n, at position q): order pointer
    = q - n (variant A), q - n - 1 (B and C); song-end hook; read the next
    entry. (q is the position of the operand byte.)
  - $FD (2), variant B: **channel stop**: order pointer = order start; song-end
    hook; then mask = mask with this channel's bit cleared; read the next entry
    (the channel is not stepped or ticked again).
  - $FD (2), variant C: **dead end**: the order pointer stays on the command;
    the channel's events end and the pattern pointer is stepped back onto the
    end-of-pattern command that led here; the channel's tick counter
    is left as it was (0xFF), so it decrements every step and re-enters this
    state after 256 steps. No Paula write, no flag change. Yogi's subsongs 1..6
    park their channels this way (envelope volume writes stop as well).

### 7.3 Summary of what changes Paula

Only: init writes (8.1), DMACON/pointer/length at every DMA-on routine
(6.1), note starts (7.1), AUDxVOL writes (6.4, 7.1) and, at stop, EndSound
(8.2).

## 8. Paula write sequences

### 8.1 InitSound writes

- **A (Blazing Thunder)**: CIA-A port A write with value 0x02 (address
  0xBFE001, bit 1 = audio filter off; it appears in the log as a line with
  address bfe001 and value 0002); then DMACON = 0x000F; then for channel 0, 1,
  2, 3 in turn: pointer (long) = S0, length (word) = 1.
- **B (Future Bike)**: pointer = S0 for channels 0, 1, 2, 3 (long each), then
  length = 1 for channels 0, 1, 2, 3. No DMACON, no CIA write.
- **C (Yogi's)**: no writes.

### 8.2 EndSound and subsong change

EndSound (called by the host when the subsong is changed or the player stops)
writes DMACON = 0x000F, then AUDxVOL = 0 for channels 0, 1, 2, 3. A
subsong change from the host is: EndSound, 236 colour clocks of idle, then
InitSound(new subsong). When a run is started on subsong n > 0, the reference
host first runs InitSound(0), then for k = 1..n: EndSound (starting at the
clock where the previous InitSound ended), 236 colour clocks after its end,
InitSound(k) (so the logs for n >= 1 contain n EndSound blocks and n+1 init
blocks; the first tick is 70940 colour clocks after the last InitSound).

### 8.3 Write order inside one interrupt

DMACON(0x8000|mask), then (after about 1850 colour clocks) per channel
pointer(2 words), length; the tick routine's writes come after that: for a
note start DMACON(bit), pointer(2 words), period, length, volume; plain
volume writes in channel order.

## 9. Wrapper (Eagleplayer) behaviours that change output

The reference player is the module plus the Eagleplayer's wrapper, which
patches the module after loading. These are the effects.

### 9.1 Silent address S0

The wrapper replaces every place where the module clears a voice pointer to
0 with "pointer = S0, length 1", where S0 is the address of a 16-bit zero word
in chip memory. In the reference runs S0 = 0x00798A58 (appears in logs as
write 0079 / 8A58); the value is arbitrary and a clean implementation
may use any silent chip address. The same replacement applies to a **loop
pointer of 0** in the DMA-on routine of A and C (6.1 step 3), and the wrapper
also stores S0 in the sample pointer of **sample table entry 0**, so
instrument 0 plays the silent word.

### 9.2 Volume scaling

All AUDxVOL writes go through a wrapper routine that multiplies by the host's
per-channel enable mask; at default settings the written value is the module's
value & 0x7F (values above 0x40 are written unchanged, e.g. 0x7F).

### 9.3 Raster waits

The module's busy-wait loops are replaced: in A and C by a call to a helper
that waits for 9 raster-line changes; in B the module's own 2-line wait is
patched to 8 lines (see 6.1). In the logs both come out as a gap of about 1850
colour clocks between the DMACON write and the first pointer write.

### 9.4 Song end

The wrapper keeps one flag per channel, all set at InitSound. Executing the
order-list commands $FF or $FE (and variant B's $FD) clears the channel's
flag. When that leaves all four flags clear, all four are set again and the
host is told that the song ended. Playback continues unchanged (the order
lists simply loop). The notification costs extra time on that one execution
(10.2, HOOK).

### 9.5 Other patches

The wrapper corrects module accesses to the custom-chip mirror addresses to
the normal 0xDFF000 range (so all writes show as DFF0xx) and patches the four
channel "clear pointer / length" code sites to S0 (9.1). It does not change
any musical behaviour.

## 10. Timing

### 10.1 Cost rule

All times in colour clocks (cc). Execution costs are in 68000 **cycles**; a
cycle count c measured from the start of a call gives the clock
`call_start + floor(c / 2)`. A write is stamped with the clock **after** the
cost of the instruction that performs it has been added (i.e. at the end of
that instruction). Several of the host's per-instruction costs are odd
(branches 9 and 11), so costs are accumulated in cycles and only halved when a
clock is needed. The model below reproduces the reference logs to the cc.

Raster line at clock t: line = floor(t / 227) mod 313; the low 8 bits of the
line are the value read by the wait loops.

### 10.2 Costs (cycles) of the fixed parts

Wrapper: prologue 256, middle 32, epilogue 168.

The numbers below are "cycles added by the part", given in the order of
section 6/7 where they occur. "(w)" marks the point where a Paula write is
stamped (after adding the part).

**InitSound call** (cycles from the call): 1168 for the wrapper's own
setup, then

- A: +24, CIA write (w); +20, DMACON write (w); +114; then per channel:
  +30, +432, LC write (w), +16, LEN write (w), +48; after channel 3: +52.
- B: +40; +12, +16, LC0 (w), +16, LC1 (w), +16; +4; +16, LC2 (w), +16, LC3 (w);
  then +12 LEN0 (w), +12 LEN1 (w), +12 LEN2 (w), +12 LEN3 (w); +72; +34; per
  channel +26, +344; after channel 3: +16.
- C: +106; per channel +30, +404; after channel 3: +16.

Reference InitSound-call clocks: Blazing Thunder 38646, Future Bike 38465,
Yogi's 41083 (11.1). The call ends at the clock obtained from the total, e.g.
BT 40387, FB 39942, Yogi's 42596.

**EndSound call** (cycles): +28, DMACON (w), +4, then +12 and vol0 (w), +12
vol1 (w), +12 vol2 (w), +12 vol3 (w), +16. Total 96 cycles = 48 cc.

**Tick routine** (cycles):

- A, C: +47; if C + 1 >= S: +18, STEP, +4; then +76; for channels 0..3: +31;
  if enabled: +18, ENV (C also +23); then +26 (each channel); at the end +16.
- B: +39; step: +18, STEP, +4; then +68; per channel +31; if enabled +18, ENV,
  +23; +26; end +16.

**STEP**: A, C: +60; B: +52; per channel +31; if enabled +18, CHANSTEP; +26;
end +16. CHANSTEP: +27; if the counter did not go negative: +16, return;
otherwise EVENTS.

**ENV** (A, C): +27; if env count = 0xFFFF: +16, return. Else +63; if volume >=
max volume: +12; (C: +27, and +16 if the value is >= 0x40); VOLWRITE; +27;
if env count != 0 after decrement: +16, return; else +67; if the new count
is 0xFF: +48 else +52.

**ENV** (B): +27; if env count != 0xFFFF: +55; if count reaches 0: +67; if
the new count is 0xFF: +48 and return (no volume write) else +46. Then (when
reached) +12, VOLWRITE, +16.

**VOLWRITE** (all variants): +30, then the volume hook of the channel (10.3):
write at the channel's volume-write offset, total as listed.

**EVENTS**: loop: +35 (read a byte); a note goes to NOTE and ends; otherwise
dispatch +64 (A, C) / +60 (B), then:

- $FF end-of-pattern: +27; if repeat stays non-negative: +38 and restart the
  pattern (loop); else ORDER (C: if ORDER ends the channel (dead end), stop).
- $FE instrument: A, C +288; B +280 (variant data copy).
- $FD speed: +46. $FC wait: +48 and end. $FB max volume (A, C): +42.

**NOTE** (after the +35 byte read; the note and duration bytes are read in
these costs):

- A: +132, DMACON write (w); +80; +20 (call into the note hook); then +24,
  pointer (w); +20, period (w); +20, length (w); then the remainder of the
  note hook up to its total (10.3, total minus 64); +24; +35; +12 if the
  volume >= max volume; VOLWRITE; +16.
- C: as A with +186 instead of +132, and after the max-volume step +27, and
  +16 if the clamped value is >= 0x40.
- B: +147, DMACON write (w); +80; +20; hook as in A (pointer, period, length
  writes, remainder); +24; +20; the note-volume hook of 10.3 (volume write at
  its offset, then to its total); +16.

**ORDER**: loop: +35 (read a byte); entry: +142, return; command: +64 (B: +60),
then $FF: HOOK, +10, continue; $FE: HOOK, +42 (A; B: +46; C: +46), continue;
$FD: B: HOOK, +64, clear mask bit, continue; C: +16, dead end.

**HOOK** (loop / jump / channel-stop commands): +20, then the channel's
loop-hook total (10.3). If this clears the last set flag (all four flags now
zero): flags are set again and +83 cycles (A and C), +84 (B) are added; the
host is told the song ended.

**DMA-on routine**: A, C: +40, DMACON (w), wait (10.4), +4, +60, per channel:
+31; if enabled: +61 (+12 if loop pointer is 0), pointer (w), +20, length (w),
+28; +26; after the loop +16. B: +44, DMACON (w), wait (10.4), +40, per
channel: +31; if enabled: +20, +24 pointer (w), +16 length (w), +24; +26;
after the loop +16.

### 10.3 Hook costs per channel (cycles, channels 0..3)

| hook | writes at | total |
|---|---|---|
| note start (pointer, period, length) | 24, 44, 64 after entry | 229, 260, 291, 299 |
| volume (A, B, C normal path) | 173, 190, 197, 224 after VOLWRITE's 30 | 314, 362, 400, 435 |
| note volume (B) | 197, 214, 221, 248 after entry | 350, 398, 436, 471 |
| loop / jump / stop reporting | - | 204, 231, 254, 267 |

(The variance by channel is the wrapper's per-channel dispatch.)

### 10.4 Waits

A and C (after the DMACON write): +66; then nine times: { +16 and read the
line byte as reference; poll: +16, read the line byte, +9, repeat the poll
until the line byte differs from the reference; +10 }; then +60.
B (after the DMACON write): +12, read the line byte, target = byte + 8 (mod
256); +4; poll: +12, read, +9, until the read equals the target.

The line is read at the clock reached after adding the 16 (or 12) cost.

## 11. Start phase

All times colour clocks from the start of the host's player initialisation
(log clock 0).

### 11.1 First InitSound call

Before the first InitSound the wrapper walks the loaded (relocated) image
to find places to patch, at fixed costs per word. Cycle costs:

- stage 1: 44 + for each of 401 words starting at the tick routine pointer
  (header long 0x0C): 47, plus 48 more for every word position where the long
  at that position equals 0x3D4000A8;
- stage 2: +16; starting at the init routine pointer (header long 0x14),
  repeat: 29; if the long at the position is 0x35803280 stop; else +21; if the
  word is 0x5407: +12 and advance 2; +29; if the long at the (new) position is
  0x4ED02B6D: +64 and advance 8; +21; advance 2; if the word just stepped over
  was 0x00DF: +26 and advance 2 more.

Total scan cycles: Blazing Thunder 75378, Future Bike 75016, Yogi's 80252.

    InitSound call clock = 957 + floor(scan cycles / 2)

(38646, 38465, 41083 exactly). The first tick is then at
(InitSound call clock) + (init duration) + 70940.

## 12. Validation

A private native model written from this text reproduced the reference logs
register-for-register and colour-clock-exact (zero differences) for Blazing
Thunder subsong 0, Future Bike subsongs 0, 1, 2 and Yogi's subsongs 0..7, over
the full 120 s runs (or until the host stopped at song end). See README.md for
the honest limits.
