# Benn Daglish SID: behaviour specification

*Sanitised for publication: behaviour only.*

*Sanitised for publication: behaviour only.*

Format: "Benn Daglish SID" (Benn Daglish's music for early Teque games, 1987-88, with the game's own
Amiga sound driver; Wanted Team adaptation 2004). Eagleplayer file `BenDaglish-SID`, internal player
name "Benn Daglish SID" (with the spaces), file name prefix `BDS.`, slug `bendaglishsid`.

This spec is written for a port that **runs the 68000 sound driver embedded in each module on the
repo's 68000 core** and reproduces what the Eagleplayer adds around it, in the way of the Riff Raff
port. All numbers were checked against the reference 68k player run through the host with
DELI_WRITE_LOG on the three real modules known (Terramex, Chubby Gristle, The Flintstones), all
five subsongs, up to song end. A model built from this text reproduces every logged register write
(address, value, order and colour clock) of all of them (LOGS.md).

## 0. Summary

A BDS module is an AmigaDOS executable: one relocatable code hunk holding the game's own 68000
sound driver, its song data, waveforms and samples, behind a small header that names three driver
entry points: a **play routine** (once per tick), an **audio-interrupt routine** (run on every audio
interrupt) and an **init routine** (subsong start). The Eagleplayer:

1. loads and relocates the hunk;
2. turns the audio-interrupt routine's exception return into a plain return and notes three voice
   volume bytes for its display;
3. replaces five fixed code shapes of the driver with calls into itself (song-end detection,
   display bookkeeping, a raster-line wait in place of a busy loop);
4. calls the init routine at each subsong start, the play routine once per tick, and the
   audio-interrupt routine whenever Paula raises an audio interrupt;
5. reports song end in two ways (section 7).

A port does the same: it runs the driver's instructions on the 68000 core with the repo's cycle
table and treats every replaced code shape as a host trap with the behaviour and cost of section 6.
Notes, envelopes, waveforms and sequencing are whatever the driver's own code does; this spec does
not need to describe them.

All values are big-endian. "Hunk offset" is an offset from the first byte of the code hunk's data
(file offset 0x20). Cycles are 68000 cycles of the cycle table; colour clocks (cc) are the host's
time unit; one colour clock is two cycles. Registers named D0..D7 / A0..A7 are the 68000's data and
address registers; they are named only where the driver and the host exchange values.

## 1. File layout

### 1.1 Hunk structure

A standard AmigaDOS load file with one hunk:

- long 0x000003F3, long 0 (no resident names), long 1 (hunk count), long 0, long 0, then the size
  long of hunk 0: its top bits are memory flags (chip memory: the byte at file offset 0x14 is 0x40),
  the rest is the size in longwords;
- long 0x000003E9 (code), long size in longwords, the hunk data;
- long 0x000003EC (32-bit relocations): groups of (count, target hunk = 0, count offsets) ending with
  a zero count; each named long gets the hunk's load address added;
- long 0x000003F2.

### 1.2 Header (hunk offsets)

| offset | size | meaning |
|---|---|---|
| 0x00 | 4 | bytes 70 FF 4E 75 (a stub that returns -1 when the file is run from a shell) |
| 0x04 | 8 | ASCII `DAGLISH!` |
| 0x0C | 4 | pointer: **play routine** |
| 0x10 | 4 | pointer: **audio-interrupt routine** |
| 0x14 | 4 | pointer: **init routine** (subsong number in D0) |
| 0x18 | 4 | number of subsongs (a count, not a pointer) |
| 0x1C | 4 | pointer: song name (zero-terminated) |
| 0x20 | 4 | pointer: author name (zero-terminated) |
| 0x24 | 4 | pointer: special info text (zero-terminated) |
| 0x28 | 16 | four information sizes: load size, calculated size, samples size, song size (no effect on replay) |

The pointers at 0x0C, 0x10, 0x14, 0x1C, 0x20 and 0x24 are relocated longs.

### 1.3 Detection

On the raw file:

1. the first long is 0x000003F3;
2. the byte at file offset 0x14 is non-zero;
3. the long at file offset 0x20 is 0x70FF4E75 and the eight bytes at 0x24 are `DAGLISH!`;
4. the longs at file offsets 0x2C, 0x30, 0x34 and 0x38 (play, audio-interrupt, init, subsong
   count) are all non-zero.

