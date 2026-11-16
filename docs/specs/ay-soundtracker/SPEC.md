# ZX Spectrum Sound Tracker songs (ZXAY `ST11` and `STRC`): behaviour spec

Scope: the two ZXAY container types that hold Sound Tracker songs for the ZX Spectrum 128 (AY-3-8912):
`ZXAYST11` (the editor's own, uncompiled song format, "Sound Tracker 1.1") and `ZXAYSTRC` (the same
song after Patrik Rak's compiler/compressor). This text gives the file layouts and how the
Eagleplayer PlayAY (AYPlayers `ZXAYST11` and `ZXAYSTRC`) plays them, so that an open replayer can be
written in `src/replay/` beside `ay_amad.rs`. Where the original Z80 Sound Tracker play routine
behaves differently, that is given too (section 8).

Method: black box only.
- PlayAY of Eagleplayer 2.06 ran in this project's 68000 host (`--68k-first --player PlayAY`,
  `--filter none --wav`, `SDL_AUDIO_DRIVER=dummy`); its Paula writes were logged
  (`DELI_WRITE_LOG`) and reduced to the state of the four Paula channels after each 50 Hz interrupt
  (period, volume, sample start and length). This is the observable channel for everything marked
  "PlayAY".
- PlayAY's AY-to-Paula stage was calibrated with EMUL files written for this work (a small Z80
  routine of our own that writes chosen AY registers at each interrupt), section 7.
- The original Z80 Sound Tracker player: the "SONG BY ST COMPILE" player inside the EMUL rips of
  agent-x (`nonamedzak 01`, player at `$C000`, init `$C000`, interrupt `$C006`, song data `$C43C`)
  ran on the project's Z80 (`ay_z80.rs`); the 58 ST11 songs below were compiled to that player's
  data format by a script of ours and put in place of the rip's song, and the AY registers were
  compared after each interrupt (section 8).
- About 70 synthetic ST11 songs, each changing one thing (a note code, an effect, a sample loop, a
  pattern count, bytes appended to the file), were played through PlayAY.
- An executable model of the rules below (a Python script, kept in the scratch tree, not in the repo)
  was run against the logs: see section 11.

No player was read, disassembled, decompiled or traced: neither PlayAY nor its AYPlayers, nor the Z80
code of the EMUL rips. PlayAY's developer sources in the DeliTracker II bonus archive were not
opened; only its readme (`DeliAY014.readme`) and `dev.doc` were read. The RAMSOFT description of
the ST11 format (`st11fmt.txt`) was read for the layout; where it and the black box disagree, the
black box wins (section 10). No code of any player is in this text.

Test files: Patrik Rak's DeliAYsongs (Aminet `mods/misc/DeliAYsongs.lha`): 57 ST11 songs in
`incoming/st11/` (each named "Some SoundTracker Song", credited in the archive to Slavo Labsky /
BusySoft and others), `st11/st11.megamix1` and `strc/strc.megamix1` (the same song, "Mega Mix 1", in
both forms). In the scratch tree: `dl-modules/ay-types/aminet-deliaysongs/x/`.

Numbers: `$` is hexadecimal. "Interrupt" is one 50 Hz call of the play routine. "cc" is an Amiga
colour clock (3 546 895 per second). Container words are big-endian; words inside the song data are
little-endian (Z80) unless said otherwise.

Data files next to this text:
- `periods.tsv`: the 96 note periods of the Z80 player (AY units) and the 256 of PlayAY (Paula
  units), with the formula.
- `songs.tsv`: the 58 ST11 files: header values, frames per pass, PlayAY's song end, and how the
  model compares with PlayAY and with the Z80 player.

## 1. Reference behaviour in brief

