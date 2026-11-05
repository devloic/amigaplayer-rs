# Fred Gray ("gray.*") - behaviour specification

*Sanitised for publication: behaviour only.*

Scope: the sound driver used in four late-1980s games by Fred Gray, as it
appears in the Amiga modules of the archive (module prefix `gray.`, Eagleplayer
"Fred Gray"). Documented from observation of the reference 68k player run
through the host with `DELI_WRITE_LOG` and from the module files themselves;
every number below was checked against those logs (see LOGS.md).

The format is unusual: **the replay code lives inside the module.** The
Eagleplayer is only a thin wrapper (detection, subsong count, a few patches of
the module's code before it is run). The module is an AmigaOS executable
("hunk" file) that holds the game's own music driver, its data tables and its
song sequencer routines. A faithful replayer therefore needs

1. a native implementation of the driver's fixed parts (sections 5-8);
2. a way to execute the module's *sequencer routines*, which are short pieces
   of 68000 code that use a small, closed set of operations (section 7).

The four known modules (Black Lamp, Star Goose, Victory Road, Eco) use three
related driver variants (family A: Black Lamp, Star Goose, Victory Road; family
B: Eco). Their differences are listed in section 9.

All multi-byte values in the module are big-endian. Offsets are relative to the
start of the code hunk's data (not to the file).

## 1. What the music is

Three Paula voices (channels 0, 1, 2; the fourth is never used) play a looping
waveform that lives in the module: a 16-byte pulse wave (family A, plus a 256-byte
sampled sound in Star Goose and Victory Road) or a 16-byte wave that the driver
recomputes 50 times a second (Eco). The driver changes only each voice's period
and volume (and, in some variants, its DMA pointer/length). There is no sample
sequencing: the "instrument" is an envelope + vibrato + pitch-sweep description.

A song is a chain of **sections**. A section is a routine that is called once
per tick; it counts ticks to a step length, and on each step reads one byte
per voice from note arrays (indexed by the position in the section) and starts
notes. The section ends when its position counter reaches the section's length
and the chain continues with another section number.

The tick rate is the vertical blank, 50 Hz; the driver's play routine is called
once per tick and the first Paula register write of a tick happens a few
thousand colour clocks after the tick start (LOGS.md).

## 2. File format

### 2.1 Container

A standard AmigaOS hunk executable with exactly one code hunk:

- long 0x000003F3 (HUNK_HEADER), long 0 (no resident library names), long 1
  (table size), long 0 (first hunk), long 0 (last hunk), then one long: the
  hunk size in longwords with the memory-type flags in the top two bits (the
  four modules have 0x40 in the top byte: chip RAM);
- long 0x000003E9 (HUNK_CODE), long = size in longwords, then the data;
- long 0x000003EC (HUNK_RELOC32): groups `count, hunk number (0), count
  offsets`, terminated by a zero count; every listed offset holds a 32-bit
  address inside the hunk that the loader adjusts by adding the hunk's load
  address; then long 0x000003F2 (HUNK_END).

The simplest correct treatment is to place the hunk data at a load address
`base` in a flat memory image and add `base` to every relocated long. The
sequencer routines and the driver then see the same absolute addresses the real
machine would. (A port that keeps `base = 0` works too provided the pointer
values written to Paula are `base + offset`; the logs in this folder
contain the real load address.)

### 2.2 Detection

A file is a Fred Gray module when all of these hold (this is the check the
reference player makes):

- the first long is 0x000003F3;
- the byte at file offset 0x14 (the top byte of the hunk size long) is non-zero;
- starting at file offset 0x20 (the first data byte of the code hunk) the
  longs are 0x70FF4E75, then the ASCII text `FRED`, then `GRAY`, followed by
  three further longs, all non-zero (see 2.3).

The archive names these files `gray.<game>`. Nothing else is checked.

### 2.3 Header (hunk data offsets)

| offset | meaning |
|---|---|
| 0x00 | long 0x70FF4E75 (a tiny "not an executable" stub, never run) |
| 0x04 | `FRED` |
| 0x08 | `GRAY` |
| 0x0c | entry offset of the **play** routine (called once per tick) |
| 0x10 | entry offset of the **init** routine (called with the subsong number in the low bits of the first data register) |
| 0x14 | number of subsongs |
| 0x18 | offset of the title string (zero terminated, e.g. `Black Lamp`) |
| 0x1c | offset of the author string (`Fred Gray`) |
| 0x20 | offset of the copyright text (several lines, ends in `Ripped and adapted by Don Adan/Wanted Team !`) |
| 0x24 | offset of the end of the driver's data (the code hunk's last used byte) |
| 0x28 | offset of one table inside the instrument/sample area (not needed for replay) |

Title and copyright strings are plain Latin-1/ASCII.

### 2.4 Subsong table