Nothing else is checked. The rule assumes exactly one size long in the hunk header. (The Dave Lowe
modules use the same wrapper with another tag; that is the sense of the player's remark that the
format is "close to" Dave Lowe's. The music data is unrelated.)

### 1.4 Metadata

- Title: the song name; author: the author name; the special info text is shown as information
  (in the known modules a copyright line and the adaptation credit).
- Subsongs 0 to count-1; default 0. The count is the header long, nothing else is checked.
- Voices: 3 (fixed by the player; the fourth Paula voice is not used for music).
- Duration: not stored; the only end is the song-end report of section 7.

### 1.5 The three known modules

| module | file bytes | hunk bytes | play | audio int. | init | subsongs | scanned words W (2.2) | song-end path (7) |
|---|---|---|---|---|---|---|---|---|
| Terramex | 8352 | 0x1E98 | 0x410 | 0x1DA4 | 0xC98 | 1 | 118 | A |
| Chubby Gristle | 18472 | 0x45C8 | 0x460 | 0x1D48 | 0xDAA | 2 | 189 | B |
| The Flintstones | 18940 | 0x4784 | 0x470 | 0x1F04 | 0xDBA | 2 | 189 | B |

Sites found (hunk offsets, section 2.3): Terramex SONGEND 0xCB0, ADR 0x544 / 0x552 / 0x560, WAIT
0x5D4; Chubby Gristle 0xDE4, 0x55E / 0x56C / 0x57A, 0x5E2; The Flintstones 0xDF4, 0x56E / 0x57C /
0x58A, 0x5F2. In all three the SONGEND site lies inside the init routine. Volume bytes (2.2, display
only): Terramex 0x366 / 0x3A4 / 0x3E2, Chubby Gristle 0x3AE / 0x3F0 / 0x432, The Flintstones 0x3BE /
0x400 / 0x442.

## 2. Loading

### 2.1 Placement

The driver uses relocated pointers only, so any placement works, but sample addresses written to
the location registers are absolute. In the reference the module file is loaded at 0x45E0 and the
hunk data lands at `align8(0x45E0 + file length) + 0x408` (0x6A88, 0x9210, 0x93E8 for the three
modules). In this host, allocating hunk size + 8 bytes from the machine's allocator after the module
and placing the data 8 bytes into that block gives exactly these addresses. The project's log
comparison also accepts one constant offset for addresses outside the module file.

### 2.2 The audio-interrupt routine scan

The player walks the audio-interrupt routine word by word from its start (even offsets). With the
three words of `data/scan_words.bin` (big-endian: **end word**, **replacement word**, **volume-load
word**):

- each word equal to the volume-load word is followed by a long: a pointer to a voice's volume
  byte; the first three found belong to voices 0, 1, 2 (display only: the player shows the byte
  times 4 as the voice volume, and the word just before the byte as the voice period);
- the walk stops at the first word equal to the end word, which is replaced by the replacement word
  (the routine then returns like a subroutine; the host runs it as a call, section 5).

`W` = number of words examined, including the end word; `m` = number of volume-load words found
(3 in all known modules; the player has room for three, a fourth would overwrite its own state).

### 2.3 Replaced code shapes (sites)

The player searches the hunk from the play routine's start, at even offsets, over **2600 bytes**,
for five word sequences of the driver's own code (data/site_keys.bin: for each key one byte giving
its word count, then that many big-endian words). Keys are processed in this order; each scans the
whole range from its start; a match must lie wholly inside the range; scanning continues right
after a match; every match is a **site**:

| # | key | words | site kind | what the driver's code there does |
|---|---|---|---|---|
| 1 | key 1 | 3 | SONGEND | writes 0x000F to the DMA control register through its chip base |
| 2 | key 2 | 4 | ADR 0 | writes voice 0's location (D4) and length (D3) |
| 3 | key 3 | 4 | ADR 1 | writes voice 1's location (D5) and length (D2) |
| 4 | key 4 | 4 | ADR 2 | writes voice 2's location (D6) and length (D1) |
| 5 | key 5 | 4 | WAIT | a short busy loop, then a read of the DMA control register into D0 |

Every occurrence is replaced (the known modules have one of each). When execution reaches the first
word of a site, the port performs the site's behaviour and cost (section 6) and continues at the
word after the matched words. The driver's chip base is A5 (0xDFF000 in the known drivers).

### 2.4 Nothing else

No hardware address fix-up, no sample pointer fill, no volume scaling: the driver's register values
reach Paula unchanged (the player has no volume or balance support).

