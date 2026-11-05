# Sound Programming Language (Holger Gehrmann): behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the "SPL" game-music format of Holger Gehrmann (1987-88, four-voice Paula sample player; file name
prefix `spl.`; Eagleplayer "Sound Programming Language", UADE player `SoundProgrammingLanguage`, adapted
by the Wanted Team from rips by Don Adan). This is a description of observed behaviour (write logs of the
reference 68k player, beam-read traces, module data) in my own words. It contains no instructions or
addresses of the original code. "cc" are Amiga colour clocks (1 cc = 2 CPU cycles).

A value model written from this description reproduces the hardware write sequence (register, value,
order) of the reference exactly for all four modules in the corpus, 12 (module, subsong) runs of 25 s
(4 641 ticks that write something) and 8 runs of 90 s, plus 2 synthetic modules (section 10). Timing is specified
with measured numbers (section 8); it is approximate to about +-60 cc for the first write after a
parser step and about +-5 cc inside a note-on sequence.

## 1. What the files are

An `spl.*` file is a standard Amiga executable (one code hunk, with a relocation table) that contains
the game's whole sound driver, its tables, the music data (note streams) and the samples. The Wanted Team
added a header in front of the driver (section 2) so that a player can find the entry points, the number
of subsongs, the title and the timer. The reference player does not interpret the music data itself: it
loads the executable, relocates it, and calls two entry points of the driver (one to start a subsong, one
once per timer interrupt). A port must therefore reproduce the behaviour of the embedded driver.

Four modules exist in the corpus. They contain three generations of the driver ("families"), selected
here by what they do, not by any name in the file:

| family | module | size | subsongs | tick rate | notes |
|---|---|---|---|---|---|
| 1 | Boulder Dash | 71 584 | 1 | 60 Hz (timer 11 823) | one sample base, 0x3a-byte channel records, 8-tick swing tempo |
| 2 | Clever & Smart | 73 060 | 1 | 50 Hz (timer 14 188) | sample tables, tempo bit mask |
| 2 | Graffiti Man | 69 992 | 8 | 50 Hz (timer 14 188) | as above plus song select, stop and transpose commands |
| 3 | Hollywood Poker | 69 868 | 2 | 17.73 Hz (timer 40 000) | no effects, 16-byte records, a step on every tick |

The three families share the note/period maths, the note stream language and the way a note is started
on Paula; they differ in the channel record, the per-tick effects, the tempo and the song start. Section
3 is common, sections 4-6 are per family, section 7 lists the host-side behaviour that shows up in the
reference logs, section 9 lists the layout parameters of each module (offsets of the driver's tables and
variables inside the code hunk).

Only these four files are known (the other Gehrmann games in the archive use a different format,
`sct.*`). Nothing here is claimed for unknown modules: the layout parameters of section 9 are per module.

## 2. Detection, loading, header, subsongs, title

### 2.1 Detection (what the Eagleplayer checks)

On the file as loaded (nothing else is looked at):
- the first long is 0x000003F3 (Amiga hunk header);
- the first byte of the first entry of the hunk size table (file offset 0x14) is not zero (the memory-type
  bits of the size word; the four modules have 0x40, chip memory);
- the long at file offset 0x20 (the first long of the code hunk) is 0x70FF4E75;
- the next eight bytes (file offset 0x24) are the text `!SOPROL!`;
- the three longs that follow (file offsets 0x2C, 0x30, 0x34: header fields 5.3 below) are all non-zero.

Anything else is rejected. File name prefix `spl.`.

### 2.2 File layout

Standard hunk file: 0x3F3 header (one hunk), hunk type 0x3E9 (code) with its length in longs at file
offset 0x1C, then the code bytes starting at file offset 0x20 (this is "offset 0" of the code hunk in this
document: every "offset" below counts from there), then a relocation table (hunk type 0x3EC) and the hunk
end. The relocation table lists the 32-bit fields of the hunk that hold hunk-relative addresses.

All pointers stored in the file are hunk-relative offsets. The loader adds the load address (BASE) to
those listed in the relocation table. In this document pointers read from the file are therefore
"offsets" and anything that is written into a hardware register or stored into the driver's own
variables at run time is "BASE + offset" (see 3.1 for why that matters).

### 2.3 Header (code hunk offsets)