| item | PlayAY (`ZXAYST11`, `ZXAYSTRC`) |
|---|---|
| timing | one interrupt per 1/50 s (70 940 cc); a row every `delay` interrupts |
| subsongs | one (the container's song count byte is 0 in every file) |
| voices | three AY tone channels → Paula channels 0–2; the AY noise → Paula channel 3 |
| periods | a 96-entry equal-tempered table, `round(30714.9 · 2^(−n/12))` in Paula units (= 8 × the AY period); not the Z80 player's table (up to +98 cents apart in the top octave) |
| volumes | AY volume 0–15 → Paula volume by a fixed table; hardware envelope ignored |
| looping | after the last position the song restarts at position 0, sound state kept |
| song end | reported after one pass for some files and never for others (section 9; rule not found) |
| fade | the header's fade fields have no observed effect |
| ST11 and STRC | play identically: `st11.megamix1` and `strc.megamix1` give the same Paula state on every one of 5 379 interrupts |

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

All files have the song table at 20, the name at 24. PlayAY takes a file by the `ZXAY` + type
identifier; other checks were not tested. The name shown is the song name (`Some SoundTracker Song`
for the 57 incoming files, `Mega Mix 1` for the megamix).

## 3. ST11 song data

| offset (from song data) | size | meaning |
|---|---|---|
| 0 | 4 | channel assignment `00 01 02 03` (all files; other values not tested) |
| 4 | 2 | fade offset, big-endian (0 in all files) |
| 6 | 2 | fade length, big-endian (50; 255 in `st11.megamix1`) |
| 8 | … | the Sound Tracker song as the editor saves it, offsets below from here (B) |

The header matches the readme's example (`dc.b 0,1,2,3 ;channel assignment`, `dc.w 0 ;fade offset`,
`dc.b 0,50 ;fade len`).

| offset from B | size | meaning |
|---|---|---|
| 0 | 15 × 130 | samples 1–15 (sample s at (s − 1) × 130) |
| 1950 | 256 × 2 | positions: pattern number, transposition |
| 2462 | 1 | `length` = number of positions − 1 |
| 2463 | 17 × 32 | ornaments 0–16, 32 signed bytes each |
| 3007 | 1 | `delay`: interrupts per row (3–10 in the files) |
| 3008 | 1 | `patlen`: rows per pattern (22–64 in the files) |
| 3009 | n × 576 | patterns 1–n (pattern p at 3009 + (p − 1) × 576) |