## 3. Time model

As in the repo's other hosted-driver ports:

- Each host call starts a count of cycles at a start clock `S` (its epoch).
- Before an instruction executes, its table cycles (`m68k_rc::cycles::cycles` of the instruction
  decoded with the repo's 68020 decode model, as the host's own guest calls do) are added to the
  count; the clock is then `S + floor(count / 2)`, and a store or read made by that instruction
  happens at that clock. The half cycle stays in the count.
- A site adds its cycles to the same count; its stores and reads happen at the count reached at the
  listed offset.
- **Re-basing:** at each point this spec marks *host point*, the reference calls the host, and the
  count restarts at 0 from the current clock: the odd half cycle, if any, is lost there.
- The CPU is a 68000 core; each call starts with fresh registers (all zero except those named),
  SR 0x2000 and a private stack.

Beam: the line byte the WAIT site reads is the bus's byte at custom offset 0x006 at the read clock
(line = floor(clock / 227) mod 313, low 8 bits).

## 4. Start-up and subsong changes

### 4.1 First start (subsong 0)

1. **Init call** at `I = 656 + 21 * W + 12 * m` (reference clock; W and m from 2.2): Terramex 3170,
   Chubby Gristle and The Flintstones 4661. The port's start runs before that clock in this host;
   it moves the clock to `I`.
2. The **init path** (4.3) with subsong 0, starting at count 0.
3. Its end `E` (Terramex 6521, the other two 8231). The bus clock is left at `E`; the first tick is
   at `E + 70940` and tick `k` at `E + 70940 * (k + 1)` (the host's default timer; the player sets no
   tempo).

### 4.2 Starting at another subsong

The reference's offline render with start subsong `N` other than 0 first does 4.1 with subsong 0,
then, at `E`, before any tick:

1. the **stop routine** (4.4) called directly, from count 0 at `E`; its end `E1 = E + 169` in the
   known cases (Chubby Gristle 8400);
2. the init path (4.3) with subsong `N`, from count 0 at `E1`; its end `E'` (Chubby Gristle
   subsong 1: 11970);
3. first tick at `E' + 70940`.

A subsong change during playback (next / previous song) is done by the host the same way (stop
routine, then init path), but was not logged; the tick schedule after it is unknown.

### 4.3 The init path

From the current count:

1. 1160 cycles (the player clears its display state), then the player marks "init running";
2. the init routine runs with D0 = subsong number and A0 = the init routine's address (other
   registers 0); sites inside it are counted as usual;
3. after it returns: "init running" is cleared; the value of **A4** at the return is kept as the
   **work-area pointer** `WA`; then 120 cycles, *host point*, 66 cycles, *host point*, 66 cycles,
   *host point*, 66 cycles, *host point*, 88 cycles (the player installs the audio-interrupt routine
   on the four audio interrupts: from here on, section 5 applies).

The known init routines switch the three voices' DMA off, clear their periods and set their lengths
to 4 words, clear their own work area, clear the audio control register's modulation bits (0x00FF
to ADKCON) and set the song-end flag byte (7) to 1.

### 4.4 The stop routine

From the current count; `P` is 72 when called directly (4.2), 90 when called from a tick (7.2):

`P` cycles, *host point*, 54, *host point*, 54, *host point*, 46, *host point* (the audio-interrupt
routine is removed: section 5 no longer applies), 44 cycles, write 0x000F to DMACON, 16 cycles,
write 0 to AUD0VOL, 12 cycles, 0 to AUD1VOL, 12, 0 to AUD2VOL, 12, 0 to AUD3VOL, 16 cycles to the
end.

(Chubby Gristle subsong 1: DMACON at E + 135, the volumes at E + 143, 149, 155, 161, end E + 169.)

## 5. Audio interrupts

The driver's audio-interrupt routine is installed for all four audio interrupt levels. Whenever the
host delivers the audio interrupt of channel `c` (the engine's per-channel call, in channel order)
and Paula's request bit for `c` is still set, the port runs the routine (with the end word replaced,
2.2) as a host call: count 0 at the current clock, A0 = 0xDFF000, other registers 0. If the request
bit was already cleared (an earlier run in the same round acknowledged it), nothing runs. While the
routine is not installed (before the first init path ends, inside the stop routine) nothing runs.
The bus clock is left at the end of the routine.