| offset | size | meaning |
|---|---|---|
| 0x00 | long | 0x70FF4E75 (detection) |
| 0x04 | 8 bytes | `!SOPROL!` |
| 0x0C | long | offset of the per-tick entry (the host calls it once per timer interrupt) |
| 0x10 | long | offset of the start entry (called with the subsong number, 1-based, as a 32-bit value) |
| 0x14 | long | number of subsongs (1, 1, 8, 2 in the corpus) |
| 0x18 | long | offset of the title, a zero-terminated string |
| 0x1C | long | offset of the author string (all four: "Holger Gehrmann") |
| 0x20 | long | offset of an information string (copyright and rip credits) |
| 0x24..0x30 | 4 longs | informational sizes (shown by the host; unused by the replay; meaning not determined) |
| 0x34 | long | timer reload value, in E-clock cycles (709 379 Hz PAL): tick period = value x 5 cc |

The title strings are "Boulder Dash", "Clever & Smart", "Graffiti Man", "Hollywood Poker".
The subsong count is the long at 0x14. There is no stored length; the music has no end except the
explicit stop command of the family-2 songs (4.7). The first subsong is number 1.

The tick period in cc is value x 5: 59 115 (family 1), 70 940 (family 2), 200 000 (family 3). The first
tick's phase and the exact tick spacing belong to the host framework.

## 3. Common behaviour

### 3.1 Memory model

The driver works on its own code hunk as memory: its variables, tables and the music streams all live
in the hunk image (the file contents are their initial values). A faithful implementation therefore
keeps the hunk as a mutable byte array, places it at a load address BASE, and applies all pointer
arithmetic with BASE included. Two consequences are visible in the reference output:
- Values the driver stores into its own tables (return addresses on the call stacks, section 3.4) are
  BASE + offset; in two modules the stack areas overlap a table, so some AUDxLEN values depend on the low
  16 bits of BASE (section 4.9 and README). Load addresses in the reference runs: Boulder Dash
  0x16188, Clever & Smart 0x16750, Graffiti Man 0x15B50, Hollywood Poker 0x15AD8.
- Sample start addresses written to AUDxLC are BASE + offset. Compare logs modulo that constant.
- A "clear four bytes at the start of a sample" step (4.4) writes into the hunk image; the hunk is
  modified by playing (family 2 only).

All 16-bit variables are big-endian words; "byte" and "word" fields in the records are at the stated
offsets of the record.

### 3.2 Notes and periods

A note byte has the semitone in its low nibble (0..11) and an octave count in its high nibble. The
driver adds, in order:
1. a transposition: T = (global transposition byte + the instrument's transposition word, if the family
   has one) modulo 256; apply T times: add 1 to the note byte (modulo 256); if the low nibble is now 12,
   add 0x10 to the byte (modulo 256) and clear the low nibble (so semitone 12 is skipped). T of 0 does
   nothing. (Family 3: global byte only.) The global byte is a byte of the hunk (section 9); the Graffiti Man
   stream command 0xF6 changes it (3.3).
2. the octave offset of the instrument (family 1: word at record+0x0C, family 2: word at record+0x04,
   family 3: byte at record+0x08): shift it left by 4 (modulo 256) and add it to the note byte (modulo 256).
3. period = (16-bit table entry indexed by the low nibble) shifted right (logical) by the high nibble.

The table has 16 words; it is the same in all four modules: 0xD600, 0xCA00, 0xBE80, 0xB400, 0xA980,
0xA000, 0x9700, 0x8E80, 0x8680, 0x7F00, 0x7800, 0x7100, 0x6B00, 0x6B00, 0x6B00, 0x6B00. (Value / 256 is
the PAL period of the octave with shift 8; shift 7 gives 428 for the first entry.) Low nibbles 12..15 are valid
indexes (all give 0x6B00); there is no clamping of the shift count.

### 3.3 The note stream language (all families)

Each of the four voices has its own byte stream (a position in the hunk) and a wait counter and a
default duration (one byte each). A "parser step" runs all voices in order 0..3 (the family decides how
many voices, 4.8). For a voice:

1. If the wait counter is not 0: decrement it; if it is still not 0 the voice is done for this step.
   (So a counter loaded with D is read again D steps later; a counter of 0 means "read every step".)
