# ZX Spectrum Sound Tracker songs (ZXAY `ST11` and `STRC`): behaviour spec

Scope: the two ZXAY container types that hold Sound Tracker songs for the ZX Spectrum 128
(AY-3-8912): `ZXAYST11` (the editor's own, uncompiled song format, "Sound Tracker 1.1") and
`ZXAYSTRC` (the same song after Patrik Rak's compiler/compressor). This text gives the file layouts
and the rules by which the original ZX Spectrum Sound Tracker player plays them, so that an open
replayer can be written in `src/replay/` beside `ay_amad.rs`, on the project's AY emulation.

**Reference.** Owner's decision of 2026-11-16: ZXAY ST11/STRC are a ZX format and play by the rules
of the original Z80 player. Sections 2–9 are normative. How the Amiga Eagleplayer PlayAY plays the
same files was measured first and is kept in appendix A, for information only; where PlayAY and the
Z80 player differ, the Z80 player is right.

The Z80 player plays a *compiled* song (the Sound Tracker compiler's output, here "STC"), not the
ST11 file. So the reference is two steps: the ST11 (or STRC) song is compiled to STC by the rules
of section 5, which reproduce the original compiler byte for byte on the two songs where both forms
are available; the STC data is then played by the Z80 player (section 6).

Method: black box only.
- The original Z80 player is the "SONG BY ST COMPILE" player inside the EMUL rip `agent-x
  nonamedzak 01` (Project AY / modland, 4 901 bytes, sha256 `d16f93a1…c09a29da`; one block of
  `$FF43` bytes at `$C000`, init `$C000`, interrupt `$C006`, song data at `$C43C`). It ran on the
  project's Z80 (`src/replay/ay_z80.rs`) with the project's ZXAYEMUL host stub and timing
  (`src/replay/ay.rs`: 70 938 T-states per frame, interrupt 32 T). Observed: the AY register writes
  of each interrupt in order, the registers after it, and which addresses were fetched as
  instructions and which were read as data. The rips `agent-x nonamedzak 02`, `exterminator
  cheatmenu` and `hero quest cheatmenu` carry a player that behaves identically (their songs moved
  into the 01 rip give the same output as the rips themselves).
- Songs were put in place of the rip's song (tool `tools/zxay-st-ref.py`, section 9) and played:
  the 58 ST11 files, `strc.megamix1`, and about 15 series of synthetic STC songs written to isolate
  one rule each, plus 1 000 random STC songs (random codes, samples, ornaments, positions).
- Two rips hold songs whose ST11 source is in the test set: `agent-x nonamedzak 01` = `st11.AGENT1`,
  `agent-x nonamedzak 02` = `st11.AGENT4` (same delay, positions, samples). Their song data (data
  only: the bytes after `$C43C`) is the ground truth for the compiler, section 5.
- An executable model of sections 6–7 (a Python script, scratch tree, not in the repo) was compared
  with the Z80 player on every interrupt (section 10).

Clean room: no player code was read, disassembled, decompiled or transcribed: not the Z80 player
(only run; fetched addresses were used to tell code from data, never the code bytes' values), not
PlayAY, its AYPlayers, DeliPlayer or AYPlayer binaries. The RAMSOFT description of the ST11 format
(`st11fmt.txt`) was read for the layouts; where it and the black box disagree the black box wins
(section 11). No code of any player is in this text. Player-memory bytes a port needs are listed
in section 8.

Test files: Patrik Rak's DeliAYsongs (Aminet `mods/misc/DeliAYsongs.lha`): 57 ST11 songs in
`incoming/st11/` (each named "Some SoundTracker Song", credited to Slavo Labsky / BusySoft and
others), `st11/st11.megamix1` and `strc/strc.megamix1` (the same song, "Mega Mix 1", in both forms).
In the scratch tree: `dl-modules/ay-types/aminet-deliaysongs/x/`.

Numbers: `$` is hexadecimal. "Interrupt" is one 50 Hz call of the play routine; interrupt 0 is the
init call. Container words are big-endian; words inside song data are little-endian (Z80) unless
said otherwise.

Data files next to this text:
- `periods.tsv`: the 96 note periods of the Z80 player (AY units) and PlayAY's 256 (Paula units).
- `songs.tsv`: the 58 ST11 files: header values, interrupts per pass, PlayAY's song end, and the
  model's comparison with PlayAY (appendix A) and with the Z80 player.

## 1. Behaviour in brief (Z80 player)

| item | |
|---|---|
| timing | one interrupt per frame (50 Hz); a row every `delay` interrupts, the first at interrupt 1 |
| subsongs | one (the container's song count byte is 0 in every file) |
| voices | AY tone A, B, C; the AY noise; the AY hardware envelope (section 6.6) |
| periods | a 96-entry table (`periods.tsv`), ± a 12-bit sample offset, & `$FFF` |
| looping | after the last position: position 0 again, nothing reset; no end marker |
| song end | none in the player; a pass is `delay × patlen × positions` interrupts (section 7.3) |
| ST11 and STRC | STRC holds the same song with some tables cut short; see section 4 and 10 |

## 2. ZXAY container

| offset | size | meaning |
|---|---|---|
| 0 | 4 | `ZXAY` |
| 4 | 4 | type: `ST11` or `STRC` |
| 8 | 8 | 0 in every file (not tested) |
| 16 | 1 | number of songs − 1: 0 in every file |
| 17 | 1 | first song: 0 |
| 18 | 2 | signed offset, from offset 18, to the song table (2 → offset 20) |
| song table + 0 | 2 | signed offset, from this word, to the song name (0-terminated, Latin-1) |
| song table + 2 | 2 | signed offset, from this word, to the song data |

All files have the song table at 20, the name at 24.

## 3. ST11 song data

| offset (from song data) | size | meaning |
|---|---|---|
| 0 | 4 | channel assignment `00 01 02 03` (all files; other values not tested) |
| 4 | 2 | fade offset, big-endian (0 in all files) |
| 6 | 2 | fade length, big-endian (50; 255 in `st11.megamix1`) |
| 8 | … | the Sound Tracker song as the editor saves it, offsets below from here (B) |

| offset from B | size | meaning |
|---|---|---|
| 0 | 15 × 130 | samples 1–15 (sample s at (s − 1) × 130) |
| 1950 | 256 × 2 | positions: pattern number, transposition (signed byte) |
| 2462 | 1 | `length` = number of positions − 1 |
| 2463 | 17 × 32 | ornaments 0–16, 32 signed bytes each |
| 3007 | 1 | `delay`: interrupts per row (3–10 in the files) |
| 3008 | 1 | `patlen`: rows per pattern (22–64 in the files) |
| 3009 | n × 576 | patterns 1–n (pattern p at 3009 + (p − 1) × 576) |

The incoming files are exactly 3 009 + 576 n bytes after B; `st11.megamix1` has one byte more. n is
not stored: it is what the file holds. Four files name patterns beyond n in their position list
(OUTFOX 20 and 21, TURBO 10, ODZIAL 12, MEGA_N'1 7): section 5.4.

### 3.1 Sample (130 bytes)

| offset | size | meaning |
|---|---|---|
| 0 | 32 | volume per position, 0–15 |
| 32 | 32 | noise/mixer per position: bits 0–4 noise period; bit 6 set = tone OFF; bit 7 set = noise OFF; bit 5 not used (the compiler replaces it, section 5.2) |
| 64 | 32 × 2 | period offset per position, little-endian: bits 0–11 magnitude m, bit 12 sign: set → period + m (pitch lower), clear → period − m |
| 128 | 1 | `rep`: 0 = no loop; 1–32 = loop start position + 1 |
| 129 | 1 | `replen`: loop length − 1 |

### 3.2 Pattern

64 rows × 3 channels × 3 bytes, row-major (row 0 channel A, B, C, row 1 …); only the first `patlen`
rows play. Cell bytes:

- byte 1: bit 7 set → rest (the editor writes `$F0`–`$F7`). Bit 7 clear: bits 4–6 note name
  h: 0 = no note (empty cell), 1–7 = A, B, C, D, E, F, G (semitone 9, 11, 0, 2, 4, 5, 7); bit 3
  sharp (+1); bits 0–2 octave o. Note index n = 12 o + semitone + sharp (0–95).
- byte 2: high nibble sample number (0 = none given), low nibble effect.
- byte 3: effect parameter.

Effects (meaning on the Z80 player, through the codes of section 5.3; the files use 0, 1, 10, 12,
13, 14, 15):

| effect | meaning |
|---|---|
| 0 | none |
| 1 | ornament 0, envelope off |
| 2 | compiled as effect 1 (no file uses it; unverified) |
| 3–14 | hardware envelope: shape = effect, R11 = parameter, R12 = 0, envelope on, ornament 0 |
| 15 | ornament = parameter & 15, envelope off |

Ornament 16 is stored but cannot be selected.

## 4. STRC song data

Decoded by laying `strc.megamix1` beside `st11.megamix1`; one file only.

| offset (from song data) | size | meaning |
|---|---|---|
| 0 | 4 | channel assignment `00 01 02 03` |
| 4 | 2 | big-endian: Z80 load address L of the block (`$C67E`) |
| 6 | 2 | big-endian fade offset (0) |
| 8 | 2 | big-endian fade length (50) |
| 10 | … | the block, loaded at L; all pointers in it are little-endian absolute Z80 addresses |

Block, offsets from L:

| offset | size | meaning |
|---|---|---|
| 0 | 1 | `delay` |
| 1 | 1 | `patlen` |
| 2 | 16 × 2 | pointer to the period-offset table of sample k (k = 0–15; 0 unused): 32 signed 16-bit little-endian values (ST11's sign/magnitude as two's complement: bit 12 set ↔ positive) |
| 34 | 16 × 2 | pointer to ornament k (k = 0–15): 32 signed bytes |
| 66 | 16 × 2 | pointer to the volume table of sample k: 32 bytes |
| 98 | 16 × 2 | pointer to the noise/mixer table of sample k: 32 bytes, bits as ST11 |
| 130 | 16 × 2 | sample k loop: `rep` (as ST11), loop length (= ST11 `replen` + 1) |
| 162 | 7 × positions | per position: pointers to the channel A, B, C row streams, transposition byte |
| after | 2 | 0: end of the position list |

Row stream: one entry per row, `patlen` entries per position: `$FE` = empty cell, `$FF` = rest, else
three bytes: note index 0–95, sample/effect byte (as ST11 byte 2), parameter (as ST11 byte 3). A
cell without a note becomes `$FE` even if it had nibbles; a rest keeps nothing.

The tables are shared and overlapped: several pointers point at the same bytes, and a table holds
only what PlayAY can play (a sample's tail after the point where it is silenced is cut and the next
table follows; sample 3's pointers, unused in the song, point at `$7F91`, outside the block). Read
each table as 32 entries from its pointer, whatever follows.

No Z80 player reads STRC (the compressor is an Amiga-side tool; the other EMUL rips seen, `intra2`
and `zx club #5`, hold "KSA SOFTWARE COMPILATION" Sound Tracker Pro players with another layout).
**The Z80 reference of an STRC song is: decode it to ST11 cells (patterns = the position list's row
streams, samples and ornaments = the tables read as above, missing tables = 0), then sections 5–6.**
The cut tables are the only loss: on `strc.megamix1` against `st11.megamix1`, R6 differs on 120 and
R7 on 1 526 of 6 000 interrupts, of which 56 can be heard (noise period of a sounding noise
channel; the R7 differences are mixer bits of silent channels). Not tested on STRC: other load
addresses, a non-zero transposition, note bytes 96–`$FD`, more than one file.

## 5. Compiling to STC (the reference conversion)

These rules give the song data of `agent-x nonamedzak 01` from `st11.AGENT1` (3 625 bytes) and of
`agent-x nonamedzak 02` from `st11.AGENT4` (2 774 bytes) byte for byte. (The two rips have two bytes
`03 F2` after the data, not part of it: the header's data length excludes them and nothing reads
them.) What those two songs do not exercise is marked unverified; section 5.5 lists it.

### 5.1 Layout

| offset | size | meaning |
|---|---|---|
| 0 | 1 | `delay` |
| 1 | 2 | offset of the position list (all offsets from the start of the data, little-endian) |
| 3 | 2 | offset of the ornaments |
| 5 | 2 | offset of the pattern table |
| 7 | 18 | `SONG BY ST COMPILE` |
| 25 | 2 | length of the data |
| 27 | 99 × k | samples (5.2) |
| … | 1 + 2 × positions | position list: positions − 1, then per position pattern number, transposition |
| … | 33 × j | ornaments: number, 32 bytes, for ornament 0 and each ornament a stream selects, increasing |
| … | 7 × patterns + 1 | pattern table: number, offsets of the A, B, C streams, for each pattern the position list names (increasing); then `$FF` |
| … | | the streams, in pattern order A, B, C; a stream byte-equal to one already written is not written again, its offset points at that one |

### 5.2 Samples

Each sample a stream selects (5.3; the set of sample codes in all streams), increasing: its number,
then 32 rows of 3 bytes: `volume | (m >> 8) << 4`, `noise byte & $DF | $20 if sign bit 12`,
`m & $FF`; then `rep`, `replen`.

### 5.3 Channel streams

One stream per pattern and channel, rows 0 to `patlen` − 1:
- an empty cell (byte 1 bit 7 clear, note name 0) is an empty row; its nibbles are dropped
  (verified on two row-0 cells only);
- a note: [sample code] [effect code] note index (`$00`–`$5F`);
- a rest: [sample code] [effect code] `$80` (no rest with nibbles in the two ground-truth songs);
- the sample code `$60 + s` is written when the cell has a sample nibble s and s differs from the
  last sample code of this stream;
- the effect code is written when the cell has an effect and its code differs from the last effect
  code of this stream: effect 1 and 2 → `$82`; 3–14 → `$80 + effect`, parameter; 15 →
  `$70 + (parameter & 15)`;
- both "last" states start empty at each stream (each pattern, each channel);
- durations: the number d of empty rows after an event (a note, a rest, or the `$81` below; up to
  the pattern end for the last one) is written as `$A1 + d` before the event's codes (before its
  sample code) on the stream's first event and whenever it differs from the last one written;
- a stream whose row 0 is empty starts with `$81` (an empty event); then `$FF`.

### 5.4 Patterns named but not stored

A position naming a pattern the file does not hold gets `patlen` empty rows (`$A1 + patlen − 1`,
`$81`, `$FF` in all three streams, written once by 5.1's sharing). Unverified: what the original compiler does there.

### 5.5 Not covered by the ground truth

| case | files | conversion | effect on the Z80 output |
|---|---|---|---|
| an envelope code equal to the stream's last effect code (the same envelope again with nothing between) | 18 (AGENT5, AMIG99, AMIGA1, AMIGA2, AXELFOLY, BARDAC, BEVERLY, BILIN3, FATHER, HOUSEM, HYMN1, HYMN2, MIKROP, MY-OWN, MYSTER, THE_FLY, both megamix) | left out, as every repeated code | with `--repeat-envelope` the envelope is restarted (R11–R13 written) on each such note: 8–1 762 interrupts differ per file |
| empty cells with nibbles after row 0 (1 067 cells in played patterns) | 38 | dropped | keeping them (as codes + `$81`, with codes on every cell) changed the output of 30 files |
| rests with sample or effect nibbles | 9 | codes kept before `$80` | |
| named, missing patterns | 4 (OUTFOX, TURBO, ODZIAL, MEGA_N'1) | empty | |
| effect 2; effects 3–9 | none | `$82`; `$83`–`$89` | |
| samples or ornaments selected only by unplayed patterns or by empty cells | none | not included | |

The four rips never repeat an envelope code in a stream, but their songs' sources, except AGENT1 and
AGENT4, are not in the test set, and AGENT1/AGENT4 have no repeated envelope, so this does not
decide the first row.

## 6. The Z80 player over the STC data

State is described over a 64 KB memory image: the STC data at `$C43C`, the player's period table at
`$C35F`–`$C41E` (section 8), zeros after the data, and the host's low memory (6.7). The rest of the
player block (`$C000`–`$C35E`, `$C41F`–`$C43B`) is player code and variables; a port never reads
it, and the one case where the player does (index 96–110, section 7.1) is not reproducible.

Lookups by number scan the record lists from their start for the first record whose number byte
matches: samples from data offset 27 in steps of 99, ornaments from the ornament offset in steps of
33, patterns from the pattern-table offset in steps of 7. A missing number was not tested.

### 6.1 State

Per channel: row base R (address of the selected sample's first row; 0 until a sample code),
ornament address O (ornament 0 at start), envelope flag, new-envelope flag, note, sample position
p, counter c (`$FF` = silent), stream pointer, duration d, rows to wait w. Song: position index,
transposition t of the current position, tick, envelope values (shape, parameter) and a pending
flag.

### 6.2 Interrupt 0 (init)

Writes R10, R9 … R0 = 0, in that order. R11–R13 are not written. All channels silent.

### 6.3 Each interrupt from 1

1. On the first interrupt, load position 0. Then if tick = 0, read a row: for each channel A, B,
   C: if w > 0, w − 1 and nothing else; else read stream codes until an event:
   - `$FF` on channel A: next position (past the last one: position 0); load it (all three stream
     pointers set, w = 0, t = its transposition) and go on reading channel A.
   - `$00`–`$5F`: note = code; p = 0; c = 32. Event.
   - `$60`–`$6F`: R = the sample's first row.
   - `$70`–`$7F`: O = ornament (code − `$70`); envelope flag off.
   - `$80`: c = `$FF` (silent). Event.
   - `$81`: nothing. Event.
   - `$82`: O = ornament 0; envelope flag off. No parameter.
   - `$83`–`$8E`, parameter: shape = code − `$80`, envelope parameter = parameter; pending on;
     channel's new-envelope flag on; envelope flag on; O = ornament 0.
   - `$A1`–`$FE`: d = code − `$A1`.
   After the event, w = d. (`$8F`–`$A0` not produced by the compiler; `$FF` on B or C: the
   compiler makes the three streams equally long, other cases not tested.)
2. tick = (tick + 1) mod `delay`.
3. For each channel A, B, C (stage 6.4–6.5), then the registers (6.6).

The pattern length is channel A's: its `$FF` moves all three on. Transposition and ornament changes
apply from the interrupt they are read in; a code on a row without a note (e.g. an ornament on an
empty row reached by a duration) applies at once to the sounding sample.

### 6.4 Sample stage

If c = `$FF`: the channel is silent (6.5). Else c = (c − 1) & `$FF`, then:
- if c = 0: rep = byte R + 96. If rep = 0 or rep ≥ 129 (i.e. (rep − 1) & `$80`): the channel goes
  silent from now (c = `$FF`) and this interrupt is an *off* interrupt. Else p = rep − 1 (0–127, not
  masked) and c = (byte R + 97 + 1) & `$FF`; if that gives `$FF` (replen 254) the channel goes
  silent and this interrupt is an off interrupt.
- if the decrement gives `$FF` (only after a loop with replen 255, c = 0): off interrupt, silent.
- row address = R + ((3 p) & `$FF`): bytes b0, b1, b2 (they may lie beyond the 99-byte record).
- off interrupt: volume 0; channel A takes its mixer bits from b1 (bit 6 tone off, bit 7 noise off),
  B and C mixer bits 0; period registers and R6 unchanged; p not advanced.
- else (sounding):
  - o = byte O + p (signed; p up to 127, so this may read beyond the 32-byte ornament);
  - index i = (note + t + o) & 127; base = word at `$C35F` + 2 i (section 7.1 for i ≥ 96);
  - m = (b0 >> 4) << 8 | b2; period = (base + m if b1 & `$20` else base − m) & `$FFF` → the
    channel's tone period registers;
  - volume = (b0 & 15) | `$10` if the envelope flag is set;
  - mixer bits: tone off = b1 bit 6, noise off = b1 bit 7; if noise is on (bit 7 clear),
    R6 = b1 & 31 (the last such channel of A, B, C wins; R6 keeps its value otherwise);
  - envelope: if the envelope flag is set: if the new-envelope flag is set clear it, else clear
    the song's pending flag;
  - p = (p + 1) & 31.

So a note plays sample positions 0–30 (31 interrupts, not 32) from the interrupt that reads it;
at the 32nd it loops to `rep` − 1 and plays `replen` + 1 positions per loop, or stops.

### 6.5 Silent channels

Volume 0; tone period registers unchanged; mixer bits of B and C 0. Channel A's mixer bits are bits
6 and 7 of the byte at R + L + 1, where L depends on what the interrupt's row reading did last:

| last lookup of this interrupt (any channel) | L |
|---|---|
| none (no row read, or a row with notes / rests / durations only) | 247 |
| a sample code | 41 |
| an ornament code, `$82` or an envelope code | 99 |
| a position change (also on interrupt 1) | 180 |

This is a fitted observation (it holds on all tests), it only changes R7 bits of a silent channel
(inaudible), and a port may ignore it if it does not aim at register-exact output.

### 6.6 Registers

Per interrupt, written in this order: R13, R12, R11 (only if the pending flag is still set after
the sample stage: R13 = shape, R12 = 0, R11 = parameter; writing R13 restarts the envelope), then
R10, R9, R8 (volumes), R7 (mixer: bit c tone off, bit c + 3 noise off, c = 0, 1, 2; bits 6–7 0),
R6, R5 … R0 (C, B, A periods, values as held). Every register R0–R10 is written at every interrupt.

The pending flag means: an envelope code read this interrupt is written at this interrupt; it is
cleared as soon as an envelope channel sounds an interrupt after the one of its own code. If no
channel with the envelope flag sounds (the channel is silent, or an ornament code cleared the flag
before), it stays set and R11–R13 are written again at every interrupt until one does.

### 6.7 Host low memory

R = 0 until the channel's first sample code, so a note before any sample code plays the bytes at
`$0000` as a sample. In the ZXAYEMUL host (`ay.rs`), `$0000`–`$00FF` is `$C9` except the boot
stub `F3 CD 00 C0 ED 56 FB 76 CD 06 C0 18 F7` at `$0000` and `$FB` at `$0038`; `$0100`–`$3FFF` is
`$FF`. Such a note's rep (`$C9`) means no loop. These bytes are the project's own host, not player
data.

## 7. Periods, indexes beyond the table, length

### 7.1 Period table

Index 0–95: octave k = i div 12 of `3832, 3600, 3424, 3200, 3032, 2856, 2696, 2544, 2400, 2272,
2136, 2016`, shifted right by k (measured, `periods.tsv`).

Index 96–127 (reached through ornaments and transpositions; the index is masked to 127) reads past
the table: 96–110 read player code bytes (`$C41F`–`$C43B`, 110's high byte is the STC delay byte).
Those values may not be used (code bytes, section 8): a port cannot reproduce them, and the tone
period of such a channel was excluded from the comparison. Files and interrupts (of 20 000):
NSZZ 300, MANUTD 108, AMIGA_3 102, AMIGA_3_ 102, BILINS 36, AGENT4 13, ODZIAL 12, HYMN1 4,
HYMN2 4 (`songs.tsv`). A port should pick a value of its own (suggestion: silence the tone, or
index & 95) and say so. Index 111–127 read the STC data's bytes 1–34 (header offsets, text, length,
the first sample's number and first row): part of the memory image, so a port that builds the STC
image reproduces them.

### 7.2 Positions and transposition

Transposition is added modulo 128 with the note and ornament (signed byte, the files use 0, 2, 3,
4, 5 and −2).

### 7.3 Length

A pass is `delay × patlen × positions` interrupts (`songs.tsv`); then position 0 again with every
channel as it was. The player has no end; the reference uses one pass with no fade
(`--length`, section 9). The container's fade fields are not used. Unverified: `delay` 0 and
`patlen` 0.

## 8. Player-memory bytes a port needs

| bytes | what | kind | status |
|---|---|---|---|
| `$C35F`–`$C41E` (192 bytes) | the 96 periods of 7.1 (12 values and shifts) | data: read as data, never fetched | **needs the owner's approval** (player data); the values were measured as register output, not read from the rip |
| `$C41F`–`$C43B` | read by indexes 96–110 | code: fetched as instructions | **not allowed**; not modelled (7.1) |
| `$C070`–`$C0AE` | the player's variables | data (run-time state) | not needed: the init sets the state of 6.1–6.2 |

Nothing else of the player block is needed. The constants of 6.5 (247, 41, 99, 180) are observed
behaviour, not memory contents.

## 9. Reference procedure

1. Build the reference EMUL: the song compiled to STC and put in place of the rip's song:

       python3 -I tools/zxay-st-ref.py SONG "agent-x nonamedzak 01.emul" OUT.emul --length

   (`--stc OUT.stc` also writes the STC data; `--repeat-envelope` the 5.5 alternative). The rip is
   only copied; its block length and, with `--length`, its song length (one pass) and fade (0) are
   set.
2. Audio: `SDL_AUDIO_DRIVER=dummy target/release/amigaplayer-rs --wav OUT.wav --seconds N OUT.emul`
   (the project's ZXAYEMUL player: its Z80 runs the original player).
3. Registers per interrupt:

       AY_DUMP=OUT.emul AY_FRAMES=N AY_WRITES=1 AY_OUT=OUT.log \
         cargo test --release --lib amad_tests::regdump -- --ignored

   gives per interrupt R0–R13 (hex) and, after ` |`, the writes in order (`register=value`).
   Line 1 is interrupt 0 (the init). A port matches when its writes are equal on each interrupt,
   excluding the tone period of a channel at index 96–110.

## 10. Verification

| check | result |
|---|---|
| section 5 vs the original compiler: `st11.AGENT1` → rip 01 data, `st11.AGENT4` → rip 02 data | byte-identical (3 625 and 2 774 bytes) |
| rip 01 and rip 02 as they are vs their songs converted from ST11 and put in rip 01 | identical AY writes on 20 001 interrupts each |
| model (sections 6–7) vs Z80 player, 58 ST11 + `strc.megamix1`, interrupts 0–19 999 | equal on all interrupts for 59 of 59 (tone periods at index 96–110 excluded, 7.1) |
| model vs Z80 player, 1 000 random STC songs × 3 000 interrupts | 0 songs unequal |
| `regdump` with `AY_WRITES=1` vs the scratch harness (same Z80) | identical logs, 59 of 59 |
| period table | measured for indexes 0–95 |

Unverified: the cases of 5.5 (repeated envelope codes, empty-cell nibbles after row 0, rests with
nibbles, missing patterns, effect 2 and 3–9 in the compiler); `delay` 0, `patlen` 0 and above 64;
code `$8F`; streams B/C shorter than A; a missing sample/ornament/pattern number; container bytes
8–15 and other channel assignments; the STRC fields beyond the one file.

## 11. Errata against the RAMSOFT description (`st11fmt.txt`)

- Byte 1 bit 3 is the sharp (+1 semitone), not a flat.
- Note names: high nibble 1 = A … 7 = G and 0 = no note (the description has 0 = A … 7 = G).
- The noise byte's bit 6 (the description's "envelope mask") is the tone mask: set = tone off.
- The effect meanings (section 3.2) are not in the description.

## Appendix A. PlayAY (informative, not the reference)

Eagleplayer 2.06 PlayAY (AYPlayers `ZXAYST11`, `ZXAYSTRC`) ran in this project's 68000 host
(`--68k-first --player PlayAY --filter none --wav`, `SDL_AUDIO_DRIVER=dummy`); its Paula writes
(`DELI_WRITE_LOG`) were reduced to the channel state after each interrupt, and its AY-to-Paula
stage was calibrated with EMUL files of our own. A model of the rules below matched PlayAY on 58 of
58 ST11 files (15 000 interrupts or to the song end) and on `strc.megamix1` (5 379 interrupts);
PlayAY plays `st11.megamix1` and `strc.megamix1` identically. It differs from the Z80 player in:

- it plays the ST11 cells directly, without compiling: sample/effect nibbles only on notes; a rest
  (high nibble 15) also clears the sample; a note without a sample is silent; high nibbles 8–12 are
  notes with semitones 0, 32, 82, 112, 0; effects 2–6 do nothing; 7–14 envelope, 15 ornament;
- samples play 32 positions on the first pass (Z80: 31), sample position & 31, loop as rep/replen;
- periods: `round(30714.9 · 2^(−i/12))` in Paula units (8 × an AY period, not an integer AY
  period, up to +98 cents from the Z80 table in the top octave), index & 255, values 96–255 from
  memory after PlayAY's table (`periods.tsv`), ± 8 m, & `$FFFF`;
- AY to Paula: tone A, B, C on Paula 0–2 (4-byte square, PER = 8 × AY period), Paula volume
  `[0, 1, 2, 2, 3, 3, 9, 15, 21, 27, 33, 39, 45, 51, 57, 63][v]`, hardware envelope ignored
  entirely, noise on Paula 3 (PER = 124 + 32 × R6, volume the largest of the noise channels);
- song end: reported after one pass for 22 of 58 files, never or later for the others; the rule
  depends on bytes outside the played song and was not found; no fade observed;
- named, missing patterns play as empty rows (the same as the Z80 reference's choice, 5.4).

Its 50 Hz interrupt is 70 940 colour clocks. `songs.tsv` keeps its song end and the model's
comparison (columns `playay_song_end_s`, `pa_model_*`).

## Errata (found while implementing `src/replay/ay_st.rs`, 2026-11-16)

1. Section 6.4: `period = (base ± m) & $FFF` holds for the sound, not for the writes. The original
   writes the high byte of the unmasked 16-bit result (e.g. R1 = `$11`, R3 = `$F0`); the chip ignores
   the high nibble. A register-exact port writes the 16-bit result.
2. STC stream and table offsets are relative to the song data: add the data's load address (`$C43C`
   in the reference image) to reach them.
3. Not covered above, chosen by the port (unverified against the original): a missing sample,
   ornament or pattern number leaves the channel as it was (pattern: the first record); a `$FF` on
   channel B or C is ignored; `delay` 0 plays as 1; note index 96 (B♯ of octave 7) is an empty cell;
   `patlen` above 64 is refused.
4. Write timing (not in this spec): the original writes R0–R13 1 500–4 500 T-states into the frame,
   56 T apart, the start varying with the code path; the port writes them at the frame start, so its
   audio is about 1 ms early (refcmp SIMILAR, not IDENTICAL). Measurable black-box from register-write
   logs if wanted.
