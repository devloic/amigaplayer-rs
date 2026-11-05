# YM-2149 / MYST (`ym.*`, `ymst.*`) player: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the UADE/Eagleplayer player "YM-2149" (its own name: "The YM2149 Player v1.52c"), which plays
files in the MYST format by Nicolas Pomarede (Mr.Styckx), 1997-98. UADE lists the prefixes `ym` and
`ymst`. Both are the same format, read the same way. The MYST distribution names its files `YM.<name>`.
"ST variant" here only means the Atari ST origin of the music: there is one player and one format.

A MYST file carries an Atari ST replay routine (68000 code, patched by the converter) with its music data,
or only the music data plus the name of a separate replay file. The replay does not touch the ST sound chip.
Every tick it writes the state of the YM-2149 registers into a block owned by the player. The player's
built-in "YM-2149 Emulator v0.98b" turns that block into Paula register writes and into sample buffers it
generates itself (square waves, noise, envelopes and their mixes). So an open replayer needs two parts:

1. a 68000 that runs the module's code at the entry points named by the header (section 3). The corpus
   holds more than 30 different replays, and many are self-modifying, so they cannot be reimplemented
   one by one;
2. a reimplementation of the emulator (sections 4 to 6), which reads only the block.

This text describes behaviour only. It was derived from write logs, DMA logs, memory dumps and execution
traces of the 68k reference. Timing is given in 68000 cycles as the reference machine charges them (static
costs per instruction, so a taken and an untaken branch cost the same), and in colour clocks (cc, 2
cycles = 1 cc).

Status (all checked by a private model written from this text, see README):

| check | result |
|---|---|
| 11 synthetic modules (every emulator mode, the edge cases below), 200 ticks each, 36 583 writes | value, order and absolute colour clock of every write equal the reference |
| the same modules, buffer memory (6 mix buffers, scratch, noise, tables) after each of ticks 1..119 of the 5 mixing modules | byte-identical |
| 830 real MYST modules, 120 ticks each, module code run by a separate 68000 emulator | writes and order identical in 793; see README for the other 37 |
| 35 of those (type 0), module code cost taken from the reference's cycle table | absolute timing identical in 33 |

## 1. File format

### 1.1 Header

A file contains the 4-byte magic `YMST` at an even offset below 256. In every corpus file it is at offset
0. A list of tags follows the magic directly: 8-byte entries `u32 tag, u32 value`, big-endian, ending with
a tag of 0. All tags are `0x8000_0000 | 'Y' << 8 | c`, written below as `Yc` (for example `YM` =
0x8000594D). Values marked "offset" are byte offsets from the magic (not from the file start). Unknown tags
are ignored.

| tag | kind | meaning |
|---|---|---|
| `YM` | value | file type: 0 = replay and data in this file, 1 = replay only (cannot be played alone), 2 = data only (needs the replay named by `YO`) |
| `YN` | value | unused by the player (0x10000 or 0x10001 in the corpus) |
| `YO` | offset | type 2: NUL-terminated file name of the replay, in the module's directory |
| `YP` | offset | a longword slot. The player stores the address of its emulator block there (section 4) |
| `YQ` | value | unused by the player |
| `YR` | offset | play-time table, one longword per subsong: high word minutes, low word seconds |
| `YS` | offset | song name (NUL-terminated) |
| `YT` | offset | author text |
| `YU` | offset | converter text |
| `YV` | offset | unused by the player |
| `YW` | offset | free information text |
| `YX` | offset | routine called once at load (relocation) |
| `YY` | offset | routine called once at load, after `YX` (patches) |
| `YZ` | offset | routine called at every subsong start (init) |
| `Y[` | offset | routine called every tick before `Y\` (optional) |
| `Y\` | offset | routine called every tick (play) |
| `Y]` | value | first subsong (default 1) |
| `Y^` | value | last subsong (default 1) |
| `Y_` | value | default subsong (default 1; not used by the Eagleplayer path) |
| `` Y` `` | offset | subsong map: longword `n-1` is the value given to `YZ` for subsong `n` |
| `Ye` | offset | unused by the player |
| `Yc` | offset | replay files: slot that receives the address of the data |
| `Yd` | offset | replay files: slot that receives the length of the data |
| `Ya`, `Yb` | | present in all files (start and size of the original replay/data); unused by the player |