2. Otherwise read commands from the stream until a note has been started:
   - 0xF4: skip this byte.
   - 0xF3 + 4-byte pointer: jump: the stream position becomes the pointer (an offset).
   - 0xF1 + 4-byte pointer: call: push the address of the pointer field (BASE + position of the operand)
     on the voice's call stack and jump to the pointer.
   - 0xF2: return: pop the call stack; continue at popped address + 4 (just behind the pointer field).
   - 0xF0: load instrument: copy the following N bytes into the start of the voice's channel record and
     skip them (N: family 1 0x22, family 2 0x1A, family 3 0x10). Runtime fields of the record beyond N are
     not touched.
   - 0xC0..0xEF: duration: set the wait counter AND the default duration to (byte - 0xC0), skip the byte.
     (It does not end the step; the next command is read at once. Counter and default are overwritten
     again when a note starts.)
   - Graffiti Man only: 0xF5 = stop the song (4.7); 0xF6 + 1 byte = set the global transposition byte to
     that byte, skip both bytes. (Clever & Smart, Boulder Dash and Hollywood Poker do not have these two
     commands: in those modules 0xF5 and 0xF6 are notes, like the rest of the list below.)
   - any other byte (everything below 0xC0, and 0xF5..0xFF where not listed above): a note (3.5).
3. After a note: advance the stream past the note byte(s), reload the wait counter from the default
   duration, the voice is done for this step.

### 3.4 Call stacks

Each voice has a stack pointer variable and a storage area in the hunk (section 9); the pointer starts at
the start of the area. Push stores a 32-bit value at the pointer and adds 4; pop subtracts 4 and reads.
There is no bounds check. The areas are 0x28 bytes (10 entries) apart in families 1 and 2 and 8, 0x28,
0x28 bytes apart in family 3 (voice 0 has room for 2 entries). In Clever & Smart voice 3's area starts only 8
bytes below the sample length table, and its music nests three deep on that voice, so the third push
lands on the table (4.9). The other modules never nest deeper than their areas.

### 3.5 Note start, common part

When a voice reads a note byte b the driver starts the note on that voice (family 3: 5.2). If the
instrument's flag bit 0 is set (families 1 and 2; "multi-note"), further notes follow: if bit 5 is set the
next three stream bytes are notes too (four in total), otherwise the next one (two in total); each is
started as a full note-on in sequence with the same voice, in stream order; the stream is then advanced
past all of them. (A multi-note instrument is an arpeggio: it keeps the last four periods, 4.5.)
With the flag clear the stream is advanced by 1.

## 4. Families 1 and 2

The two families use the same record layout idea and the same per-tick effects; they differ in how the
sample is addressed, in the note-on sequence and in the tempo.

### 4.1 Channel records

Four records of 0x3A bytes, contiguous, one per voice (hardware voice n = record n). Field offsets:

| field | family 1 | family 2 |
|---|---|---|
| sample start | long at 0x00: offset relative to the sample base | word at 0x00 is a sample number (index into the tables of 4.2); word at 0x14 is a start offset in bytes |
| length | word at 0x04, in words | from the length table |
| loop start | none | word at 0x02, in bytes (0 = no loop part) |
| octave offset | word at 0x0C | word at 0x04 |
| flags | word at 0x0E | word at 0x06 |
| arpeggio step reload | word at 0x10 | word at 0x08 |
| decay step reload | word at 0x12 | word at 0x0A |
| envelope reload | word at 0x14 | word at 0x0C |
| envelope step | word at 0x16 | word at 0x0E |
| initial volume | word at 0x1E | word at 0x16 |
| instrument transposition | word at 0x20 | word at 0x18 |

Runtime fields (both families, same offsets): 0x22 arpeggio counter (word), 0x24 decay counter, 0x26
current decay volume, 0x28 envelope counter, 0x2A envelope volume, 0x32 / 0x34 / 0x36 / 0x38 the last
four periods (0x32 is the newest). Flags: bit 0 arpeggio / multi-note, bit 1 rotate four periods
(otherwise two), bit 2 volume decay, bit 3 envelope restart, bit 5 four notes per step. Other bits are
not used. The corpus uses the values 0, 1, 4, 0x23, 0x27 (the envelope bit 3 never appears in the real
modules; it is specified from the driver and was checked on two synthetic modules, section 10).

### 4.2 Family 2 tables

A table of 16-bit lengths (in bytes) and a table of 32-bit sample offsets, both indexed by the sample
number (offset table: entries are 4 bytes, length table: 2 bytes). In Graffiti Man the offset table is
immediately followed by the length table; both have 11 used entries. Offsets are hunk offsets of the
sample data (relocated: BASE + offset goes to the hardware).

