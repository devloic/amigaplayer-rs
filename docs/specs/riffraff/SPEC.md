# Riff Raff: behaviour specification

*Sanitised for publication: behaviour only.*

Format: "Riff Raff" (music by Riff Raff for PAL Developments / Virgin games, 1989; Wanted Team
adaptation 2001). UADE player file `RiffRaff`, internal player name "Riff Raff" (with the space),
file name prefix `riff.`, slug `riffraff`.

Second pass. This version is written for a port that **runs the 68000 sound driver embedded in each
module on the repo's 68000 core** and reproduces what the Eagleplayer adds around it. All numbers
were checked against the reference 68k player run through the host with DELI_WRITE_LOG on four real
modules and the three synthetic modules of the first pass (LOGS.md). A model built only from this
text reproduces every logged register write (address, value, order and colour clock) of all of
them, up to and including song end.

## 0. Summary

A Riff Raff module is an AmigaDOS executable: one relocatable code hunk holding the game's own
68000 sound driver, its song data and its samples, behind a small header that names the driver's
entry points. The Eagleplayer:

1. loads and relocates the hunk;
2. fixes the driver's hardware addresses, fills empty sample pointers, and replaces a few fixed
   code shapes of the driver with calls into itself (volume scaling, song-end detection, faster
   waits, display bookkeeping);
3. calls the driver's init routine once per subsong start and two driver routines once per tick,
   with a raster-line wait and a DMA-control write of its own between them.

A port does the same: it runs the driver's instructions on the 68000 core, charging each
instruction the cycles of the repo's cycle table (section 3), and treats every replaced code shape
as a host trap with the behaviour and cycle cost of section 6. The sequencing, notes, effects and
samples are then whatever the driver's own code does; this spec does not need to describe them.

All values are big-endian. "Hunk offset" means an offset from the first byte of the code hunk's
data (file offset 0x20 for these one-hunk files). Cycles are 68000 cycles as counted by the cycle
table; colour clocks (cc) are the host's time unit; one colour clock is two cycles (section 3).

## 1. File layout

### 1.1 Hunk structure

A standard AmigaDOS load file with one hunk:

- long 0x000003F3, long 0 (no resident names), long 1 (hunk count), long 0, long 0 (first and last
  hunk), then the size long of hunk 0. Its two top bits are memory flags (bit 30 set: chip memory;
  the byte at file offset 0x14 is therefore 0x40); the rest is the size in longwords.
- long 0x000003E9 (code), long size in longwords, the hunk data.
- long 0x000003EC (32-bit relocations): groups of (count, target hunk = 0, count offsets), ending
  with a zero count. Each named long gets the hunk's load address added.
- long 0x000003F2.

Everything below that is called a pointer is a relocated long.

### 1.2 Header (hunk offsets)

| offset | size | meaning |
|---|---|---|
| 0x00 | 4 | bytes 70 FF 4E 75 (a stub that returns -1 when the file is run from a shell) |
| 0x04 | 8 | ASCII `RIFFRAFF` |
| 0x0C | 4 | pointer: **tick routine B** (called every tick; see 5) |
| 0x10 | 4 | pointer: **init routine** (subsong number in X0) |
| 0x14 | 4 | number of subsongs (count, not a pointer) |
| 0x18 | 4 | pointer: **tick routine A**, or 0 when the module has none (see 5) |
| 0x1C | 4 | pointer: song name (zero-terminated) |
| 0x20 | 4 | pointer: author (zero-terminated) |
| 0x24 | 4 | pointer: special info text (zero-terminated) |
| 0x28 | 4 | pointer: sample address table (longs) |
| 0x2C | 4 | pointer: end of the sample address table = start of the sample length table (words) |
| 0x30 | 16 | four information sizes (load, calculated, samples, song); no effect on replay |

When tick routine A exists it does the driver's main work (sequencing, envelopes, note starts) and
returns a DMA-enable value; routine B then only sets the voices' loop pointers. When A is absent,
routine B does all the work and contains two busy-wait delays of its own.

### 1.3 Detection

A file is a Riff Raff module when, on the raw file:

1. the first long is 0x000003F3;
2. the byte at file offset 0x14 is non-zero;
3. the longs at file offsets 0x20, 0x24, 0x28 are 0x70FF4E75, `RIFF`, `RAFF`;
4. the longs at file offsets 0x2C, 0x30 and 0x34 (routine B, init, subsong count) are non-zero.

Nothing else is checked. The rule assumes exactly one size long in the hunk header.

### 1.4 Metadata

- Title: the song name string. Author: the author string. The special info string is shown as
  information (in the known modules: a copyright line and the adaptation credits).
- Subsongs 0 to count-1; default 0.
- Voices: 4, or 3 when the fourth-voice flag (2.6) is clear.
- Sample count (information only): with `n` = number of longs in the sample address table, it is
  the index of the first entry after entry 0 that equals entry 0; n-1 if none does; n if entry 0 is
  0 (before the fill of 2.3).
- Duration: not stored; the only end is the song-end report of section 7.

### 1.5 The four known modules (hunk offsets)

| module | file bytes | init | routine B | routine A | subsongs | region end (2.4) | DMA-mask word (2.6) |
|---|---|---|---|---|---|---|---|
| Bomb Fusion | 45016 | 0x0D6 | 0x27E | none | 1 | 0x846 | 0xEF8 |
| Protector | 36800 | 0x0A6 | 0x266 | 0x212 | 1 | 0x75E | 0xE2E |
| Side Winder II | 20976 | 0x0EC | 0x2C0 | 0x26A | 1 | 0x7D4 | 0x4FF8 |
| T'Bird | 54044 | 0x0C4 | 0x2AC | 0x25C | 2 | 0x742 | 0xFE0 |

## 2. Loading

### 2.1 Placement

Any placement works (the driver uses only relocated pointers), but location register values are
absolute addresses, so they depend on it. In the reference the module file is loaded at 0x45E0 and
the hunk data lands at `align8(0x45E0 + file length) + 0x408` for the four real modules (0xF9C0,
0xD9A8, 0x9BD8, 0x11D08); for the 676-byte synthetic modules it landed 0x158 higher than that rule.
Treat it as a host property: the project's log comparison accepts addresses outside the module file
that differ by one constant from the reference.

### 2.2 The zero buffer

The Eagleplayer owns a 4-byte chip-memory buffer of zeros. Wherever this spec says "the zero
buffer", its address is meant. In the reference it is at **0x006AC650** for every module; a port
should use exactly that address (it shows up in location registers, and the comparison accepts an
address only if it is identical or differs by the same constant as the hunk).

### 2.3 Sample pointer fill

Let `n` = (table end - table start) / 4 (header 0x28, 0x2C). If `n` > 0 and the first long of the
sample address table is 0, the player:

1. stores the zero-buffer address into that first long;
2. treats the `n` longs that start right after the sample length table (at table end + 2n) as a
   second address table (the driver's loop-start addresses) and replaces each long that is 0 there
   by the zero-buffer address.

When the first long is not 0 nothing is changed. (Bomb Fusion: no fill. Protector, Side Winder II
and T'Bird: filled, with 8, 6 and 10 zero loop addresses.)

### 2.4 Patch region and hardware address fix-up

The **region** starts at the init routine and ends at the first long equal to 0x35803280 found at an
even offset from the init routine (the first two entries of the driver's period table). Its length
in words is `W`.

The driver addresses the custom chips as 0x00DF00xx (the adapter stripped one digit). The player
walks the region word by word from its start: when a word equals 0x00DF, the following word gets
0xF000 ORed in and is skipped; the walk stops at the marker long (checked before each word). Call
the number of fixed words `D`. After this, every hardware reference reads 0x00DFF0xx. This is done
before the site search of 2.5 (two search keys contain fixed addresses).

### 2.5 Replaced code shapes (sites)

The player searches the region for ten fixed word sequences of the driver (search keys: big-endian
words of the module's own code, given in data/site_keys.bin). Keys are processed in this order; each key scans
the region from its start at even offsets, a match must lie wholly before the marker, scanning
continues right after a match, and words of an earlier match cannot be part of a later one. Every
match is a **site**; its kind gives its behaviour and cost (section 6).

| # | key (words, from data/site_keys.bin) | site kind |
|---|---|---|
| 1 | key 1 (4 words) | READBACK |
| 2 | key 2 (4 words) | WAIT |
| 3 | key 3 (4 words) | WAIT |
| 4 | key 4 (3 words) | VOLUME |
| 5 | key 5 (9 words) | TRIPLE |
| 6 | key 6 (8 words) | MINVOLUME |
| 7 | key 7 (3 words) | RESTART |
| 8 | key 8 (12 words) | CLEARALL |
| 9 | key 9 (5 words) | CLEARVOICE |
| 10 | key 10 (4 words) | CLEARRECORD |

`data/site_keys.bin` holds the ten keys in this order: for each key one byte giving its word count, then that many big-endian 16-bit words. The words are taken from the driver's own code; the lead decides on embedding.

When execution reaches the first word of a site, the port performs the site's behaviour, charges
its cost and continues at the word after the matched words. Sites found in the known modules:

- Bomb Fusion: WAIT (two, keys 2 and 3), VOLUME, TRIPLE, RESTART, READBACK.
- Protector, Side Winder II: CLEARRECORD, CLEARALL, VOLUME, TRIPLE, RESTART, READBACK, CLEARVOICE.
- T'Bird: CLEARRECORD, CLEARALL, MINVOLUME, TRIPLE, RESTART, CLEARVOICE.

### 2.6 DMA-mask word and the fourth-voice flag

Search routine B from its start, at even offsets, for the first word 0x33F9; the long after it is a
pointer to the driver's **DMA-mask word** (the voices this song uses). After every init call the
player reads that word: if bit 3 is clear, the song is a 3-voice song (fourth-voice flag clear),
otherwise 4-voice. T'Bird subsong 1 is a 3-voice song; all other known songs are 4-voice.

### 2.7 Load cost (time before the first host write)

The player's load step costs `C_load` cycles, which sets when the first host write happens (4.1):

`C_load = 932 + F + 21 * K + T`

- `F = 50 * (W - D) + 26 * D + 29` (the fix-up walk of 2.4);
- `K` = number of words examined by the search of 2.6, including the 0x33F9 word;
- `T` (2.3), with `n` the sample table entry count:
  - `n` = 0: `T = -37`;
  - first entry non-zero: `T = 16 + 46 * j + 10`, where `j` (1..n-1) is the index of the first
    entry equal to entry 0; if there is none, `T = 16 + 46 * (n - 1) + 15`;
  - first entry zero: `T = 68 + 44 * n + 12 * z`, where `z` is the number of zero longs replaced in
    the second table.

Known modules: Bomb Fusion W 952, D 24, K 14, T 394, C_load 48673; Protector W 860, D 22, K 8,
T 516, C_load 44117; Side Winder II W 884, D 22, K 8, T 404, C_load 45205; T'Bird W 831, D 20,
K 8, T 672, C_load 42871.

## 3. Time model

The reference runs the player and the driver on a 68000 whose time is the repo's cycle table
(`m68k_rc::cycles::cycles` of the decoded instruction; the 68000 and 68020 decode models give the
same result for these modules). As in the repo's other hosted-driver ports:

- Each host call starts a count of cycles at a start clock `S` (the call's epoch).
- Before an instruction executes, its table cycles are added to the count; the clock is then
  `S + floor(count / 2)`. A store made by that instruction happens at that clock. Some table
  entries are odd, so the half cycle must be carried in the count, not rounded per instruction.
- A site (6) adds its cycles to the same count; its stores happen at the count reached at the
  listed offset.
- Re-basing: when the song end is reported (7), the reference makes a host call and continues with
  a new epoch at the current clock; the odd half cycle, if any, is lost at that point.

Beam: line = floor(clock / 227) mod 313, counted from clock 0; one frame is 71051 cc. The waits
below read the low 8 bits of the line number.

## 4. Start-up and subsong changes

### 4.1 First start (subsong `s`)

1. **Host volume writes.** `V0 = 476 + floor(C_load / 2)` (reference clock). The host refreshes the
   volume twice: writes to the volume registers of voices 0, 1, 2, 3, 0, 1, 2, 3 at `V0` + 0, 132,
   276, 415, 664, 796, 940, 1079 cc. Each value is the voice's scaled shadow volume (6.3), which is
   0 at this point.
2. **Init call** at `I = V0 + 1108`: 1140 cycles before the first instruction of the init routine
   (the player clears its display state), the init routine with X0 = `s` (other registers are not
   used by the known drivers), sites inside it counted as usual, then after its return 127 more
   cycles, or 139 when the fourth-voice flag is clear (2.6). Call the clock at the end `E`.
3. **First tick** at `T0 = E + 70940`; tick `k` starts at `T0 + 70940 * k` (PAL vertical blank,
   constant).

Reference values: Bomb Fusion V0 24812, I 25920, T0 98551; Protector 22534, 23642, 96330;
Side Winder II 23078, 24186, 96926; T'Bird 21911, 23019, 95740.

The shadow raw volumes (6.3) start at 0 when the module is loaded and are not cleared by an init.

### 4.2 Starting at another subsong

The reference's offline render with a start subsong `N` other than 0 first does 4.1 with subsong 0
and then switches, before any tick:

1. At `E` (end of the subsong-0 init) the player's stop routine runs: DMACON = 0x000F at `E + 14`,
   then volume 0 written directly to voices 0, 1, 2, 3 at `E + 22`, `+30`, `+38`, `+46`; it ends at
   `E + 54`. These writes bypass the shadow volumes.
2. The host's two volume rounds as in 4.1 step 1, with `V0' = E + 274`.
3. Init call with X0 = `N` at `V0' + 1108`, as in 4.1 step 2; its end `E'`.
4. First tick at `E' + 70940`.

(T'Bird subsong 1: V0' 25074, init at 26182, first tick 98936.) Changing the subsong during
playback was not logged; the same stop / volume / init order is expected, but the tick schedule
after it is unknown.

## 5. One tick

Every tick starts a call at the tick clock (count 0).

Without routine A (header 0x18 = 0):

1. 281 cycles, then routine B runs;
2. after B returns, 168 cycles.

With routine A:

1. 269 cycles, then routine A runs; it leaves a DMA value in the low word of X2;
2. a line wait (6.2) with P = 46, Q = 44;
3. 16 cycles, then the low word of X2 is written to DMACON (also when it is 0);
4. a line wait with P = 46, Q = 44;
5. 32 cycles, then routine B runs;
6. after B returns, 168 cycles.

All other register writes of a tick come from the driver (its own instructions and its sites), in
its own order. Routines A and B do not depend on register contents at entry.

## 6. Sites

### 6.1 Voice of a site

At every site the driver holds the current voice record's address in the pointer VREC and the current voice's
custom-chip base (0xDFF000 + 16 * voice) in VCHIP. The voice used for the costs below is 0, 1 or 2 when
VCHIP equals 0xDFF000, 0xDFF010 or 0xDFF020, and 3 otherwise. Stores go to the addresses the driver
names (VCHIP-relative registers keep their own voice).

### 6.2 Line wait

`wait(P, Q)`: add P cycles. Then 9 times: add 16 cycles and take the line (low 8 bits) as the
reference; repeat { add 16 cycles, read the line, add 9 cycles } until the line read differs from
the reference; add 10 cycles. Finally add Q cycles. Every read uses the clock after the cycles added
before it. Registers are unchanged.

### 6.3 Volume path

The player keeps a raw shadow volume per voice. A scaled volume write for voice `v` and value `x`:

- raw = x AND 0x7F; store it as voice `v`'s shadow;
- write `raw * scale_v >> 6` to voice `v`'s volume register, where `scale_v` is 0 for a muted voice,
  else `(master * balance) >> 6` with the left balance for voices 0 and 3 and the right balance
  for voices 1 and 2. With the host defaults (master 64, balances 64) the value written is raw.

### 6.4 Behaviour and cost of each kind

Offsets and totals are in cycles from the moment the site is reached; columns v0..v3 are by the
voice of 6.1. After `total` cycles execution continues after the site's words.

| kind | behaviour | stores at | total v0 / v1 / v2 / v3 |
|---|---|---|---|
| READBACK | low word of X0 becomes 0 (no read of the audio control register) | none | 44 |
| WAIT | `wait(66, 64)` (replaces the driver's busy loop; its counter register is left unchanged) | none | (the wait) |
| VOLUME | scaled volume write of the word at VREC | 232 / 255 / 278 / 268 | 373 / 427 / 481 / 479 |
| MINVOLUME | scaled volume write of the unsigned smaller of the low words of X1 and X2 (X1 on equality); if X2 is smaller, add 10 to both columns | 241 / 264 / 287 / 277 | 402 / 456 / 510 / 508 |
| TRIPLE | location (long at VREC+2, both words at once), length (word at VREC+6), period (word at VREC+8) to VCHIP+0xA0, +0xA4, +0xA6 | 48, 68, 88 | 281 / 312 / 343 / 351 |
| RESTART | the long at VREC+0x2A is copied to VREC+0x1C; song-end step of section 7 | none | 190 / 217 / 240 / 253; +88 when it reports song end |
| CLEARALL | location registers of voices 0, 1, 2, 3 get the zero-buffer address | 48, 76, 104, 132 | 184 |
| CLEARVOICE | VCHIP+0xA0 location = zero-buffer address, VCHIP+0xA4 length = 1 | 44, 60 | 84 |
| CLEARRECORD | word at VREC+0x32 = 0, long at VREC+2 = zero-buffer address | none | 80 |

Notes:

- MINVOLUME replaces a shape that ends with an early return on one branch; after the site, both
  branches continue after the 8 words (in T'Bird that is a return as well).
- The replaced shapes of CLEARALL, CLEARVOICE and CLEARRECORD stored 0; the player stores the zero
  buffer instead, so silent voices point at it.
- No register other than those named changes (X0 in READBACK).

## 7. Song end

Each voice has a "not yet restarted" flag. At every init they are set for voices 0, 1, 2 and, when
the fourth-voice flag is set, voice 3; an untracked voice 3 has its flag clear.

At a RESTART site of voice `v` (VCHIP equals that voice's base exactly), the flag of `v` is cleared.
If then no flag is set, the song end is reported: the flags are set again as at init, the site
costs 88 more cycles, the report happens 60 cycles before the site's end, and the clock is re-based
there (section 3). Replay continues with the driver's own loop (it does not stop or restart
anything). The reference's offline render stops at the report (its log may hold the rest of that
tick, or one more tick).

Reference song ends: Bomb Fusion clock 599403737 (169.0 s), Protector 408715115 (115.3 s),
Side Winder II 299964697 (84.6 s), T'Bird 572159967 (161.4 s), T'Bird subsong 1 667505389 (188.2 s).

## 8. Host volume, balance and muting

When master volume or balance changes, the player recomputes the scales and rewrites the four volume
registers from the shadow raw volumes, voices 0 to 3, with the per-voice spacing of 4.1 step 1
(0, 132, 276, 415 cc). Muting a voice through the host's voice mask writes 0 to its volume register
at once and makes its scale 0; DMA-control writes are not filtered. (Not exercised by any log.)

## 9. What the driver does (information)

For orientation only (the port runs the code): the driver keeps one record per voice (volume word
at +0, sample start at +2, length at +6, period at +8, sequence position at +0x1C, loop position
at +0x2A; 0x36 or 0x3C bytes per record), steps each used voice's sequence when its note counter
runs out, applies a volume slide and a period slide per tick and a global fade, starts notes by
switching the voice's DMA off, setting location, length and period, and switching DMA on after a
delay, then points the voice at its loop. Its period table starts at the region-end marker.

## 10. Validation

A private model was built from this text alone: it loads the module, applies 2.3 to 2.5, runs the
driver on the repo's 68000 core with the cycle table, applies section 6 at the sites, and follows
sections 4, 5 and 7. Compared with the reference logs (LOGS.md), every write of all logs is equal in
address, value, order and colour clock: the four 10 s logs, the four 60 s logs, the four
full-length logs to song end (up to 214723 writes), T'Bird subsong 1 for 60 s and to its song end,
and the three synthetic logs of the first pass (with their hunk address taken from their log).