### 1.2 Detection

A file is accepted when all of these hold:

- the magic is found (even offset below `min(256, file size)`);
- the tag list contains `YM` with value 0 or 2. Value 1 (replay files) and a missing `YM` are rejected;
- type 2: the list contains `YO`, the named file can be loaded and starts with `YMST` at offset 0, and its
  tags contain `YZ`, `Y\`, `YP`, `Yc` and `Yd`. The data file must contain `YZ`. Its `YP` slot is written
  without a check.
- type 0: `YZ`, `Y\` and `YP` are present.

The player reads nothing else to detect a file. Prefix collisions: no other UADE player uses `ym` or
`ymst`. Atari "YM" register-dump files (`YM2!`..`YM6!`, usually LHA-packed, file suffix `.ym`) are a
different format. They have no `YMST` magic and are rejected.

### 1.3 Subsongs, title, length

- Subsong range: `Y]` .. `Y^`. The host starts at `Y]`.
- Name: the `YS` string. Author line: the `YT` string with its terminating NUL replaced by `.`, then, if
  `YU` exists, ` Converted by ` and the `YU` string (at most 255 characters in all). Information text: `YW`.
  Subsong count: `Y^ - Y] + 1`. For type 2 all of these come from the data file.
- Play time: when `YR` exists, entry number `subsong` (the table is indexed by the subsong number itself, so
  entry 0 is not used for subsong 1) gives `minutes * 60 + seconds`.
- Song end: the player never signals one. The music loops in the replay. The play time is the only length
  information.

## 2. The module's code and how it is called

Module code runs as ordinary 68000 code in chip memory (writable, often self-modifying). It reaches the
emulator only through the block address the player stores into its `YP` slot, and through the service
table whose address is in the block (4.4). In the corpus, some replays still write the ST sound-chip
ports (0xFF8800/0xFF8802) or read 0xFF8800. On the Amiga those accesses go to memory that does not affect
Paula.

Registers at each call, as the reference passes them. Unlisted registers keep whatever the previous call
left. Several replays depend on this (the relocation routine of a replay file uses a0/a1 as data address
and length):

- load, type 0: a0 = block address, a1 = address of the `YP` slot; `YX` then `YY` (each called only if
  present; a6 = the routine's own address).
- load, type 2 (data file D, replay file R; both `YP` slots receive the block address): a0 = block, a1 =
  D's `YP` slot. Call D's `YX` if present. Store a0 into R's `Yc` slot and `a1 - a0` into R's `Yd` slot,
  and set a1 = `a1 - a0`. Call R's `YX`. Then the same three steps with D's `YY` and R's `YY`.
- subsong start: d0 = subsong number, or longword `n-1` of `` Y` `` when present; d1 = address of the
  `` Y` `` table or 0. Type 0: call `YZ`. Type 2: call D's `YZ`, store its returned a0 / `a1 - a0` into R's
  `Yc` / `Yd` (a1 = the length), then call R's `YZ` with d0 as D's routine left it.
- every tick: `Y[` (if present; a0 = d0 = its address), then `Y\` (a0 = its address) of the file holding
  the replay (R for type 2).

Before every subsong start after the first, the host stops the sound (4.6). The emulator is initialised
only once, when the player loads the module (4.2). Its state carries over between subsongs.

## 3. The tick

The host calls the player's tick routine at 50 Hz (the reference: every 70 940 cc). In order:

1. Gains: the volume and balance settings are stored into the four voice records (4.3). Left gain
   `= left balance * volume >> 6` goes to the voices on channels 0 and 3, right gain to channels 1 and 2
   (64 at the default settings). The voice mask (the host's channel enable bits, 0xF by default) is
   stored into the block.
2. The module's `Y[` and `Y\` run (section 2). They store the YM register state into the block.
3. The emulator tick (section 5): first the Paula writes for the state computed at the end of the
   previous tick (the commit), then the new computation from the block. All Paula writes of a tick
   therefore happen in its commit phase, and what the module wrote in tick `n` is heard from tick `n+1`.

## 4. The emulator

### 4.1 Memory layout

The emulator's buffers and tables live in one area. Paula addresses point into it, and a comparison
with the reference works when the relative layout is kept. Offsets are relative to the silent word `S`:

| offset from S | size | content |
|---|---|---|
| -0x2218 | 16 | volume table `VOL[16] = 0, 2, 3, 4, 6, 8, 11, 14, 17, 22, 26, 30, 35, 42, 50, 64` |
| -0x2208 | 16 | linear table `LIN[16] = 0, 5, 10, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64` (only service 11) |
| -0x21F8 | 4096 | scaled-sample table `SC[16][256]` (4.2) |
| -0x1158 | 512 | envelope level tables: 8 pairs, 64 bytes apart, 32 words used each (4.2) |
| -0xF58 | 0x158 | the emulator block (4.3) |
| -0xE00 | 6 x 512 | mix buffers M0..M5: voice A uses M0/M1, B M2/M3, C M4/M5 |
| -0x200 | 512 | square scratch buffer |
| 0 | 2 | silent sample (two zero bytes) |
| +2 | 258 | square table: bytes 0x7F, 0x80, then 128 x 0x7F, then 128 x 0x80 |
| +0x104 | 256 | envelope waveforms: 8 x 32 bytes |
| +0x204 | 4096 | noise N |
| +0x1204 | 4096 | noise-high NH |
| +0x2204 | 4096 | noise-low NL |

Square sample of octave `k` (0..7): address `S + 2 + q[k]` with `q = 0, 0x80, 0x7E, 0x7A, 0x72, 0x62,
0x42, 0x02`, length `2^k` words. It is one cycle of `2^k` bytes of 0x7F followed by `2^k` bytes of 0x80.

### 4.2 Initialisation (once, when the player loads the module)

- Block: all 0x158 bytes cleared, then: `+0x10` = 0xFF (no envelope trigger), `+0x0E` = 0x3F (all mixer
  bits off), `+0x14` = service table address, `+0x1A` = 0x000F, `+0x1C` = 1, the pointer fields of 4.3,
  `+0x128` = 0x0800, the four voice records as in 4.3 with flags 0x8000, both register sets = (silent,
  LEN 1, PER 0x8C, VOL 0), gain 64. The envelope pointers of voices A, B, C point at pair 0.
- `SC[v][i]`, for level `v` and byte `i` read as signed `s`: `(((s + 128) * VOL[v]) >> 6) - ((128 * VOL[v])
  >> 6)`, stored as a byte.
- Noise: a Park-Miller generator (`x = 16807 * x mod (2^31 - 1)`, computed with Schrage's method: q =
  127 773, r = 2 836, a non-positive result gets 2^31 - 1 added), seeded with 1. The first value is
  discarded. Then for `j = 0..2047` the next value decides `N[j] = N[j + 2048] =` 0x81 if it is odd, else
  0x7F. `NH[j] = min(N[j] + 127, 127)`, `NL[j] = max(N[j] - 127, -127)` (signed bytes: NH is 127 or 0, NL
  is 0 or -127).
- Envelopes: 16 halves `h = 0..15` of 16 levels each, of type `T[h] = 0, 2, 1, 2, 0, 0, 0, 1, 0, 3, 1, 1,
  1, 3, 1, 0` (0: 15 down to 0, 1: 0 up to 15, 2: all 0, 3: all 15). Pair `p` = halves `2p, 2p+1`. Level
  table of pair `p` (at `-0x1158 + 64p`): 32 words `level << 8`. Waveform of pair `p` (at `+0x104 + 32p`):
  32 bytes `(SC[level][0x7F] - 0x40) * 2`. YM shape `0..15` uses pair `P[shape] = 0, 0, 0, 0, 1, 1, 1, 1,
  2, 0, 3, 4, 5, 6, 7, 1` (pair 0 `\_`, 1 `/_`, 2 `\\`, 3 `\/`, 4 `\` then high, 5 `//`, 6 `/` then high,
  7 `/\`). One-shot shapes therefore repeat every two envelope periods.
- Paula writes, 10 cc apart: AUD0..3 LC = S (both words with one stamp), AUD0..3 LEN = 1, AUD0..3 PER =
  0x80, DMACON = 0x000F, and 18 cc later DMACON = `0x8200 | voice mask` (0x820F). In the reference the
  first of these writes comes 1 197 211 cc after the load call. Almost all of that time builds the tables.
  The envelope tables are built only the first time (a second module loaded into the same player instance
  skips them).

### 4.3 The emulator block (the module-visible interface)

Words are big-endian. The module writes the YM fields. Some replays also write voice-record fields.

| off | size | meaning |
|---|---|---|
| 0x00, 0x04, 0x08 | word | tone period of YM voice A, B, C (12 bits used) |
| 0x02, 0x06, 0x0A | word | volume of A, B, C: bits 0-3 level, bit 4 = envelope |
| 0x0C | word | noise period (5 bits) |
| 0x0E | word | mixer (YM register 7; bits 0-2 tone A-C off, bits 3-5 noise A-C off) |
| 0x10 | byte | envelope trigger: the module writes a value below 0x80 (0) to restart the envelope; the emulator sets 0xFF after using it |
| 0x11 | byte | envelope shape (4 bits) |
| 0x12 | word | envelope period (16 bits, treated as signed in comparisons) |
| 0x14 | long | address of the service table (4.4) |
| 0x18 | word | 0 (a debug flag of the stand-alone version) |
| 0x1A | word | voice mask (rewritten by the host every tick) |
| 0x1C | word | source of the fourth voice: 0 = A, 1 = B, 2 or more = C, negative = none |
| 0x1E, 0x52, 0x86, 0xBA | 0x34 each | voice records of A (Paula channel 0), B (channel 3), C (channel 1) and the fourth voice (channel 2) |
| 0xEE, 0xF2, 0xF6, 0xFA | long | addresses of the records of channels 0, 1, 2, 3 (A, C, fourth, B) |
| 0xFE | long | address of VOL |
| 0x102..0x116 | 6 longs | addresses of M0..M5 |
| 0x11A | word | double-buffer selector, 0 or 4 |
| 0x11C, 0x120, 0x124 | long | addresses of N, NH, NL |
| 0x128 | word | noise length 0x800 |
| 0x12A | long | address of S |
| 0x12E | 8 longs | addresses of the 8 square samples |
| 0x14E | word | tone threshold (signed; 0) |
| 0x150..0x156 | 4 words | commit counters of the four records |

Voice record (offsets inside the record):

| off | size | meaning |
|---|---|---|
| 0x00 | word | flags: bit 15 = restart DMA at the next commit, bit 14 = only PER and VOL changed, bit 7 = voice driven by the module, low bits = mode code (5.3) |
| 0x02 | word | "SID" mode (written by the module): 0 = off, 1 = two-level timer envelope, other non-zero = see 5.3 |
| 0x06 | word | SID period |
| 0x08 | long | SID levels: two words, each `level << 8` |
| 0x0C | long | noise phase (16.16) |
| 0x10 | long | envelope / SID phase |
| 0x14 | long | address of the envelope level table of the current pair |
| 0x18 | long | Paula register base of the channel (0xDFF0A0, 0xDFF0D0, 0xDFF0B0, 0xDFF0C0 for the four records) |
| 0x1C | word | DMA bit (1, 8, 2, 4) |
| 0x1E | word | gain (0..64) |
| 0x20..0x29 | | set A: LC (long), LEN, PER, VOL |
| 0x2A..0x33 | | set B: LC (long), LEN, PER, VOL |

### 4.4 Services

The table at `+0x14` has 14 entries 4 bytes apart. The corpus calls only these three (each preserves
all registers):

- entry 10 (offset 0x28), a0 = start, a1 = end: every byte `b` in `[a0, a1)` becomes `((VOL[v] * 255) >> 6)
  - 0x80` with `v` = 0 if `b >= 0x80`, else `b & 15` (ST 4-bit samples to signed 8-bit);
- entry 11 (offset 0x2C): the same with `LIN`;
- entry 12 (offset 0x30): every byte XOR 0x80.

The other entries belong to the stand-alone emulator (its own audio interrupt, display, start/stop) and are
not used through this player.

### 4.5 Stop

- End of a subsong (the host's end-sound call): DMACON = 0x000F, then AUD0..3 VOL = 0, 10 cc apart.
- Player unload: DMACON = 0x000F.

## 5. The emulator tick

### 5.1 Fourth voice

If `+0x1C` is not negative, the flags word and both register sets (0x20..0x33) of the source record (A,
B or C) are copied into the fourth record, both at the start of the emulator tick and again at its end.
With the default `+0x1C = 1`, channel 2 mirrors voice B (channel 3).

### 5.2 Commit (records in the order A, B, C, fourth)

`R` = set of DMA bits restarted so far in this tick (starts empty). For each record, with counter `c`:

1. flags bit 15 set: clear it, add the record's DMA bit to `R`, `c = 2`, write DMACON = `R` (all bits
   restarted so far, without the set bit), then continue at 3.
2. else flags bit 14 set: clear it, write PER = set-A PER, VOL = set-A VOL x gain >> 6. Done with this
   record.
3. flags bit 7 clear: write LC (set A, both words with one stamp), LEN, PER, VOL = VOL x gain >> 6.
   Flags bit 7 set: if `c = 0`, nothing. Else `c = c - 1`. If it is still non-zero, write everything as for
   bit 7 clear. If it became 0, write LC and LEN of set B only (the loop part of a module-driven sample).

After the four records: if `R & mask` is not empty, wait for 11 changes of the beam's line counter (5.5).
Then write DMACON = `0x8200 | mask` and DMACON = `(0x8200 | mask) XOR 0x820F` (the second is 0x0000 with
the default mask). Both writes happen in every tick.

So with no change, a record writes nothing (PER/VOL-only flag cleared and nothing set again) or rewrites
all four registers every tick (modes whose flag word stays the same, 5.4).

### 5.3 Computation

First the block's fields are masked in place: periods `& 0x0FFF`, volumes and noise `& 0x1F`, mixer `&
0x3F`, shape byte `& 0x0F`. Then the double-buffer selector toggles (0 <-> 4). If the trigger byte is
below 0x80, the phase (+0x10) of records A, B, C is set to 0, their level-table pointer to pair `P[shape]`,
and the trigger byte to 0xFF.

Then for voice A, B, C with its period `p` and volume `v`, the mixer bits of that voice (tone on = bit
clear, noise on = bit clear) and its record:

- flags bit 7 set: skip the voice (the module drives it).
- mode index = `tone_on + 2 * noise_on + 4 * (envelope bit of v or SID mode non-zero)`.
- `buf` = the voice's mix buffer chosen by the new selector (selector 4 -> M1/M3/M5, 0 -> M0/M2/M4).

Helper values: `octave(p)` is the `k` (7..0) of the first of `0x500, 0x280, 0x140, 0xA0, 0x50, 0x28, 0x14,
0x0A` (in that order, `k = 7` for 0x500) that `p` reaches or exceeds. `amiper(p, k) = ((p * 0xE517) >> k)
>> 12`, 16 bits. Clamps below are signed 16-bit comparisons. "Set" means set A and set B both get the
value. The mode code returned becomes the flags (5.4).

| index | mode | behaviour | code |
|---|---|---|---|
| 0 | silent | if set-A LC is already S: code 0x4000. Else set LC = S, LEN 1, PER 0x80, VOL 0, code 0 | 0 / 0x4000 |
| 1 | tone | if `p <= threshold` (+0x14E): as mode 0. `p = max(p, 10)`, `k = octave(p)`. If set-A LC is not square `k`: set LC = square `k`, LEN `2^k`, code 1; else code 0x4001. Set PER `amiper(p, k)`, VOL `VOL[v & 15]` | 1 / 0x4001 |
| 2 | noise | PER `= noise * 16 + 0x8C`. If set-A LC is not N: set LC = N, LEN 0x800, code 2; else code 0x4002. VOL `VOL[v & 15]` | 2 / 0x4002 |
| 3 | tone + noise | `p = 0`: as mode 2. Else 5.3.1 | 4 |
| 4 | envelope only | SID mode non-zero, or envelope period `e = 0`: as mode 0. Else `e` clamped to 10..0x11E1 (signed), set LC = waveform of pair `P[shape byte]`, LEN 0x10, PER `(e * 0xE517) >> 12`, VOL 64 | 0x40 |
| 5 | tone + envelope / SID | `p = 0`: as mode 4. Else 5.3.2 | 8 |
| 6 | noise + envelope | not supported: nothing set | 0x10 |
| 7 | tone + noise + envelope | not supported: nothing set | 0x20 |

#### 5.3.1 Tone + noise

`p = max(p, 10)`, `k = octave(p)`, `P = amiper(p, k)`. Set LC = buf, LEN 0x100, PER P, VOL `VOL[v & 15]`.
`step = ((P << 13) / (noise * 16 + 0x8C)) << 3` (unsigned 16-bit quotient; on overflow the 32-bit dividend
is used unchanged), a 16.16 value. The high word of the record's noise phase is masked `& 0x7FF`. Let `ph`
be the phase and `o = ((step >> 8) + (ph >> 16)) & 0x7FF`. For `j = 0..255`: `T` = NH if `(j >> k) & 1 = 0`,
else NL; `i = ph >> 16` (signed word); `buf[j] = T[i]`, `buf[256 + j] = T[i + o]`, then `ph = ph + step`.
The new `ph` is stored. (The second half uses the same phases shifted by `o`, which counts the old phase
twice. That is the original's arithmetic, and it is audible as noise only. Indices can run past the end of
NL; see README.)

#### 5.3.2 Tone + envelope, tone + SID

`p = max(p, 10)`, `k = octave(p)`, `P = amiper(p, k)`. Set LC = buf, LEN 0x100, PER P, VOL 64. The scratch
buffer is filled with 512 bytes of the square of octave `k` (`2^k` x 0x7F, `2^k` x 0x80, repeated). Then:

- SID mode 0 (YM envelope): `e` = envelope period, 0 -> 1, values `>= 0x11E1` (signed) -> 0x11E1. `E =
  ((e * 0xE517) >> 12) & 0xFFFF`. `step = ((P << 13) / E) << 3` (same overflow rule). With `ph` = the
  record's phase and `W` = its level table (32 words): for `j = 0..511`: `buf[j] = SC[W[(ph >> 16) & 31] >>
  8][scratch[j]]`, `ph = ph + step` (32 bits). Stored phase: high word `& 31`, low word kept.
- SID mode 1: `s` = SID period, values `>= 0x11E1` (signed) -> 0x11E1. `E = ((s * 0xE517) >> 12) & 0xFFFF`.
  `step = ((P << 13) / E) << 3`, capped to 0xFFFF when above 0x10000, then only its low 16 bits are used
  (exactly 0x10000 gives 0). With `L` = the record's two level words (high, low) and the low word of the
  phase: for `j = 0..511`: add `step` to the phase word. On a carry, swap the two words of `L`. The low byte
  of the current word is replaced by `scratch[j]`, and `buf[j] = SC` indexed by that whole word (level in
  the high byte). Both words and the phase are stored back.
- other SID modes: the buffer is not written (Paula plays its old content).

A SID period or envelope period whose `E` is 0 makes the reference's division fault. The reference stops
playing. With the clamps this happens only for SID period 0 or for a few envelope periods at or above
0x8000.

### 5.4 Flags after the computation

Let `f` be the record's flags (bits 15 and 14 were cleared by the commit) and `code` the result. If `code`
has bit 14, flags = `code`. Else if `code = f`, flags stay unchanged. Else flags = `code | 0x8000`. So:

- tone in the same octave: PER/VOL only every tick;
- a change of octave, or a change between modes: restart, with all four registers. The exception is when
  the previous flags equal the new code without bit 14 (for example two octave changes in a row in tone
  mode). Then there is no restart, only a full rewrite;
- modes 3, 4, 5, 6, 7: restart on entering, then a full rewrite every tick (modes 3 and 5 alternate
  between the two mix buffers).

### 5.5 Timing

The tick routine is entered at the host's call time `C`. `t` counts cycles from 0 at that point. A write
(or a read of the beam position) is stamped `C + floor(t / 2)`, where `t` includes the whole instruction
that writes. Both words of an LC write share one stamp. Costs:

| step | cycles |
|---|---|
| entry up to the start of `Y\` (type 0, no `Y[`) | 546 |
| type 2 | +8 |
| with `Y[`: entry to the start of `Y[` | 522 (+8 type 2); after `Y[` returns, 44 to the start of `Y\` |
| the module's routines | their own instructions, including their return |
| return of `Y\` to the emulator tick | 40 |
| emulator head: `+0x1C` negative / 0 / 1 / 2 or more | 157 / 326 / 351 / 359 |
| fixed block, then the start of the first commit | 107 + 42 |
| commit, entry | 25 |
| restart: to the DMACON write | +56, then +10 |
| no restart: test of bit 14 | +25 |
| PER/VOL only: PER write at +72, VOL write +84 later, then +16 | |
| full write (after a restart or the bit-14 test): +65, then LC at +28, LEN +12, PER +12, VOL +84, end +16 | |
| module-driven (bit 7), after the +65: | +17; counter 0: +16 end. Else +21; if the counter is still non-zero, the full write follows (+28 LC ...); if it became 0: LC (set B) at +28, LEN +12, end +26 |
| between two records | 34 |
| after the fourth record | 107 + 21, +28 if any record took the full-write or module-driven path with its bit in the mask, +21 |
| line wait (only if `R & mask`) | 16, then 11 rounds (below) |
| to DMACON on | 36; DMACON off 24 later |

A line-wait round: +8 and read the line byte `b`. Then repeat: +8 and read the line byte again. If it
differs from `b`, +19 and the round ends. Else +9 and read again. The line byte is `(clock / 227) mod 312`
(low 8 bits), and it changes at every multiple of 227 cc. So DMACON on comes 27..36 cc after the 11th line
start that follows the start of the wait.

Observed gaps in the logs (cc): restart DMACON -> LC 51/52, LC -> LEN 6, LEN -> PER 6, PER -> VOL 42, VOL ->
next record's DMACON 65/66. PER/VOL-only: PER -> VOL 42. With a restart in voice A, the first write of a
tick comes `(1167 + h + m) / 2` cc after the call (`h` = 0 for type 0, 8 for type 2, plus 20 and the cost of
`Y[` when present; `m` = the play routine's cycles; with `+0x1C = 1`). The ±1 comes from the floor.

## 6. How to check

`tools/replay-cmp.py REF.log PORT.log` compares tick by tick. The logs in `logs/` (LOGS.md) come from the
reference 68k player run through the host with DELI_WRITE_LOG. The synthetic modules make the module code
trivial (a list of word stores into the block per tick, cost `98 + 45 * stores` cycles, +8 at the loop
point). With them, every write including its colour clock is reproducible from this text alone. The host
calls the tick routine at 1 274 251 + 70 940 n cc for them.