### 4.3 Note-on, family 1

Given voice n, note byte b (record = record n):
1. Transpose b (3.2 step 1; T = global byte + record word 0x20).
2. Write AUDxLEN = 1 and AUDxPER = 1 (two word writes, LEN first).
3. Wait (8.2).
4. Clear the runtime bytes 0x22..0x29 of the record. Set 0x26 and 0x2A to the initial volume; set 0x28 to
   the envelope reload.
5. Add the octave offset, look up the period (3.2).
6. Write DMACON = 1 << n (DMA off for the voice).
7. Write AUDxPER = period.
8. Shift the period history: 0x38 <- 0x36, 0x36 <- 0x34, 0x34 <- 0x32, 0x32 <- period.
9. Write AUDxLC = BASE + (sample base) + (long at 0x00) as two word writes, high word then low word
   (the sample base is the variable of section 9: its initial value is the offset of the first sample
   area; at run time BASE + that).
10. Write AUDxLEN = word at 0x04, then AUDxVOL = initial volume.
11. Write DMACON = 0x8000 | (1 << n) (DMA on).

The sample plays in a loop over the whole sample until the next note. Sequence of writes per note-on:
LEN, PER, DMACON(off), PER, LC hi, LC lo, LEN, VOL, DMACON(on) = 9 writes.

### 4.4 Note-on, family 2

Given voice n, note byte b, record n, sample number s, length L = length table[s] (bytes), start offset
S = word at 0x14:
1. If b is 0, nothing happens at all (no writes, nothing changes).
2. Write AUDxLEN = 1, AUDxPER = 1.
3. Wait (8.2).
4. Transpose b (T = global byte + record word 0x18); clear runtime bytes 0x22..0x29; set 0x26 and 0x2A to
   the initial volume (word 0x16); set 0x28 to the envelope reload (word 0x0C).
5. Add the octave offset (word 0x04), look up the period.
6. Write DMACON = 1 << n. Write AUDxPER = period. Shift the period history as in 4.3.
7. Write AUDxLC = BASE + offset table[s] + S (high word, low word). Clear the 32-bit value at the start
   of the sample (offset table[s], in the hunk image) to zero.
8. Write AUDxLEN = (L - S) / 2 (16-bit), AUDxVOL = initial volume, DMACON = 0x8000 | (1 << n).
9. Wait (8.2).
10. Let d = (word 0x02) / 2. If d is 0: write AUDxLEN = 1. Otherwise write AUDxLEN = L - d (16-bit; note
    this is not halved) and then AUDxLC = BASE + offset table[s] + 2d (high word, low word).

Per note-on: 9 writes + (1 or 3) = 10 or 12.
The first sample longs being zeroed give the "play once then silence" behaviour when d = 0 (the hardware
re-reads the cleared start for its one-word loop).

### 4.5 Per-tick effects (before the parser, every tick; families 1 and 2)

For each voice in the order 0..3 (family 2: the voices that are active, 4.8), look at the flags:

Arpeggio (bit 0): if the arpeggio counter is non-zero, decrement it. Otherwise reload it from the arpeggio
reload and rotate the period history: with bit 1 set (four-slot): the new slot 0x32 gets the old 0x38, 0x38
gets the old 0x36, 0x36 gets the old 0x34 and 0x34 gets the old 0x32; with bit 1 clear (two-slot): new
0x32 = old 0x34 and new 0x34 = old 0x32. Then write AUDxPER = the value in slot 0x34.

Decay (bit 2): if the decay counter is non-zero, decrement it. Otherwise reload it from the decay reload;
if the decay volume (0x26) is non-zero, subtract 8 from it (16-bit, no clamping) and write AUDxVOL =
that value. (All real modules use initial volumes that are multiples of 8, so it stops at 0; a value not
a multiple of 8 would wrap and the host masks the write to 7 bits.)

Envelope restart (bit 3): if the envelope counter is non-zero, decrement it. Otherwise reload it from the
envelope reload; if the envelope volume (0x2A) is non-zero: subtract the envelope step (16-bit); if the
result is negative (bit 15) use 0; store it as 0x2A and as the decay volume 0x26; write AUDxVOL = it; then
restart the voice: write AUDxLEN = 1, AUDxPER = 1, wait (8.2), write DMACON = 1 << n, write AUDxLEN (family
1: the record length word; family 2: the raw length table value, not halved), write AUDxPER = the value in
slot 0x32, write DMACON = 0x8000 | (1 << n).