One byte per subsong, immediately in front of the init routine (the
byte just before the init entry belongs to the table when the count is even; if
the count is odd the table is padded by one byte, so the table start is
`init_entry - ((count + 1) & ~1)`). The byte is the number of the section the
subsong starts with. Examples: Black Lamp 01 0c 0e 13; Star Goose 01 04 0a 1a 1f;
Victory Road 01 02 0b 0d 18 1c 1d 1e; Eco 01 02 07 08.

### 2.5 Length and song end

There is no stored length. A subsong ends the first time the sequencer finds
section number 0 current (section 0 is a "stop" marker). Subsongs that loop forever
never reach it. Number of ticks until the first end (measured with the
reference, subsong: ticks) - Black Lamp 1: 1, 3: 343; Star Goose 0: 1; Victory
Road 0: 1, 1: 3221, 2: 181, 4: 1933, 5: 31, 6: 17, 7: 81; Eco 0: 1, 3: 2049.
Subsongs not listed (Black Lamp 0 and 2, Star Goose 1-4, Victory Road 3, Eco 1 and
2) loop and never end. Subsong 0 of Star Goose, Victory Road and Eco, and
subsong 1 of Black Lamp, end in the first tick without sound (they are
"empty"; the host skips them).

### 2.6 Where the driver's tables are

The tables and variables are at fixed offsets inside each module but the
offsets differ per module. The table of section 2.7 gives them for the four
known modules; for another module of the same family they are located as
follows (all by following references, not by scanning for text):

- The **play** routine's first actions are fixed: it loads the base of the
  channel-record array, runs the sequencer-control routine once, runs the
  per-channel update for each channel and finally the Paula output stage. The
  sequencer-control routine contains the absolute addresses of the command
  variable, the section-number variable, the tick counter, the position word and
  the section jump table. Everything else follows from these.
- The **section jump table** is a list of 32-bit absolute addresses; entry 0
  points at a one-instruction "return" routine located directly behind the
  table, so the table ends where that return routine starts.
- The **length table** (one big-endian word per section number) follows the
  section jump table closely; the **voice table** (4-byte entries), the
  **instrument table** (16-byte entries) and the **channel records** follow in
  data. The instrument table is entered at number 1 (entry 0 is not used).
- The **envelope table**, **vibrato table** and **period table** are read by
  the per-channel routines: the period table has 48 words (4 steps per
  semitone), the vibrato table has up to 128 signed bytes (see 6.5), the envelope
  table is addressed by a byte offset (see 6.4).

### 2.7 Layout table of the known modules

All values are hunk-data offsets. "state" is the channel-record array (3
records); "stride" is the distance between records.

| item | Black Lamp | Star Goose | Victory Road | Eco |
|---|---|---|---|---|
| driver family | A | A | A | B |
| play / init entry | 0xe2 / 0x8c | 0xe4 / 0x8e | 0xec / 0xa6 | 0x1da / 0xa6 |
| subsong table | 0x88 | 0x88 | 0x9e | 0xa2 |
| subsongs | 4 | 5 | 8 | 4 |
| state | 0x10d8 | 0x1992 | 0x19b0 | 0x14fa |
| stride | 0x20 | 0x20 | 0x20 | 0x24 |
| command byte (new song) | 0x1044 | 0x1854 | 0x1814 | 0x1438 |
| pending-section byte (follows the command byte) | 0x1045 | 0x1855 | 0x1815 | 0x1439 |
| current-section byte | 0x1046 | 0x1856 | 0x1816 | 0x143a |
| tick counter byte | 0x1040 | 0x1850 | 0x1810 | 0x1434 |
| position word | 0x1042 | 0x1852 | 0x1812 | 0x1436 |
| transposition word | - | 0x184e | 0x180e | 0x1432 |
| volume cap byte | - | 0x185a | 0x181a | 0x143e |
| type mask byte | 0x1049 | 0x1859 | 0x1819 | 0x143d |
| voice-refresh flag byte | - | 0x998 | 0x181c | - |
| wave phase word | - | - | - | 0xd24 |
| section jump table | 0x104e | 0x1860 | 0x1822 | 0x1444 |
| length table | 0x10a0 | 0x191a | 0x193c | 0x14ac |
| voice table | 0x10c8 | 0x1976 | 0x198c | 0x14de |
| instrument table (entry 0) | 0x1128 | 0x19e2 | 0x1a00 | 0x1556 |
| envelope table | 0x54a | 0xaa8 | 0x8d2 | 0x1706 |
| vibrato table | 0xfc0 | 0x17ae | 0x175e | 0x13a2 |
| period table | 0xf60 | 0x174e | 0x16fe | 0x1342 |
| percussion-mask table | - | 0x182e (32 B) | 0x17ee (16 B) | 0x1422 (16 B) |
| channel-mode flag bytes (3) | - | 0xdd4 | 0xc2a | - |
| synth wave (16 B) | 0xd2 | 0xd4 | 0xc2e | 0xc64..0xc83 region |
| sampled sound (256 B) | - | 0x1bb2 | 0x1c00 | (0xc84.. region, 376 B) |
| sections | 20 | 46 | 70 (40 used) | 25 |