The known routines read the interrupt request register themselves, and for each of voices 0, 1, 2
whose bit is set write that voice's volume (from the driver's state) and acknowledge it (disable
and clear the request). Their branch for channel 3 is never enabled in the logs.

## 6. One tick and the sites

### 6.1 Tick

At the tick clock, a call starts at count 0:

1. 256 cycles, then the play routine runs with A0 = its own address (other registers 0);
2. after it returns, 195 cycles; then the byte at `WA + 0x11` (the **song-end flag byte**) is read:
   - non-zero: 168 cycles, end of the tick;
   - zero: the song-end path of 7.2, then 168 cycles, end of the tick.

The bus clock is left at the end of the tick; interrupts raised during the tick are delivered after
it (by the engine), at that clock.

### 6.2 Site behaviour and cost

Offsets are cycles from the moment the site is reached; after `total` cycles execution continues
after the site's words.

| kind | behaviour | stores / reads at | total |
|---|---|---|---|
| SONGEND, init running | DMACON (A5 + 0x96) = 0x000F | store at 36 | 77 |
| SONGEND, otherwise | DMACON = 0x000F; then the song end is reported (7.1) | store at 36; report at 151, *host point* there | 151, then 76 after the host point |
| ADR v | location register of voice v (A5 + 0xA0 + 16v) = the long in D4 / D5 / D6 (one long store), then its length register (A5 + 0xA4 + 16v) = low word of D3 / D2 / D1 | 36, 48 | 128 |
| WAIT | line wait, then the low word of D0 = the word read from A5 + 2 (DMA control read-back); the upper word of D0 and all other registers unchanged | see below | see below |

WAIT in detail: add 234 cycles. Then 9 times: add 16 and read the line byte as the reference;
repeat { add 16, read the line byte, add 9 } until the byte read differs from the reference; add 10.
Then add 56, read the DMA control word, add 20. Every read uses the clock after the cycles added
before it.

The ADR sites store exactly what the replaced code stored (the player only notes the values for its
display). The location long appears in the write log as two word writes at the same clock.

## 7. Song end

### 7.1 Path A: the driver restarts itself (Terramex)

When the Terramex driver reaches the end of its track list it jumps from the play routine into its
own init routine (returning to the tick from there). Its SONGEND site is then reached while "init
running" is clear: the song end is reported (second SONGEND row of 6.2). The init routine sets the
flag byte to 1 again, the tick ends normally, and replay continues from the start of the song.

### 7.2 Path B: the flag byte is zero (Chubby Gristle, The Flintstones)

These drivers clear the song-end flag byte themselves at the end of the song. In the tick (6.1),
after the 195 cycles:

1. the stop routine (4.4) with `P` = 90;
2. 48 cycles, *host point*: the song end is reported;
3. 34 cycles, then the init path (4.3) with the current subsong (the song restarts);
4. 168 cycles, end of the tick.

### 7.3 Reference song ends

The report clocks: Terramex 687914213 (193.9 s), Chubby Gristle 476796661 (134.4 s) and subsong 1
68186000 (19.2 s), The Flintstones 114151367 (32.2 s) and subsong 1 616552200 (173.8 s). The offline
render stops at the end of the 0.1 s chunk holding the report, so each full log holds the rest of
that tick (path B: the stop routine and the new init) and possibly a few more ticks.

## 8. What the driver does (information)

For orientation only (the port runs the code): the drivers are Amiga versions of Daglish's C64
routine. Each tick the play routine counts down each voice's note duration, steps its pattern and
track lists, and recomputes waveform, pitch and volume state; it switches voices off and on through
DMACON, writes period registers, writes location and length through the ADR shapes only when a
voice's waveform changes, waits, then enables the voices and their audio interrupts. The volume is
written by the audio-interrupt routine when a voice's waveform restarts (and, in Chubby Gristle and
The Flintstones, also by the play routine). Chubby Gristle and The Flintstones also carry samples
(samples size 9968 bytes).

## 9. Validation

A private model, written alongside this text and checked to follow it, loads and relocates the hunk, applies 2.2 and 2.3,
runs the driver on the repo's 68000 core with the cycle table, applies section 6 at the sites,
section 5 on audio interrupts and sections 4 and 7. Compared with the reference logs (LOGS.md),
every write is equal in address, value, order and colour clock, for the 10 s log and the
full-length log (to song end) of each of the five subsongs (up to 138601 writes); the repo's
`tools/replay-cmp.py` reports "no difference" for all of them.