The parts are tested in this order (arpeggio, decay, envelope) for one voice before the next voice.

### 4.6 Tempo and the tick

Family 1: each tick runs the effects, then a tempo counter (one word, starts at 0) decides about the
parser: if the counter is 0, set it to 7 and run a parser step; if it is 3, set it to 2 and run a parser
step; otherwise decrement it. The parser therefore runs on a repeating cycle of 8 ticks with steps at tick
0 and tick 5 (spacing 5, 3, 5, 3 ...). The first tick runs a step.

Family 2: each tick runs the effects (Graffiti Man: only while the song variable is not 0, 4.8), then
increments a 16-bit tick counter (starts at 0, so the first tick tests bit 1) and tests bit (counter AND
31) of a 32-bit tempo mask (section 9); if the bit is set it runs a parser step (also when the song
variable is 0). All four modules have mask 0x92492492 for the song start
(steps every third tick, with a gap of two at the wrap, bit 31 to bit 1).

### 4.7 Start of a subsong

Family 1: load the four stream positions and four call-stack pointers from the layout of section 9;
clear all four wait counters and default durations, clear the tempo counter. Then run the hardware preset
(4.10).

Family 2: store the subsong number (1..8 in Graffiti Man) in the song variable; for Graffiti Man load the
four stream positions from the song table (section 9) and the tempo mask from its initial copy; load
call-stack pointers, clear counters and default durations and the tick counter. An unknown song number
changes nothing but the variable. Clever & Smart has only one song: the same without the song variable, and
writes ADKCON = 0x0088 once.

Stopping: Graffiti Man's stop command 0xF5 (3.3) sets the song variable to 0 and aborts the current parser
step (voices not yet visited are not read); the voice's stream position is not advanced. From then on no
effects run and the parser, which still runs when the tempo mask says so, visits only voices 0 and 1 (4.8;
it reads the 0xF5 again if it is in one of them). The sound that is playing keeps playing (its DMA is
never stopped). This is the only end of a song in the corpus: the jingle songs 7 and 8 end this way
(last write at 3.2 s for song 7 and 3.7 s for song 8). The other songs and modules loop through jump commands for
ever.

### 4.8 Active voices (Graffiti Man)

If the song variable is 0 no effects run. The parser and the effects visit voices 0..3 when the song
variable is 1, 7 or 8, and only voices 0 and 1 otherwise (songs 2..6 are effects on two voices; also
after a stop). Boulder Dash and Clever & Smart always visit all four voices.

### 4.9 Clever & Smart: overflowing stack

Voice 3's call stack area starts 8 bytes before the sample length table; its stack grows into the table
with the third nested call: the pushed value (a 32-bit BASE + position) is stored big-endian, so length
table entry 0 gets its high word and entry 1 its low word. Samples 0 and 1 are used by this module, so
AUDxLEN of notes using them comes from the clobbered entries and depends on the low 16 bits of BASE.
Example: with BASE = 0x16750 the first affected note in the 25 s log (voice 3, the 152nd of the ticks that
write) shows AUDxLEN 0x392D where an unclobbered table gives 0x0585. A port that wants the same values
must use the same BASE (or accept different lengths for these notes; nothing else depends on BASE).

### 4.10 Hardware preset (families 1 and 3, start)

At the start of a subsong (and again once per earlier subsong, section 7) the reference writes, in this
order, with the module's first sample: AUD0LEN = (word in the sample area) / 2, AUD0PER = 0x01AC,
AUD0VOL = 0x40, DMACON = 0x800F, AUD0LC = BASE + offset of that sample (two word writes), ADKCON = 0x0088.
Voice 0 therefore starts playing the first sample (an arbitrary block of the sample area, 0x1010 words long
in the corpus) at period 0x1AC until the first note on voice 0 replaces it; voices 1..3 are enabled but
have no data. Family 1: also the length is stored into record 0's length word. Offsets: section 9.

## 5. Family 3 (Hollywood Poker)

### 5.1 Records and start

Four records of 16 bytes (contiguous) loaded by 0xF0 with 16 bytes: long 0x00 = sample start offset (a
relocated pointer: BASE + offset goes to AUDxLC), word 0x04 = AUDxLEN (words), byte 0x08 = octave offset
(3.2 step 2), byte 0x0F = volume. No flags, no runtime fields, no effects, no tempo: one parser step on
every tick. The subsong number is 1 or 2 (any other number behaves as 2): load the four stream positions
from the table of section 9 (voice 0 starts at the same position in both songs), the call-stack pointers
and clear the wait counters and default durations; then the hardware preset (4.10).