Section routine entry points (what to execute for section n) are read from the
section jump table; the entry points of the shared sequencer helpers that the
section routines call are in 2.8.

### 2.8 Entry points of the helpers the section routines call

Section routines call a small set of shared helper routines located elsewhere
in the module's code. To recognise a call you need the helpers' entry offsets
(module-relative, inside the module's own data):

| helper | Black Lamp | Star Goose | Victory Road | Eco |
|---|---|---|---|---|
| tick counter / step test | 0x482 | 0x9c0 | 0x7ea | 0x92a |
| voice select | 0x5aa | 0xb48 | 0x972 | 0xa12 |
| percussion trigger | - | 0x972 | 0x79c | 0x8de |
| note, voice A (nibble form) | 0x68e | 0xc6a | 0xa94 | 0xb42 |
| note, channel 0 | 0x6aa | 0xc8a | 0xab4 | 0xb62 |
| note, channel 1 | 0x6ae | 0xc90 | 0xabe | 0xb6c |
| note, channel 2 | 0x6b2 | 0xc96 | 0xac8 | 0xb76 |
| end of step: advance | 0x658 | 0xc30 | 0xa5a | 0xb08 |
| end of step: jump to section d1 | 0x62a | 0xc00 | 0xa2a | 0xad8 |
| reset all voices | 0x46e | 0x9a4 | 0x7ce | 0x90e |

A section routine reaches them with a subroutine call, except the "end of
step" pair which is entered by a plain jump at the end of the routine. Their
behaviour is specified in 6.3. They are *not* to be executed as code: implement
them natively and trap the calls.

## 3. Working memory

The driver keeps all its variables, the channel records and the tables in the
module's data hunk itself. A replayer must therefore hold the hunk in a
read/write memory image; the song's *initial* variable and channel-record
contents are the bytes stored in the module file (they are zero except for the
bytes noted in 4). Section routines read and write this memory directly
(variables of 2.7, channel-record fields through the record base).

## 4. Channel record

Three records, `stride` bytes apart, starting at "state". Offsets inside a
record (all in bytes; "w" = big-endian word):

| off | size | meaning |
|---|---|---|
| 0x00 | w | period value computed this tick (before the output doubling) |
| 0x02 | b | note number of the last note (not used for sound) |
| 0x04 | b | envelope counter, 15 right after a note start, counts down to 0 |
| 0x05 | b | current volume level (envelope output) |
| 0x06 | b | by-product of the envelope step (not used for sound) |
| 0x07 | b | channel number 0/1/2 (initial value in the image); a shift count in 6.6 |
| 0x08 | b | envelope sub-tick counter |
| 0x09 | b | always 0 (offset of the by-product lookup) |
| 0x0a | w | current pitch (note number * 4; 4 units per semitone) |
| 0x0c | w | target pitch |
| 0x0e | w | slide step (signed); initial image value 1 |
| 0x10 | w | instrument countdown (ticks) |
| 0x12 | b | vibrato selector (see 6.5) |
| 0x13 | b | vibrato delay in ticks |
| 0x14 | b | envelope row offset |
| 0x15 | b | envelope speed (ticks per envelope step) |
| 0x16 | b | number of the instrument to start when the countdown ends |
| 0x17 | b | instrument flags (6.6) |
| 0x18-0x19 | | unused |
| 0x1a | b | type-mask bits (initial image: channel 0: 0x09, 1: 0x12, 2: 0x24) |
| 0x1b | b | instrument request (non-zero = load that instrument entry) |
| 0x1c | w | vibrato offset, signed, added to the pitch |
| 0x1e | b | vibrato delay counter |
| 0x1f | b | vibrato phase, 0..15 |
| 0x20 | long | **Eco only**: address of the channel's Paula pointer register pair (0x00DFF0A0, 0x00DFF0B0, 0x00DFF0C0) |

Offsets 0x0a-0x19 are loaded as one 16-byte block from the instrument table
(6.6) and are the "instrument entry" layout: +0 current pitch, +2 target pitch,
+4 slide step, +6 countdown, +8 vibrato selector, +9 vibrato delay, +10
envelope row offset, +11 envelope speed, +12 next instrument, +13 flags, +14/15
unused.

The *initial image* of the three records is part of the module (bytes 0x07, 0x0f
(= slide step low byte 1), 0x1a and, for Eco, 0x20..0x23 are non-zero); copy it
verbatim.

## 5. One tick

The play routine does, in this order:

1. (Eco only) recompute the wave buffer (8.4).
2. **Sequencer control** (6.1).
3. For channel 0, 1, 2 in turn: **envelope step** (6.4), **slide** (6.2),
   **vibrato** (6.5), **instrument load / countdown** (6.6), **period
   computation** (6.7), and in family A (not Eco) a final "export" stage that
   copies a field the output never uses (omit it).
4. A "foreign trigger" stage that copies three bytes from locations nobody
   writes to the instrument-request bytes; it never does anything audible. Omit
   it.
5. **Paula output** (6.8).

Section routines (6.1) run during step 2 and may change channel records, so they
act before the channel stages of the same tick.

## 6. Behaviour

### 6.1 Sequencer control and sections

State: command byte `cmd`, pending byte `pend`, current section byte `cur`,
tick counter `cnt` (byte), position `pos` (word), transposition `trans`
(word, variants other than Black Lamp), refresh flag `vflag`, cap `cap`.

At the start of every tick:

1. If `cmd` is non-zero: `pend` := `cmd`; `cmd` := 0; (Black Lamp: nothing
   more; Star Goose, Victory Road and Eco: `trans` := 0; Star Goose and Victory
   Road also `vflag` := 0; Eco also writes the value 8 to the channel-2 DMA
   length register at this point); then **reset all voices** (6.3): request
   instrument 1 on all three channels (instrument-request byte := 1) and, in all
   variants but Black Lamp, `cap` := 15.
2. If `pend` is non-zero: `cur` := `pend`; `pend` := 0; `cnt` := 0; `pos` := 0.
3. Run the routine for section `cur`: the entry address is the 32-bit
   value at `jump table + 4 * cur`.
   - Section 0 is the "stop" marker: nothing runs, but the song-end signal of
     2.5 is raised on every tick while `cur` = 0 (the host decides what to do;
     the channel stages and Paula output of the tick still run).
   - Section n > 0 is executed as described in section 7. It ends by returning
     (or by tail-jumping into one of the "end of step" helpers).

`cmd` is set exactly once, by the init routine (10.1). Everything else is
driven by the sections themselves.

### 6.2 Slide (pitch sweep)

Let `a` = current pitch and `c` = target pitch, both sign-extended 16-bit
values, `e` = slide step (sign-extended). If `a = c` do nothing. Otherwise, if
`a` is greater than `c` when both are compared as unsigned 32-bit numbers (the
sign-extended values), `a` := `a - e`; else `a` := `a + e` (stored modulo
65536; no clamping, so a step that does not divide the distance makes the
pitch oscillate around the target). The same rule happens every tick.

### 6.3 Helpers called by section routines

All helpers are implemented natively; their inputs are the registers
set by the section routine.

**Tick counter / step test** (input: low byte of data register 1 = speed `s`).
Let `c` = (`cnt` + 1) & 255. If `c` = `s`: `cnt` := 0, data register 7 low word
:= `pos`, and the section continues. Otherwise `cnt` := `c`, data register 7 low
word := `pos`, and **the section routine is abandoned** (it returns at once; the
rest of the tick proceeds). So a section does its work every `s`-th tick; the
position counter advances by one per step.

**Voice select** (input: data register 0 = three voice numbers, byte 0 for
channel 0, byte 1 for channel 1, byte 2 for channel 2). Copies the 4-byte entry
`voice table + number` into channel-record bytes 0x12..0x15 of the matching
channel (selector, delay, envelope row, speed). Family A guard (Star Goose,
Victory Road): the copy is done only when `vflag` is non-zero or `pos` = 0; when
it is done `vflag` := 0. Black Lamp and Eco always copy.

**Percussion trigger** (family A except Black Lamp, and Eco; input: low byte of
data register 0 = mask `m`): let `i` = `pos` & 31 (Star Goose) or & 15 (Victory
Road, Eco); `r` := `m` AND `mask table[i]`. Star Goose / Victory Road: `vflag` :=
`r`; if `r` is non-zero channel 0's instrument-request byte := 27. Eco:
the DMA length register of channel 2 is written 8; if `r` is non-zero it is
then written 188 and channel 2's instrument-request byte := 6.

**Note, one voice** (entry for channel k; input: data register 7 low word = index
`i` (sign-extended), data register 0..? a0 = note array address, and the
"transposition" register: the low word of data register 2 in Black Lamp, the low
word of address register 1 in the others). Fetch `n` = `array[i]`. If `n` = 0:
rest, nothing happens. Victory Road only: if bit 6 of `n` is set the note is a
"kill" - channel k's envelope row offset (0x14) := 0, `vflag` := 1, nothing
else. Otherwise `t` := `n` + transposition (16 bit); clear bit 7 (the "glide"
bit; **its normal behaviour of sliding to the new pitch is disabled in the
reference: the pitch always jumps at once**); then, in all but Black Lamp: `t` :=
`t` & 0x7f and `t` := `t` + `trans`. Record `t` (low byte) in the note field;
target pitch := `t` * 4; current pitch := `t` * 4; then the **note start** (below).

**Note, voice A** (channel 0 only; a "nibble form" used by a few sections): fetch
`n`; if the low nibble of `n` is 0 nothing happens; otherwise channel 0's
vibrato selector (0x12) := high nibble of `n` shifted as `n & 0xf0` and the
note is `n & 0x0f` (same processing as above from the transposition step on,
without the kill test).

**Note start** (common to the note helpers, to instrument load and to
the period stage of a freshly loaded instrument): vibrato delay counter := vibrato
delay (0x13); envelope sub-tick counter := 0; vibrato phase := 0; envelope
counter := 15; then one envelope step (6.4, "first part": decrement the counter
and compute the volume, without the sub-tick test). (Eco also sets the wave
phase word to 0 here.)

**End of step - advance** (tail of every ordinary section): let `L` =
`length table[cur]` and `p` = (`pos` + 1) & 0xffff. If `p` = `L`, `pend` := `cur` +
1. In any case `pos` := `p`. **End of step - jump** does the same but
`pend` := data register 1 low byte (the target section) when `p` = `L`.
When `pend` is set, the change is made at the start of the next tick (6.1,
item 2).

**Reset all voices**: instrument-request byte of channels 0..2 := 1; in all
variants but Black Lamp, `cap` := 15. (Instrument 1 is a silent entry, see 6.6.)

### 6.4 Envelope step

Part of the per-channel stage, run before the slide. `sub` = sub-tick counter,
`spd` = speed (0x15), `ctr` = envelope counter (0x04).

1. `sub` := `sub` + 1 (byte). If `sub` != `spd` (byte compare) stop.
2. `sub` := 0. If `ctr` = 0 stop (the volume level stays).
3. `ctr` := `ctr` - 1; index := ((`ctr` XOR 15) + row offset) & 255; level :=
   `envelope table[index]`; in every variant but Black Lamp the level is
   limited to `cap` (level := min(level, cap)); volume level := level.

(The by-product written into byte 0x06 is irrelevant.) The "first part" used by
the note start omits steps 1 and the `sub` clear.

### 6.5 Vibrato

Phase := (phase + 1) & 15 (byte at 0x1f). `idx` := phase OR selector (0x12)
(byte OR). `v` := signed byte at `vibrato table[idx]`, sign-extended to 16 bits
into the vibrato offset (0x1c). If the delay counter (0x1e) is non-zero: it is
decremented and the vibrato offset is set to 0 that tick. Selector bytes
(0x12) are multiples of 16 in all songs except a few instruments (e.g. 0x58 in
Black Lamp) where the low bits are simply OR-ed into the index as written.

### 6.6 Instrument load, countdown and the type mask

Run after the vibrato.

- If the instrument-request byte (0x1b) is non-zero `r`: copy the 16 bytes at
  `instrument table + 16 * r` to record bytes 0x0a..0x19; request byte := 0;
  then
  - family A except Black Lamp: `shift` := byte 0x07 (the channel number);
    `m` := `type mask` OR byte 0x1a; `m` := `m` AND (255 XOR ((flags (0x17)
    << shift) & 255)); store `m` in `type mask`. (Black Lamp keeps the same
    update in its own mask byte, unused by output.)
  - Eco: wave select: write the long `((flags << 4) & 255) + wave base` to the
    channel's pointer register pair - the address stored in the record at
    0x20 - i.e. two word writes, high word to the register and low word to the
    register + 2 (`wave base` = module offset 0xc64 + load base; flags values 0..5
    pick the 16-byte waves at offsets 0xc64.. 0xcb4).
  - then perform a **note start** (6.3) for the channel.
- Otherwise decrement the 16-bit countdown (0x10); if it becomes 0, the
  request byte := byte 0x16 (the next instrument, which is loaded next tick).
  A request of 0 is "none".

Instrument 1 (the reset entry) has: current/target pitch 0x208, step 0,
countdown 1, selector/delay/row/speed 0, next 0, flags 1. It sets the voice
to a very low pitch with envelope row 0 (level 0): silence.

Instrument table entries are 16 bytes; the layout is the one in section 4
(byte 0x0a..0x19). The numbers of defined entries: Black Lamp 30, Star Goose
28, Victory Road 31, Eco as many as the table before the wave area allows.
Whatever the entries contain, the replayer just copies them.

### 6.7 Period computation

`p` := (current pitch + vibrato offset) mod 65536 (unsigned 16 bit);
`octave` := `p` div 48; `step` := `p` mod 48; `base` := `period table[step]`
(word). `shift` := `octave` & 255; if non-zero, `base` := `base` >> (`shift` &
63) (a shift of 16 or more gives 0). Record 0x00 := `base`.

### 6.8 Paula output

After all three channels, write in this order:

1. Period registers, channel 0, 1, 2: AUDxPER = (record 0x00 << 1) & 0xffff;
2. Volume registers, channel 0, 1, 2: AUDxVOL = (volume level << 2) & 0xff
   (values above 63 are written as is; none occur).
3. Variants with channel-mode switching (Star Goose, Victory Road): see 8.3.

Eco has no step 3. Section routines and the instrument stage may also write
DMA pointer / length registers while they run (Eco; section 8.4), those
writes happen where the code runs, i.e. before the period/volume writes of
the same tick in the case of section routines and instrument loads.

Register addresses: AUD0 = 0xDFF0A0, AUD1 = 0xDFF0B0, AUD2 = 0xDFF0C0 (pointer
high word at +0, low word at +2, length at +4, period at +6, volume at +8).

## 7. Executing a section routine

A section routine is 68000 machine code. They use only a small subset of the
instruction set; implement an interpreter for it (registers D0-D7, A0-A7, the
condition flags Z, N, C, V; byte/word/long sizes; 16 MB address space is enough).
The subset used by all four modules:

- data movement: move (byte, word, long) between data registers, address
  registers (movea), memory with absolute short/long address, register
  indirect with displacement, indexed (register + data register word + 8-bit
  displacement), immediate; quick move; load effective address from an
  absolute address; clear and test (byte, word, long);
- arithmetic/logic: and, or, eor, add, sub, compare with immediate operands;
  add/sub quick; the register forms used on D0/D7 (shifts left/right with an
  immediate count, bit test on a data register with an immediate bit number);
- control: conditional branches (equal, not equal, carry set, carry clear
  and the other usual conditions, short and long forms), unconditional branch,
  jump to an absolute address, subroutine call, return.

Execution rules:

- On entry data register 7 low word = `pos`; address register 6 = the base of
  the channel-record array (sections reach the instrument-request bytes of the three channels as
  fields 0x1b of records 0, 1, 2); the custom-chip registers are accessed by absolute address
  (0x00DFF0xx); every other register starts undefined (sections always set
  what they use).
- A subroutine call, **or an unconditional branch/jump**, whose target is one
  of the helper entries of 2.8 invokes the helper natively; a helper reached by
  a subroutine call returns to the next instruction (or abandons the routine,
  see "tick counter"); a helper reached by branch/jump ends the section routine
  afterwards. Any other target is ordinary code.
- Memory writes into the 0x00DFF000 page are Paula register writes: append
  them to the tick's output in execution order. A long write is two word writes
  (high word to the address, low word to address + 2) with the same colour
  clock.
- A return ends the routine. (Sections never nest.)

What the routines of the four modules actually do: call the tick counter with
a speed in data register 1; select voices; call note helpers with a note array
address in address register 0 and a transposition, usually after masking data
register 7 (`pos`) to wrap the array; trigger percussion; request instruments by
storing numbers to the instrument-request bytes (including from arrays: byte
`array[pos & mask]` stored as a request); modify `trans` and `cap` (a few
position tests and an arithmetic update, e.g. a fade-out derived from the
position); write DMA pointers/lengths (Eco); call reset; set the next section
(a plain store to `pend`) or clear `cur` (stop); and finally enter the "end of
step" helper.

## 8. Paula pointer / length protocol

### 8.1 Init (host-level, before the first tick)

The init routine, called with the subsong number, stores the start-section
number (subsong table, 2.4) into `cmd`, points all three voices at the module's
silent/pulse wave and programs them (in this order, as long and word writes
with strictly increasing colour clock): AUD0 pointer (as high word then low
word), AUD1 pointer, AUD2 pointer, AUD0 length, AUD1 length, AUD2 length,
AUD0 volume, AUD1 volume, AUD2 volume, and (Eco only) the three periods 0x190,
before enabling the three voices and the master DMA bit with DMACON = 0x8007.
Length is 8 words (16 bytes). The pointer is the module address of the 16-byte
wave: offsets 0xd2 (Black Lamp), 0xd4 (Star Goose), 0xc2e (Victory Road);
Eco: AUD0 -> 0xc84 and AUD1, AUD2 -> 0xc74 (the wave buffer, 8.4).

In the reference host, the init routine is run twice per subsong start: the first
run is immediately followed by DMACON = 0x000F (all four voices off) and writes of
volume 0 to the four volume registers (the "check run"); the real run follows.
Neither is part of the format.

### 8.2 Silence wave contents

Black Lamp: bytes 7F x14, 81 x2 at 0xd2 (family A pulse), Star Goose 3F x14,
C1 x2 at 0xd4, Victory Road 3F 3F at 0xc2e ... the real bytes are always the
module's own; the replayer takes the 16 bytes from the module image at the
pointer.

### 8.3 Voice mode switching (Star Goose, Victory Road)

Per channel there is a mode flag byte (three bytes at the "channel-mode flag"
offset; the flag for channel 0 is the **last** of the three bytes, channel 2's
is the first). After the three period and three volume writes, process the type
mask:

1. For channel 0, 1, 2 (mask bits 0, 1, 2): if the mask bit is clear and the
   channel's mode flag is non-zero: write the channel's pointer (synth wave) and
   length 8 and clear the flag.
2. For channel 0, 1, 2 (mask bits 3, 4, 5): if the mask bit is clear and the
   flag is zero: write the pointer (sampled sound) and length 128 words and set
   the flag to 255.

Mask bits come from instrument loads (6.6): at load time the channel's pair of
bits (b, b+3) are first set (OR of record 0x1a) and then cleared according to
the instrument's flags: flag bit 0 (synth) clears the low bit, flag bit 3
(sample) clears the high bit; both set clears both (the channel then flips
between the two modes every tick). Writes go pointer high word, pointer low
word (same clock), length (19 colour clocks later); channel 0 uses 0xA0-0xA4,
channel 1 0xB0-0xB4, channel 2 0xC0-0xC4. The pointer values are the load address
plus the offset in the layout table.

### 8.4 Eco wave buffer and wave select

A 16-byte buffer at module offset 0xc74 is recomputed at the start of every
tick: `phase` := (`phase` + 1) & 15 (word at the "wave phase" offset); for
i = 0..15: `buf[i]` := (`src[i]` + `src[phase + i]`) & 255, where `src` is the 32-byte
table at 0xd04 (so for phase + i >= 16 the second 16 bytes are read). A note start
sets `phase` to 0. The buffer is the wave of channels 1 and 2 (and channel 0
uses a separate wave, 0xc84, which is static until an instrument selects a
wave). Instrument loads write the pointer pair to one of the 16-byte waves
starting at 0xc64 (flags 0 -> 0xc64, 1 -> 0xc74 ...) as described in 6.6.
Sections write fixed wave pointers (0xc84, 0xc74, 0xc64..) directly with long
writes to the pointer registers. The percussion helper of Eco switches
channel 2 between a 16-byte wave (length 8) and a 376-byte one (length 188) that
plays the bytes after 0xc84.

## 9. Variant differences (summary)

| feature | Black Lamp | Star Goose | Victory Road | Eco |
|---|---|---|---|---|
| record stride | 0x20 | 0x20 | 0x20 | 0x24 |
| transposition word `trans` | no | yes | yes | yes |
| volume cap `cap` | no | yes | yes | yes |
| voice-select guard (`pos` = 0 or refresh flag) | no | yes | yes | no |
| percussion trigger | no | yes (32-entry mask) | yes (16-entry mask) | yes (also DMA length) |
| kill note (bit 6) | no | no | yes | no |
| per-instrument mode switching of DMA pointers | no | yes | yes | no (wave select instead) |
| wave buffer recomputed each tick | no | no | no | yes |
| write of 8 to AUD2 length at song start | no | no | no | yes |
| note: `t & 0x7f` and `+ trans` | no | yes | yes | yes |
| glide bit effective | no (disabled) | no | no | no |

## 10. Host-level behaviour

### 10.1 Init

See 8.1. After init the host calls the play routine once per tick; the very
first tick sets up the sections from the start section stored in `cmd`.

### 10.2 Song end

When section 0 is current the module's own "stop" routine is the host's
song-end callback (the Eagleplayer replaces the first jump-table entry by a
call into the host). It is invoked once per tick while section 0 is current.
The reference host reacts to it by stopping all voices (DMACON 0x000F, volumes
0) and, for the first two seconds of a silent subsong, going on to the next subsong
(that explains the repeated init sequences in the first part of some logs); a
subsong that has played sound is stopped after a small number of ticks (0-2
ticks after the first section-0 tick in the logs). A replayer reports a song
end on the first tick on which `cur` = 0.

### 10.3 Wrapper patches that change behaviour

The Eagleplayer patches the module's code once, right after loading it, in two
ways that matter:

- the first jump-table entry is replaced by a call that reports the song end
  (10.2);
- in the note-start code, the "slide to the new pitch if the glide bit was set"
  decision is removed (the code that saves and restores the status
  register around the glide test is overwritten by no-ops, and with it the
  conditional branch), so **a note always sets the current pitch to the target
  at once** (6.3). All section data containing bytes with bit 7 set behave
  the same as the same bytes with it clear.

## 11. Validation

A model that executes the section routines with the subset interpreter of
section 7 and implements 6 and 8 natively reproduced the write logs of the
reference 68k player register for register, in order, for every subsong of the
four modules, for as long as the log runs (up to 400 s of play or until the
song end), including the pointer/length writes (pointer values equal up to the
constant load address). LOGS.md lists the runs and the timing model.

For the Eco wave buffer, the model's bytes were also compared with the guest
memory of the original driver after every tick: identical.

## 12. Start phase: time from reset to the first init and to the first tick

All times are colour clocks from the start of the run (the reference run's
clock; the host places the start of the player at 100 colour clocks into a
video frame, which is already inside the constant below). Write times are the
ones in the logs.

### 12.1 What happens before the first init

The wrapper loads the module (the host's load costs no guest time), then
patches it (section 10.3). To find the places to patch it walks the module's
code image one 16-bit word at a time, in four consecutive stages, starting at
the **play entry** (header long 0x0c). Each stage has its own cost per word
stepped over. Then it counts the entries of the section jump table, and the
host runs the init routine. Nothing else depends on the module: the size of the
hunk, the number of relocations, the file size and the subsong number were
varied in test copies and did not change the start by a single colour clock.

Stages (word values are the markers searched for):

| stage | scan | cost per word (cc) |
|---|---|---|
| A | from the play entry up to and including the first word 0xE586 | 10.5 |
| B | the section jump table: starting with entry 1, entries up to the terminator | 30 per entry |
| C | from the word after that marker to the first word 0x40E7, then on to the first word 0x46DF after it | 22 |
| D | from just after the patched area behind the 0x46DF marker, to the first long 0x3BBC0080, or to the limit (play entry + 0xD48) if none is found | 26 |

Counts to use:

- A = number of words examined from the play entry to the marker, inclusive.
- B = 1 + (position of the terminator - address of entry 1) / 4, where the
  terminator is the first table position whose first word is 0x4E75 (the return
  routine that follows a complete table) or whose long is zero.
- C = number of words stepped over from the word after the 0xE586 marker up to
  the 0x46DF marker (both 0x40E7 and 0x46DF are looked for in turn, 22 cc per
  word throughout).
- D = number of words stepped over from the end of the patched area (4 bytes
  behind the 0x46DF marker, 6 bytes if the low byte of the word that follows the
  marker is zero) to the 0x3BBC0080 long or to the limit.

Time of the first write of the first init (the first line of every log,
first period of the "check" run):

    T_first = 1686 + 10.5*A + 30*B + 22*C + 26*D
              + 11  if the table's terminator is a zero long (not 0x4E75)
              + 14  if the 0x3BBC0080 long was found before the limit
              + 8.5 if the low byte of the word behind the 0x46DF marker is zero
              - 6   if the init routine starts its Paula programming 6 cc
                    sooner (Eco: 52 cycles from its entry to the first write
                    instead of 64)

Constants 1686 and the 11, 14, 8.5, 6 terms are measured (they are the
combined cost of the fixed parts: the rest of the wrapper's set-up, the
host's call overhead and the first instructions of the init routine up to its
first write). The wrapper's own initialisation of its state area (clearing 80
bytes, 28 cycles per word) lies inside the 1686.

Values for the four modules (A, B, C, D, flags; measured / formula):

| module | A | B | C | D | zero end | found | long form | T_first measured / formula |
|---|---|---|---|---|---|---|---|---|
| Black Lamp | 62 | 20 | 696 | 938 | no | no | no | 42637 / 42637 |
| Star Goose | 75 | 46 | 1442 | 123 | no | yes | yes | 38798 / 38798 |
| Victory Road | 78 | 70 | 1219 | 126 | no | yes | yes | 34722 / 34721.5 |
| Eco | 80 | 26 | 1174 | 441 | yes | no | yes | 40613 / 40613.5 (init 6 cc earlier) |

Half colour clocks are rounded to nearest. Residual 0 to 1 cc on the four known
modules.

Validation on edited copies of the modules (every row measured with the
reference, formula applied afterwards): ten single edits (Black Lamp with shortened tables, markers moved
earlier or later, a marker inserted; Star Goose and Victory Road with the
0x3BBC0080 long removed) reproduce to 0 or 1 cc; four combined
edits of Eco, Star Goose and Victory Road (cut tables, moved markers, changed
flag, 12000 bytes of padding and 60 relocations removed) 38201, 33125, 40613 and
31908 measured against 38201, 33127, 40613.5 and 31908.5 predicted (the Eco edit with a changed flag and a cut table is off by 2 cc, on the Eco-style figure 33125 against 33127). The first
init of a run is the only one the formula covers; a later init run (new
subsong) starts 1029 cc (Star Goose, Black Lamp, Victory Road) or 1073 cc (Eco)
after the previous one.

### 12.2 From the last init to the first tick

The host starts the 50 Hz timer at the moment the module's init routine
returns after the **last** init run of the start sequence (it is not restarted by
later subsong changes). The first timer interrupt is exactly 70940 colour
clocks later (tested: adding 16 cc of idle time before the init routine's
return moved the first tick by exactly 16 cc).

Define L as the time stamped on the final DMACON write (0x8007) of that last
init run. The init routine ends right after that write (a return, 8 cc). The
interrupt wrapper then does about 118 cc of preparation before it calls the
play routine, so the play routine is entered at about

    L + 70940 + 126  (observed 122 to 131: Black Lamp 122-124, Star Goose and
                      Victory Road 131, Eco 126; the spread is the error of
                      the per-tick cycle estimate, not a module property)

and the first period write of tick 0 happens (standard cycles from the play
entry to that write) / 2 later. For later ticks add 70940 per tick.
Equivalent for a host that keeps tick starts at "play entry": delay between
the end of init's final write and the play entry = 126 +/- 5 cc.

### 12.3 What is still unexplained

- The 1686 constant is a fit of the fixed costs, not a sum of itemised parts.
- The 12 to 25 cc jitter of the in-tick timing (section 11, LOGS.md) also
  limits how exactly the delay of 12.2 can be fixed.
- Behaviour with a hunk whose play entry lies so that the scan limit falls
  outside the hunk is not defined (the wrapper then reads beyond the module).