The incoming files are exactly 3 009 + 576 n bytes after B; `st11.megamix1` has one byte more.
n is not stored: it is what the file holds. Four files name patterns beyond n in their position list
(OUTFOX 20 and 21, TURBO 10, ODZIAL 12, MEGA_N'1 7); PlayAY plays those positions as empty rows
(nothing new starts; what plays continues), and the model doing the same matches all four. Pattern
number 0 in a played position was not tested.

### 3.1 Sample (130 bytes)

| offset | size | meaning |
|---|---|---|
| 0 | 32 | volume per position, 0–15 (the model uses the low nibble; no file sets the high one) |
| 32 | 32 | noise/mixer per position: bits 0–4 noise period; bit 6 set = tone OFF; bit 7 set = noise OFF; bit 5 not seen used |
| 64 | 32 × 2 | period offset per position, little-endian: bits 0–11 magnitude m, bit 12 sign: set → the period is raised by m (pitch lower), clear → lowered by m |
| 128 | 1 | `rep`: 0 = no loop; 1–32 = loop start position + 1 |
| 129 | 1 | `replen`: loop length − 1 |

### 3.2 Pattern

64 rows × 3 channels × 3 bytes, row-major (row 0 channel A, B, C, row 1 …); only the first `patlen`
rows play. Cell bytes:

- byte 1, high nibble h:
  - 0: no note (empty cell);
  - 1–7: note names A, B, C, D, E, F, G (semitone 9, 11, 0, 2, 4, 5, 7);
  - 15: rest (`$F0`–`$FF`; the files use `$F0`–`$F7`);
  - 8–12 (never written by the editor): PlayAY plays them as notes with semitone values 0, 32, 82,
    112, 0; 13 and 14 not measured.
  - bit 3: sharp (+1 semitone); bits 0–2: octave o.
  - note index n = 12 o + semitone + sharp (C of octave 0 = 0, B of octave 7 = 95).
- byte 2: high nibble sample number (0 = keep the channel's sample), low nibble effect.
- byte 3: effect parameter.

Effects (PlayAY; the files use 0, 1, 10, 12, 13, 14, 15):

| effect | PlayAY |
|---|---|
| 0 | none |
| 1 | ornament 0, envelope off |
| 2–6 | no visible effect (the ornament is kept) |
| 7–14 | envelope: shape = effect, period = byte 3 (R11 = byte 3, R12 = 0); ornament reset to 0. Not audible in PlayAY (section 7) |
| 15 | ornament = byte 3 & 15 (high nibble ignored), envelope off |

Ornament 16 is stored but cannot be selected. Ornament 0 is whatever the file stores there (all
zeros in the editor's files; the model uses the stored values and matches).

## 4. STRC song data

Decoded by laying `strc.megamix1` beside `st11.megamix1`; one file only, so every field below was
checked on that file alone, by playing the decoded song through the model against PlayAY's log of
the STRC file (0 differences in 5 379 interrupts).

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
| 2 | 16 × 2 | pointer to the period-offset table of sample k (k = 0–15; 0 unused): 32 signed 16-bit little-endian values added to the period in AY units (ST11's sign/magnitude turned into two's complement: bit 12 set ↔ positive) |
| 34 | 16 × 2 | pointer to ornament k (k = 0–15): 32 signed bytes |
| 66 | 16 × 2 | pointer to the volume table of sample k: 32 bytes |
| 98 | 16 × 2 | pointer to the noise/mixer table of sample k: 32 bytes, bits as ST11 |
| 130 | 16 × 2 | sample k loop: `rep` (as ST11), loop length (= ST11 `replen` + 1) |
| 162 | 7 × positions | per position: pointers to the channel A, B, C row streams, transposition byte |
| after | 2 | 0: end of the position list (number of positions = entries before it) |

Row stream: one entry per row, `patlen` entries per position: `$FE` = empty cell, `$FF` = rest, else
three bytes: note index 0–95, sample/effect byte (as ST11 byte 2), parameter (as ST11 byte 3).
The compressor drops what PlayAY ignores: a cell without a note becomes `$FE` even if it had a
sample or effect nibble (two such cells in the megamix), a rest keeps nothing.

The tables are shared and overlapped: several pointers point at the same bytes, a table holds only
what can play (a sample's tail after the point where it is silenced is cut, and the next table
follows), and sample 3's pointers (unused in the song) point at `$7F91`, outside the block. Read
each table as 32 entries from its pointer, whatever follows; that is what played identically.
Not tested on STRC: other load addresses, a non-zero transposition, note bytes 96–`$FD`, more
than one file.

## 5. The sequencer (PlayAY)

State per channel: on/off, note index, sample number, ornament number, envelope flag, sample
position, positions left. Song state: position index, row, tick counter, transposition of the
current position. At start: position 0, row 0, tick 0, all channels off with sample 0, ornament 0.

At each interrupt:

1. If the tick counter is 0, read a row (the first interrupt reads row 0 of position 0):
   - if row = `patlen`: row = 0, position + 1; past position `length`, position 0. Take the
     position's pattern and transposition.
   - for each channel A, B, C, by the cell's byte 1:
     - h = 0 (empty): nothing; the sample and effect nibbles are ignored.
     - h = 15 (rest): channel off and its sample number set to 0 (a later note without a sample
       nibble stays silent until a sample is given). Sample/effect ignored.
     - a note: if the sample nibble is non-zero it becomes the channel's sample; apply the effect
       (section 3.2); note index = n; channel on; sample position 0; 32 positions to play.
   - row + 1.
2. tick = (tick + 1) mod `delay`.
3. For each channel that is on and has a sample s ≠ 0 (a note on a channel that never had a sample
   is silent), at sample position p (taken & 31):
   - index i = (note + transposition + ornament[p]) & 255 (ornament value signed);
   - Paula period = PlayAYTable[i] + 8 m if the offset's sign bit is set, else − 8 m; taken
     & `$FFFF` (BFAS_2 wraps this way); 0 → the channel is silent;
   - volume v = sample volume[p] & 15; mixer bits from the noise byte; if noise is on for this
     channel, the AY noise period R6 = noise byte & 31 (channels in order A, B, C: the last one
     wins; R6 keeps its value when no channel sets it);
   - then advance: p + 1, positions left − 1; when it reaches 0: if `rep` = 0 the channel goes off,
     else p = `rep` − 1 and positions left = `replen` + 1 (and again at each end of the loop).
   A channel that is off or without a sample gives no sound (AY volume 0).

So a note sounds from the interrupt that reads it; a sample without loop plays positions 0–31 once
(32 interrupts) then stops; with a loop it plays 0–31 once, then positions `rep` − 1 … `rep` − 1 +
`replen` (mod 32) over and over.

Transposition: added modulo 256, so it acts as a signed byte. Played positions in the files use 0,
2, 3, 4, 5 and 254 (−2).

Period table (`periods.tsv`): indexes 0–95 = `round(30714.9 · 2^(−i/12))` exactly (30 715 … 127).
Indexes 96–255 are read from memory after PlayAY's table: the values in `periods.tsv` are those seen
(several are 0, i.e. silent), and 232–255 look like changing run-time state. A replayer may use them
as listed; in the files JMJ, ODZIAL, NSZZ and BILINS (ornaments or transpositions that leave
0–95) reach them, and the model with the listed values matches all four.

## 6. AY registers

Expressed as the AY registers an AY-based replayer (like `ay_amad.rs` with `ay.rs`) would write
after each interrupt for the PlayAY behaviour:

| register | value |
|---|---|
| R0/R1, R2/R3, R4/R5 | tone period of A, B, C. PlayAY's pitch is Paula period / 8 (PlayAYTable[i] / 8 ± m, not an integer in general; the Z80 player's integer table is in section 8) |
| R6 | noise period as section 5 |
| R7 | mixer: bit c = tone off (noise byte bit 6), bit c + 3 = noise off (bit 7), c = 0, 1, 2 |
| R8, R9, R10 | v of the channel; v \| `$10` while the channel's envelope flag is set; 0 when the channel is off |
| R11, R12, R13 | on a row with effect 7–14: R11 = parameter, R12 = 0, R13 = shape (written, so the envelope restarts, on every such row) |

## 7. PlayAY's output stage (AY → Paula)

Calibrated with our own EMUL register players and confirmed on every ST11/STRC log:

- Paula channels 0–2 = AY tone A, B, C: a 4-byte square wave (`LEN` 2 words) at `$A08300`,
  `PER` = 8 × the AY period (for EMUL; for ST11/STRC the table value ± 8 m directly). A channel is
  off when its tone is off, its volume is 0 or its period is 0.
- Paula `VOL` = `[0, 1, 2, 2, 3, 3, 9, 15, 21, 27, 33, 39, 45, 51, 57, 63][v]`. The envelope bit
  and R11–R13 have no effect at all: a channel with the envelope on plays at its sample volume.
- Paula channel 3 = noise: a noise sample at `$A08304`, `LEN` `$2000` words, `PER` = 124 + 32 × R6,
  `VOL` = the largest Paula volume among the channels that have noise on and v > 0; off when there
  is none. The noise sample's contents were not recorded.
- Clocks: PlayAY's interrupt every 70 940 cc (50 Hz, DeliTracker's default timer). The Spectrum 128
  AY runs at 1.77345 MHz; one AY period unit (16 / 1 773 450 s) equals 8 Paula periods of a 4-byte
  wave (4 · 8 / 3 546 895 s) within 2 ppm.

## 8. The original Z80 player (what differs)

Model variant run against the Z80 player of the agent-x rip with each song compiled to its format:
45 of the 58 songs give equal registers on all of 6 000 interrupts (the rest: section 11). Rules
that differ from PlayAY:

- first pass of a sample: 31 positions (0–30), not 32; then the loop or off as in PlayAY.
- the sample and effect nibbles are applied on every cell, also without a note and on a rest.
- a rest is any byte 1 with bit 7 set (`$80`–`$FF`); h & 7 gives the note name otherwise.
- a note on a channel without any sample reads data that does not exist (not modelled).
- period table: first octave `3832, 3600, 3424, 3200, 3032, 2856, 2696, 2544, 2400, 2272, 2136,
  2016`, octave k = that >> k (96 entries); AY period = table ± m, & `$FFF`; index ≥ 96 → 0 in the
  model (the Z80 player reads past its table: not modelled).
- effects 2–14 all set the envelope (R13 = effect, R11 = parameter, R12 = 0, ornament 0) in the
  model; only 10, 12, 13, 14 are in the files, so 2–9 are unverified for the Z80 player.
- R0–R10 are written at every interrupt; R11–R13 only on rows with an envelope effect.
- the mixer bits of a channel that is off vary (seen: tone A off, B and C on); not modelled.

## 9. Song end, length, loop, fade

- A pass is `delay × patlen × (length + 1)` interrupts (`songs.tsv`); then the song continues at
  position 0 with the channels as they were (no reset).
- PlayAY's song end (seen by the host as `dtg_SongEnd`), 300 s renders: 22 of the 58 ST11 files
  report it at exactly one pass; KOMPLEXS after 2.55 passes, VOJAL after 16.06; the other 34 not at
  all; `strc.megamix1` at one pass while `st11.megamix1` never.
  Synthetic songs: every plain 1–5-position song reports it after one pass (delays 1–6, patlen 32
  and 64); adding one to three stored but unplayed patterns removes it for most delays tried (not for
  delay 2 with three extra); appending 1–256 bytes after the patterns keeps it, appending 576 removes
  it. Repeated runs give the same answer. So the detection depends on bytes outside the played song;
  its rule was not found. **Unverified**; a replayer should report the end after one pass, which is
  what PlayAY does whenever it reports one at the first loop.
- Fade offset and fade length: no fade in any PlayAY log (volumes before PlayAY's song end are those
  of the song; the model has no fade and matches). The fields are probably for the host (DeliAY's
  fade-out at song end), not tested further.
- The song has no length field; the length is the pass above.

## 10. Errata against the RAMSOFT description (`st11fmt.txt`)

- Byte 1 bit 3 is the sharp (+1 semitone), not a flat.
- Note names: high nibble 1 = A … 7 = G and 0 = no note (the description has 0 = A … 7 = G).
- The noise byte's bit 6 (the description's "envelope mask") is the tone mask: set = tone off.
- A rest is high nibble 15 in PlayAY (bit 7 in the Z80 player).
- The effect meanings (section 3.2) are not in the description.