### 5.2 Note-on, family 3

Given voice n and note byte b:
1. Transpose b by the global byte (3.2).
2. Write AUDxLEN = 1, AUDxPER = 1; wait (8.2).
3. Add the octave offset (byte 0x08), look up the period.
4. Write DMACON = 1 << n; AUDxPER = period; AUDxLC = BASE + long 0x00 (high word, low word);
   AUDxLEN = word 0x04; AUDxVOL = byte 0x0F; DMACON = 0x8000 | (1 << n).
The byte-wide volume write is logged as the byte value masked to 7 bits (module data has 0xF4, logged
0x74; values above 0x40 act as 0x40 on Paula). 9 writes per note.
In families 1 and 3 the transposition precedes the first write of the note-on; in family 2 it follows the
first wait.

## 6. Voice/hardware conventions

Hardware voice n has its registers at 0xDFF0A0 + 0x10 n: LC (0, high word), LC low (2), LEN (4), PER (6),
VOL (8). DMACON is 0xDFF096; bit n enables voice n; ADKCON is 0xDFF09E. The logs record every write
as a 16-bit value (a byte write to the volume register is logged as a word at the register address).

## 7. Host behaviour seen in the reference logs

- The first writes are always the host's own (volume registers cleared: all four voices, twice; the first
  four at time 0, the second four 249 cc after the fourth; each write 132..144 cc after the previous).
- Starting subsong k (1-based) makes the host run the start entry once for each subsong 1..k: before the
  start entry for subsong j+1 the host writes DMACON = 0x000F (327 cc after the last volume write) followed
  by three rounds of four volume-register writes of 0 (voices 0..3; gaps 8, 8, 8 then 228, 132, 144, 139
  then 249, 132, 144, 139). Subsong 1 has none. For Graffiti Man the repeated start entries write nothing;
  for Hollywood Poker the hardware preset (4.10) is repeated. The final state is that of subsong k.
- Every AUDxVOL write goes through a master-volume stage: logged value = (value AND 0x7F) x 64 / 64 =
  value AND 0x7F (default master volume). DMACON, LC, LEN, PER pass unchanged.
- Subsong numbers on the command line are 0-based; the driver is called with k + 1.
- The host patches the driver's busy-wait loops (8.2): every delay of the original becomes the same
  raster wait.

## 8. Timing (colour clocks)

### 8.1 Conventions

Gaps are measured in the reference write log between consecutive writes of a tick unless said otherwise.
"ch n" is the hardware voice. A tick's first write time is the host's (interrupt latency, phase).
Medians over 25..90 s runs; ranges are the observed min..max. One raster line is 227 cc in the host's
beam model (the beam position is read from the high byte of the vertical beam counter, i.e. the line
number modulo 256).

### 8.2 The wait

Each place where the original had a busy-wait loop is now: after the preceding write, the first beam read
happens 41 cc later; further reads follow every 12.5 cc (the first two are 8 cc apart). The wait ends at
the first read in which the line number has changed 9 times since the first read (the read lands in line
L0+9, within 12.5 cc after the start of that line). The next write follows the last read by a fixed
"post" time that depends on the place. In numbers the wait lasts about 9 x 227 = 2043 cc minus the
position within line L0 at the first read, rounded up to the 12.5 cc read grid; so the visible gap
between the write before and the write after varies by about 227 cc from note to note (family 1: 2399 to
2659 including the post time).

### 8.3 Note-on, family 1 (Boulder Dash), median gaps in cc

| step | ch 0 | ch 1 | ch 2 | ch 3 |
|---|---|---|---|---|
| PER=1 after LEN=1 | 0 | 0 | 0 | 0 |
| wait post time (to DMACON off), plus 19.5 per octave shift | 392 | 405 | 419 | 432 |
| PER after DMACON off | 6 | 6 | 6 | 6 |
| LC high after PER | 151 | 167 | 183 | 187 |
| LC low after LC high | 0 | 0 | 0 | 0 |
| LEN after LC low | 60 | 76 | 91 | 95 |
| VOL after LEN | 156 | 183 | 210 | 210 |
| DMACON on after VOL | 90 | 106 | 121 | 125 |

The octave shift count is the high nibble of the final note byte (4.3 step 5): the post time is
392 + 19.5 x shifts for voice 0 and so on.

### 8.4 Note-on, family 2 (Clever & Smart, Graffiti Man), median gaps in cc

| step | ch 0 | ch 1 | ch 2 | ch 3 |
|---|---|---|---|---|
| PER=1 after LEN=1 | 0 | 0 | 0 | 0 |
| wait 1 post time (to DMACON off), about 430 + 26 per transposition step, +-12 | 430 | 430 | 430 | 430 |
| PER after DMACON off | 6 | 6 | 6 | 6 |
| LC high after PER | 142 | 158 | 173 | 177 |
| LC low after LC high | 0 | 0 | 0 | 0 |
| LEN after LC low | 87 | 102 | 117 | 121 |
| VOL after LEN | 156 | 183 | 210 | 210 |
| DMACON on after VOL | 100 | 116 | 131 | 135 |
| wait 2: post time to the first loop-part write | 70 (no loop part) / 84 (loop part) | same | same | same |
| loop part: LC high after LEN | 76 | 92 | about 102 | 112 |
| loop part: LC low after LC high | 0 | 0 | 0 | 0 |

(The transposition steps are those of 3.2 step 1; for the corpus T = global + instrument word.)

### 8.5 Note-on, family 3 (Hollywood Poker), median gaps in cc

| step | ch 0 | ch 1 | ch 2 | ch 3 |
|---|---|---|---|---|
| PER=1 after LEN=1 | 0 | 0 | 0 | 0 |
| wait post time (to DMACON off), plus 19.5 per octave shift | 158 | 172 | 186 | 199 |
| PER after DMACON off | 6 | 6 | 6 | 6 |
| LC high after PER | 101 | 116 | 131 | 136 |
| LC low after LC high | 0 | 0 | 0 | 0 |
| LEN after LC low | 66 | 82 | 98 | 101 |
| VOL after LEN | 157 | 184 | 210 | 210 |
| DMACON on after VOL | 92 | 108 | 124 | 127 |

### 8.6 Time before a note-on's first write