## 11. Verification

| check | result |
|---|---|
| model (PlayAY rules) vs PlayAY Paula state, 58 ST11 files, 15 000 interrupts or to PlayAY's song end | identical on 58 of 58 |
| model on the STRC decode vs PlayAY playing `strc.megamix1` | identical, 5 379 interrupts |
| PlayAY `st11.megamix1` vs `strc.megamix1` | identical Paula state, 5 379 interrupts |
| model (Z80 rules) vs Z80 player, 58 songs, 6 000 interrupts | identical on 45; the others differ on 1–649 interrupts (`songs.tsv`), in the unmodelled cases of section 8 (index ≥ 96, notes without a sample, the mixer of off channels) |
| period table | measured from PlayAY for all 256 indexes, from the Z80 player for 0–95 |

Unverified or not tested: container bytes 8–15 and other channel assignments; a song count above 0;
`patlen` 0 or above 64; `delay` 0; pattern number 0 in a played position; note codes with h 13, 14;
the Z80 player's effects 2–9 and its handling of index ≥ 96; the STRC fields beyond the one file;
the rule of PlayAY's song end; the noise sample's contents.

Files that make good regression rows (PlayAY as reference): `st11.megamix1` and `strc.megamix1`
(same song in both forms; STRC reports the end), `st11.AMIG99` (short, 34.6 s pass), `st11.AXELFOLY`,
`st11.NSZZ` (ornaments reaching indexes ≥ 96), `st11.JMJ` (notes without a sample, rests, song end
at 82.0 s), `st11.OUTFOX` (positions naming missing patterns), `st11.BFAS_2` (16-bit period wrap,
song end at 61.5 s), `st11.TURRICAN` (29 patterns, song end at 225.3 s), `st11.CUPTEA` (patlen 48),
`st11.M_BILL`.