Within a tick the parser visits voice 0..3; the time from the last write of the previous note-on (same
tick) to the first write of the next note-on is: a fixed part plus about 63 cc for every voice that was
skipped between them without writing, plus the cost of the stream commands executed in between.
Fixed part (previous voice's note-on without loop part, direct neighbour): family 1: 700; family 2: 487;
family 3: 550. The first note-on of a tick after the effects phase: family 1 about 850..930. Per command
cost (cc, +-15): duration command 117, skip 35, call 78..90, return 95..120, jump 150..205, instrument
load: family 1 560, family 2 430..460, family 3 about 150. For family 1 and family 3 the transposition
loop runs before the first write (26 per step; contained in the fixed part above for the corpus
transposition values 9 and 5); in family 2 it runs after the first wait (8.4).
These are fits to the observed gaps; they are good to about +-60 cc.

### 8.7 Effects phase (families 1 and 2)

Gaps between consecutive effect writes of one tick (medians; the effect loop visits the voices in order):
family 1: arp after decay-of-previous-voice 247..285, arp after arp on the next voice 317, decay write
after the arp write of the same voice 273; envelope restart sequence (after the volume write: LEN=1 at
+93..127, PER=1 +0, wait with post time 55, DMACON off, LEN +20, PER +70 (voice 0) / +106 (voice 3), DMACON
on +86 / +121). Family 2: decay write after the decay write of the previous voice 313..365 (after voice 0:
376), arp write of voice 3 after a decay write of voice 0 345. The cost of visiting a voice without a
write is about 60..70 cc per effect part tested.

## 9. Layout parameters per module

All offsets are code hunk offsets. "Words" are 16-bit, "longs" are 32-bit, values in hex.

### 9.1 Boulder Dash (family 1)

| item | value |
|---|---|
| channel records (4 x 0x3A) | 0x0B6 |
| sample base variable (long; initial value is the offset of the sample area) | 0x0B2 (initial 0x1372) |
| period table | 0x2F6 |
| global transposition byte | 0x23E (value 9) |
| stream position variables (4 longs) | 0x216 |
| call-stack pointer variables (4 longs) | 0x226 |
| wait counters (4 bytes), default durations (4 bytes) | 0x236, 0x23A |
| tempo counter word | 0x250 (0) |
| call stack areas | 0x12CC, 0x12F4, 0x131C, 0x1344 (0x28 bytes each) |
| initial stream positions | 0xA26, 0xABE, 0xBBE, 0xC96 |
| hardware preset: length word at 0x1378 (0x2020 -> LEN 0x1010), sample offset | 0x13A2 |
| load address in the reference run | 0x16188 |

### 9.2 Clever & Smart (family 2)

| item | value |
|---|---|
| channel records | 0x138 |
| length table (words), offset table (longs) | 0x1182, 0x11BE |
| period table | 0x2F8 |
| global transposition byte | 0x137 (value 9) |
| stream position variables, call-stack pointers | 0x298, 0x2A8 |
| wait counters, default durations | 0x2B8, 0x2BC |
| tick counter word | 0x2D0 |
| tempo mask (initial value 0x92492492) | long at 0x132 |
| call stack areas (0x28 bytes each) | 0x1102, 0x112A, 0x1152, 0x117A |
| initial stream positions | 0xD3A, 0xDD2, 0xEEA, 0xEFA |
| load address in the reference run | 0x16750 |

### 9.3 Graffiti Man (family 2)

| item | value |
|---|---|
| channel records | 0x13C |
| offset table (longs), length table (words) | 0x1A40, 0x1A6C |
| period table | 0x2F2 |
| global transposition byte | 0x13B (value 7) |
| song variable (word) | 0x0A8 |
| stream position variables, call-stack pointers | 0x29C, 0x2AC |
| wait counters, default durations | 0x2BC, 0x2C0 |
| tick counter word | 0x2D4 |
| tempo mask initial value (long) | 0x136 (0x92492492) |
| call stack areas (0x28 bytes each) | 0x19A0, 0x19C8, 0x19F0, 0x1A18 |
| load address in the reference run | 0x15B50 |

Song table (stream start positions for voices 0..3):

| song | voice 0 | voice 1 | voice 2 | voice 3 |
|---|---|---|---|---|
| 1 | 0x1380 | 0x1440 | 0x14F0 | 0x1518 |
| 2 | 0x15C0 | 0x18E0 | 0x1950 | 0x1978 |
| 3 | 0x1690 | 0x18F0 | 0x1950 | 0x1978 |
| 4 | 0x1748 | 0x1900 | 0x1950 | 0x1978 |
| 5 | 0x17F0 | 0x1910 | 0x1950 | 0x1978 |
| 6 | 0x1850 | 0x1920 | 0x1950 | 0x1978 |
| 7 | 0x18B8 | 0x1930 | 0x1960 | 0x1988 |
| 8 | 0x18CC | 0x1940 | 0x196C | 0x1994 |

### 9.4 Hollywood Poker (family 3)

| item | value |
|---|---|
| records (4 x 16) | 0x0EE |
| period table | 0x1F2 |
| global transposition byte | 0x156 (value 5) |
| stream position variables, call-stack pointers | 0x12E, 0x13E |
| wait counters, default durations | 0x14E, 0x152 |
| call stack areas (voice 0..3) | 0xCBE, 0xCC6, 0xCEE, 0xD16 |
| stream start positions song 1 | 0x862, 0x676, 0x6DE, 0x71E |
| stream start positions song 2 | 0x862, 0x872, 0x8BA, 0x912 |
| hardware preset: length word at 0xD44, sample offset | 0xD6E |
| load address in the reference run | 0x15AD8 |

## 10. Validation

A value model written from this text (not from the driver) was run against the reference logs, comparing
(register, value) per write in order, tick by tick:
- Boulder Dash subsong 1, 25 s: 371 of 371 ticks identical (also 90 s of the timing run).
- Clever & Smart, 25 s: 565 of 565.
- Graffiti Man songs 1..8, 25 s each: 349, 727, 531, 557, 453, 599, 93, 86 of the same (songs 7 and 8 end).
- Hollywood Poker songs 1 and 2: 153 and 157.
- Synthetic modules: Boulder Dash and Clever & Smart with the envelope bit set in some instruments
  (bytes patched in a copy; the envelope restart is not used in the real modules): 1197 and 774 of the
  same.
The 90 s runs (Boulder Dash, Clever & Smart, Graffiti Man songs 1..4, Hollywood Poker songs 1 and 2) also
match to the end (1877, 2080, 2036, 2452, 1808, 1846, 323, 232 ticks). The start writes (hardware preset,
ADKCON) match as well. Timing was checked for self-consistency only (the
numbers of section 8 are fits to the same logs).
