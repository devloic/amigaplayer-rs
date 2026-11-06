# Protracker4 (ProTracker 4 IFF modules, prefix `mod3.`): behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Eagleplayer "Protracker4" ("Protracker 4.x Eagleplayer V1.0 (18-Apr-97)", described by
its author as an experimental player), file prefixes `mod3.` and `ptm.`. This is a description of
the observed behaviour of the reference 68k player run through the host with DELI_WRITE_LOG
(write logs, interrupt entry times) and of the module data, in my own words. "cc" = Amiga colour
clocks (1 cc = 2 CPU cycles of the 7.09 MHz 68000); "cycles" = CPU cycles.

Summary: a `mod3.` file is an IFF `FORM` of type `MODL` (the file format of ProTracker 3.6x/4.0)
that wraps an ordinary 31-instrument ProTracker module (`M.K.`, or `8CHN` for eight voices) in a
`PTDT` chunk, with an `INFO` chunk that carries the song name, the initial tempo and a timing-mode
flag. The replay is a ProTracker 2.x engine. Its register values and their order are the same as
the repo's ProTracker 2.3D core (`src/protracker.rs`) except for the list in section 6. A value
model written from this spec reproduces every (register, value) write in order, tick for tick, for
all 14 reference runs (section 7).

The `ptm.` prefix does not mean a different format. The player accepts only the `FORM`/`MODL`
container (2.1). PolyTracker `ptm.` files (the repo's PolyTracker port) are rejected by it and
must stay with that port.

## 1. File format

All values are big-endian. The layout as ProTracker 4 writes it (the only real module, MOD3.heaVen,
is this way):

| offset | content |
|---|---|
| 0 | `FORM`, long size = file length - 8 |
| 8 | `MODL` |
| 12 | `VERS` chunk: id, a size long that is 0x16, then 10 bytes (two zero words, then `PT4.00`) |
| 30 | `INFO` chunk: id, size long = 72 (8 header bytes included), 64 data bytes |
| ... | `CMNT` chunk: id, size long (8 header bytes included), text (author line, comments) |
| ... | `PTDT` chunk: id, size long = data length (header not included), then a complete ProTracker module |

The chunk sizes are not consistent: VERS says 0x16 for 18 bytes of chunk, INFO and CMNT count their
own 8-byte header, PTDT does not. The player handles this as follows (2.2), and a port must do the
same to locate the chunks exactly as the reference does.

INFO data (offsets from the first data byte, i.e. chunk start + 8):

| data offset | size | meaning |
|---|---|---|
| 0 | 32 bytes | song name, zero padded (shown as the title) |
| 0x20 | word | number of instruments (31 written; not used for replay) |
| 0x22 | word | number of positions (display only; the replay uses the PTDT song length) |
| 0x24 | word | number of patterns (not used) |
| 0x26 | word | volume (0x40 seen; not used) |
| 0x28 | word | initial tempo in BPM; 0 means 125 |
| 0x2A | byte | flags; bit 0 set = "CIA timing": an Fxx with xx >= 0x20 sets the tempo. Clear = every non-zero Fxx sets the speed |
| 0x2B.. | bytes | date and time, play time (display only) |

PTDT data: a standard ProTracker module. Song name 20 bytes, 31 sample headers of 30 bytes
(name 22, length in words, finetune byte, volume byte, repeat start in words, repeat length in words)
at data offset 20, song length byte at 950, restart byte 951 (ignored), 128 order bytes at 952,
signature at 1080 (`8CHN` selects 8 voices, any other value 4 voices; it is not checked
otherwise), pattern data at 1084: 64 rows of (voices × 4 bytes) per pattern. The number of
patterns stored is (highest order entry among all 128 entries) + 1. Only entries 0..127 count
(a byte with bit 7 set is ignored). The sample data follows the patterns, in sample order,
2 × length bytes each.

### 1.1 Subsongs, title, length

One subsong. Title = INFO song name (the host falls back to the file name when it is empty).
Duration is computed by the player for display only (not specified).

## 2. Detection and loading

### 2.1 Check (what the Eagleplayer accepts)

On the file bytes: long 0 equals `FORM`, the long at 4 is not greater than the file length (signed
comparison), long 8 equals `MODL`. Nothing else is checked, so any IFF MODL passes. A file that
fails the initialisation below plays silence in the reference.

### 2.2 Chunk walk (initialisation)

- The chunk at 12 must be `VERS`, otherwise initialisation fails.
- If the VERS size long equals 0x16, the walk starts at offset 12 + 0x12 = 30 (the INFO chunk of
  a ProTracker 4 file). Otherwise the walk starts at the VERS chunk itself.
- To find a chunk, the player starts at the walk start and compares the 4-byte id there with the
  wanted one. On a mismatch it advances by the size long of the chunk it is on (no 8 added, no
  padding). It gives up when the position is no longer below (file start + FORM size).
- INFO and PTDT are each searched from the walk start. If either is missing, initialisation fails.
  The reference then stays silent: its tick still runs at 50 Hz on an empty state, and every
  volume write is 0. Synthetic test s3vers, a VERS of correct size 10, gives this. A port should
  reject such a file or play silence.
- PTDT data starts 8 bytes after the PTDT id.

### 2.3 Initial state

- Voices: 4, or 8 when the PTDT signature is `8CHN`.
- Speed 6, tick counter 0, position 0, row 0, pattern = order[0], break row 0, no flags.
  Pattern-delay counters are 0. The fine-slide mask is **0** (see 4.4 "1xx/2xx").
- All voice state is zero (including the "previous cell" long, sample pointers, period, volume).
- Voice i has DMA bit 1 << i.
- DMA off for all voices: one write of DMACON 0x000F (4 voices). With 8 voices the host's
  amplifier gets a DMA-off for each of the 8 voices.
- The CIA timer is set from the INFO tempo, also when the flags bit 0 is clear: timer value =
  1773447 / tempo (integer division; 1789773 on NTSC). The interrupt period is
  5 × (timer value + 1) cc: 70940 at 125, 43255 at 205, 98525 at 90.

## 3. Host interface and register writes

The player never touches Paula itself. Every write goes through the Eagleplayer host's
DMA-mask/poke-address/poke-length/poke-period/poke-volume functions. In this host they write Paula
directly with 4 voices, and the mixing amplifier's virtual voices with 8 (logged at 0xD00000 |
voice << 4 | register). So:

- **LOC** is a pair of writes (high, low word) at the same clock.
- **LEN**: a length value of 0 or 1 is written as **0** (any length <= 1, unsigned). This matters:
  ProTracker's one-shot samples have repeat length 1, so the repeat writes LEN = 0, which Paula
  plays as 65536 words from the repeat start.
- **DMACON** (4 voices): value = (mask & 0xF) | (0x8000 if switching on). With mask 0 and on, the
  value 0x8000 is still written. After every DMA-on (even with mask 0) the host waits for the
  audio DMA: the clock is set to the start of the raster line 10 lines later, i.e.
  t = (floor(t / 227) + 10) × 227.
- **VOL**: the value is stored as the voice's "last written volume". If the host's voice mask has
  this voice switched off, 0 is written instead. For voices 4..7 the mask bits of voices 0..3 are
  used. The host default is all on.
- The player never touches the LED filter (E0x does nothing, 4.5). The filter stays as the host
  leaves it, off in this host.

## 4. Replay

### 4.1 Voice state

Per voice: current cell (the 4 pattern bytes, kept as one long), sample start, length (words),
repeat start, repeat length (words), wave start (funk pointer), period, finetune, volume, DMA bit,
tone-portamento target / speed / direction, vibrato command byte and position, tremolo command byte
and position, wave-control byte (low nibble vibrato waveform, high nibble tremolo waveform),
glissando/funk byte (low nibble glissando, high nibble funk speed), funk accumulator, loop row and
loop counter, sample-offset memory.

Global: speed, tick counter, tempo, CIA flag (INFO bit 0), song position, row, break row, a
position-jump flag, a loop-jump flag, a pattern-delay request and a pattern-delay counter, the
fine-slide mask.

### 4.2 Per tick (one call per CIA interrupt)

1. counter += 1.
2. If counter >= speed: counter = 0, then
   - if the pattern-delay counter is non-zero: run the effects of every voice (4.4) as on an
     effect tick (with counter 0),
   - else play a new row (4.3);
   - then advance the row (4.6).
3. Else: run the effects of every voice (4.4).
4. While the position-jump flag is set: go to the next position (4.6).

The first row is therefore played on the 6th interrupt.

### 4.3 New row

For each voice in order 0..n-1: process the voice's cell (below), then write VOL = the voice volume.
After all voices: DMACON on with the collected DMA bits, the host's DMA wait, then for each voice in
order 0..n-1: LOC = repeat start, LEN = repeat length. The order is ascending, unlike PT 2.3D's
descending order.

Processing a cell:
1. If the voice's previous cell long is 0 (an entirely empty cell last row), write PER = period.
2. Read the new cell. Instrument = (byte 0 & 0xF0) | (byte 2 >> 4). Note = word 0 & 0xFFF.
   Command = byte 2 & 0xF, parameter = byte 3.
3. Instrument non-zero: from its header, sample start, length, finetune, volume (no clamping). If
   the repeat start is non-zero: repeat start = sample start + 2 × repeat start, length = repeat
   start + repeat length (words), repeat length as stored. Otherwise repeat start = sample start,
   repeat length as stored, length as stored. Wave start = repeat start.
4. Note = 0: "more effects" (below), done.
5. Note non-zero:
   - E5x: finetune = x, then go on;
   - command 3 or 5: set up tone portamento (4.4, 3xx), then "more effects", done;
   - command 9: "more effects" first (applies the sample offset), then go on;
   - period lookup: the first index i (0..36) of the finetune-0 period row with note >= row[i]; if
     none, i = 37. Period = row[finetune][i] (index 37 of a row is the first entry of the next one);
   - EDx: "more effects", done (no trigger now);
   - trigger: DMACON off (voice bit); clear the vibrato position unless wave-control bit 2 is set,
     the tremolo position unless bit 6 is set; LEN = length, LOC = sample start; if the sample start
     is 0 (no instrument ever): repeat start = 0, repeat length = 1, LEN = 1 (written as 0); PER =
     period; add the voice bit to the DMA set; then "more effects".

"More effects" (row tick): by command:
- 9: sample offset: if parameter non-zero it is remembered. With o = memory × 128 (words): if
  o < length then length -= o and sample start += 2 × o, else length = 1. The function runs
  twice for a 9xx with a note (before and after the trigger), as in PT 2.3D.
- B: position jump (4.6). D: pattern break (4.6). F: speed/tempo (4.5). C: volume = min(param, 64).
- E: the E command (4.5).
- anything else (0-8, A): write PER = period.

### 4.4 Effects of a voice (effect ticks, and all ticks of a delayed row)

1. Funk update (EFx): if the funk speed is non-zero, add funk table[speed] to the accumulator; if
   bit 7 becomes set, clear it and advance the wave start by one byte (back to the repeat start when
   it reaches repeat start + 2 × repeat length), then replace that sample byte b by 255 - b
   (in sample memory).
2. If (cell & 0xFFF) == 0 (no command, no parameter): only the VOL write (step 4).
3. By command:
   - 0 arpeggio: k = counter mod 3. k = 0: PER = period. Otherwise n = parameter's high nibble
     (k = 1) or low nibble (k = 2). Search the voice's finetune row for the first entry the period
     is >= to (37 entries). Write PER = the word n entries further on. Past the end of the last
     row the words are the player's own variables: the voice count (4 or 8), then the high and
     low words of voice 0's current cell, then voice 0's state. If no entry matches, nothing is
     written.
   - 1xx: period -= (xx & mask), then mask = 0xFF. If (period & 0xFFF) < 113, period =
     (period & 0xF000) | 113. PER = period & 0xFFF. The mask starts at **0**, so the first 1xx or
     2xx of a song (unless an E1x/E2x came first) does not move the pitch.
   - 2xx: the same with +, upper limit 856.
   - 3xx: if xx non-zero, speed = xx and the cell's parameter byte is cleared. If there is no
     target, nothing is written. Otherwise step the period by the speed towards the target and
     stop at it (the target becomes 0 when reached). Write PER = period, or with glissando
     (low nibble of the glissando byte) the first finetune-row entry the period is >= to (index 35
     if none).
     Setup on a row with a note: index = the first finetune-row entry the note is >= to (35 if
     none); minus one if the finetune is negative (8..15) and the index > 0. Target = that entry.
     Target == period: target = 0. Target < period: direction down, else up.
   - 4xy: non-zero nibbles replace the vibrato command's nibbles (x speed, y depth). Then the
     vibrato value at position (pos >> 2) & 31 for the waveform (low two bits of wave control):
     0 sine table; 1 ramp: (p × 8), or 255 - (p × 8) when the position byte is negative; 2 or 3:
     255. Delta = (value × depth) >> 7. PER = period + delta (period - delta when the position byte
     is negative). The stored period is unchanged. Position += speed × 4 (byte wrap).
   - 5xy: tone portamento without taking a new speed, then volume slide.
   - 6xy: vibrato without new parameters, then volume slide.
   - E: the E command (4.5).
   - any other command (7, 8, 9, A, B, C, D, F): first PER = period, then
     - 7xy tremolo: as vibrato on the tremolo state, but the ramp's sign test uses the
       **vibrato** position (PT's bug), delta = (value × depth) >> 6. VOL = volume ± delta clamped to
       0..64, written here instead of step 4. The stored volume is unchanged.
     - Axy: volume slide: x non-zero: volume += x, at most 64; else volume -= y, at least 0.
4. VOL = volume (not after tremolo).

Volume slides change the volume that step 4 writes.

### 4.5 E commands and F

E commands run on row ticks (from "more effects") and on every effect tick. "Tick 0" means counter
= 0. That is the row tick, and also every tick-0 pass of a delayed row.

- E0x: nothing (no filter change).
- E1x / E2x: on tick 0 only, the mask becomes 0x0F and a 1xx/2xx step is done (writes PER).
- E3x: glissando = x. E4x: vibrato waveform = x (low nibble of wave control). E7x: tremolo
  waveform = x (high nibble). E5x: finetune = x.
- E6x (tick 0 only): x = 0: loop row = current row. Else if the loop counter is non-zero:
  decrement it, and if it is still non-zero jump. If the counter is 0: set it to x and jump.
  "Jump" = break row = loop row and set the loop-jump flag.
- E8x (every tick): Karplus-Strong on the repeat part in sample memory: for each of the first
  2 × repeat length - 1 bytes, b[i] = (b[i] + b[i+1]) >> 1 (signed bytes, arithmetic shift). The
  last byte becomes (b[last] + b[0]) >> 1, using the already changed b[0]. No register write.
- E9x: x = 0: nothing. On tick 0 with a note in the cell: nothing (the row already triggered).
  Otherwise, when counter mod x = 0: retrigger.
- EAx / EBx (tick 0 only): volume += x (max 64) / -= x (min 0).
- ECx: when counter == x, volume = 0 (written by the following VOL).
- EDx: when counter == x and the cell has a note: retrigger.
- EEx (tick 0, and only if no pattern delay is running): pattern-delay request = x + 1.
- EFx (tick 0 only): funk speed = x; if x is non-zero, do one funk update now.
- Retrigger writes: DMACON off (voice bit), LOC = sample start, LEN = length, PER = period, DMACON on
  (voice bit) with the host's DMA wait, LOC = repeat start, LEN = repeat length, **PER = period**.
  PT 2.3D does not write the last PER.

F (row tick only): parameter 0: nothing (**no song end**, unlike PT 2.3D). If the CIA flag is
set and the parameter is >= 0x20: tempo = parameter and the CIA timer is set as in 2.3. The new
period starts with the next interrupt after the one in progress: the interval that is running
keeps the old length. Otherwise speed = parameter and counter = 0.

### 4.6 Row advance, positions, song end

After a row tick (also a delayed one):
1. row += 1.
2. If a pattern-delay request is pending: counter = request, request = 0.
3. If the pattern-delay counter is non-zero: decrement it; if still non-zero, row -= 1.
4. If the loop-jump flag is set: clear it, row = break row, break row = 0.
5. If row == 64: next position.

Next position: row = break row, break row = 0, clear the position-jump flag, position =
(position + 1) & 0x7F. If position >= song length: position = 0 and report song end. Then the
pattern of the new position is taken.

- Bxx (row tick): song end is reported when xx < position, or when xx == position and position ==
  (the byte at offset 0x3B6 **of the whole file**, minus 1, 8-bit). The player reads the song-length
  offset from the file start instead of the PTDT data. In a ProTracker 4 file that byte is inside
  the PTDT sample headers, so "jump to itself on the last position" practically never reports
  an end (synthetic m04 loops forever in the reference). Then position = xx - 1 (16-bit, so 0
  gives 0xFFFF and the next position is 0), break row = 0, set the position-jump flag.
- Dxy (row tick): v = 10x + y. If v > 64, break row = 0, else break row = v. Set the position-jump
  flag. v = 64 is accepted as a row number (not observed).
- The position-jump flag is acted on at the end of the same tick (4.2 step 4). B and D on one row
  combine as in ProTracker.
- Song end is only a report to the host. The player itself keeps playing. The host stops after
  it (m06 ends at 71.8 s, as with the built-in engine).

## 5. Timing

### 5.1 Tick period

The interrupt period is 5 × (floor(1773447 / tempo) + 1) cc (2.3). A tempo change takes effect
one interrupt later (4.5). The first interrupt follows the initialisation after a host-dependent
delay (about 72 000 cc in this host).

### 5.2 Write spacing

All times are code costs of the player between two host calls. The host calls themselves take no
time, except the DMA wait after DMA-on (section 3). The costs are deterministic. Give them in CPU
cycles and convert to cc by halving, flooring the absolute clock at each write. That is why the
measured gaps alternate by 1 cc for odd cycle counts. Measured values (cycles):

Effect tick, 4 voices:
- interrupt entry to the first write: 732 when voice 0 writes only VOL. 790 when voice 0 writes
  PER first for a command 9, A or C. 938 for a VOL-only voice 0 with a funk speed set.
- VOL to VOL of the next voice, both "no command": 600 (806 when the next voice has a funk speed).
- VOL to PER of a voice with a plain command (7..D, F; "PER = period" then the effect): 660
  (866 with funk). PER to VOL: 342 for C/9/F/8. For A: 438 when sliding up, 446 when sliding down.
- vibrato: VOL to PER 1052 (positive position) / 1042 (negative), PER to VOL 358 (sine waveform).
- last VOL to the end of the interrupt: 404.

Row tick, 4 voices, per voice:
- previous-cell-empty PER: 368 after the previous voice's VOL, 696 after the interrupt entry.
- no note, plain command: PER (from "more effects") 446 after the previous-cell PER, or 618
  after the previous voice's VOL when the previous cell was not empty. Then VOL 392 later.
- note trigger: DMACON off comes 1021 + 41 × i cycles after the previous-cell PER (i = the period
  lookup index of 4.3), +62 cycles when the instrument has a non-zero repeat start, and 1195 + 41
  × i after the previous voice's VOL. Then LEN 294, LOC 182, PER 210, "more effects" PER 378,
  VOL 392 (VOL 644 after the trigger PER for C).
- after the last voice's VOL: DMACON on 306. Then the DMA wait to line start + 10 lines. Then the
  first repeat LOC 260 after the wait's end, LEN 204 after LOC (208 when written as 0), next LOC
  208 after LEN. The end of the interrupt comes 454 after the last LEN.
- tone-portamento setup: +44/45 cycles per index of its lookup.

The voice loop does not wait anywhere between a voice's DMA off and the DMA on. So the time
between them is only code time: about 900 cc for the last voice, more for earlier voices. That
is shorter than PT 2.3D's 1515-cc wait for the last voices, and the emulated Paula
restarts a note only when its current word has ended. The reference behaves this way and a port
with the same timing will too.

The full measured table is in logs/gap-table.tsv (section 7), keyed by the role of the previous
and the current write, command, tags (vibrato waveform and sign, slide direction, tone-portamento
direction, sample-offset fit, LEN written as 0 or not), instrument present, repeat start non-zero,
funk active. Each key gives a constant in cycles plus a per-lookup-index slope. 8-voice timing was
not tabulated.

## 6. Differences from the repo's ProTracker 2.3D core

On MOD3.heaVen the reference and the built-in engine (playing the extracted PTDT module) write
the same values per voice in the same order on every one of 4915 ticks. The cross-voice order of
the repeat block differs, and so do LEN = 0 versus 1 and the tick rate. On the other modules the
differences are:

1. Container, chunk walk and INFO fields (sections 1-2); `8CHN` = 8 voices through the host's
   amplifier.
2. Initial tempo from INFO (125 if 0); the CIA timer is set at start.
3. INFO flags bit 0 clear: Fxx >= 0x20 sets the speed, not the tempo.
4. F00 is ignored (PT 2.3D stops the song).
5. LEN values <= 1 are written as 0. The repeat block after DMA-on is in ascending voice order.
6. Retrigger (E9x/EDx) writes a final PER after the repeat LOC/LEN.
7. The fine-slide mask starts at 0: the song's first 1xx/2xx (unless preceded by E1x/E2x) does
   not slide. Seen in 6 of the 7 test modules.
8. E0x does not touch the filter. E8x changes sample memory every tick.
9. Song end: wrap past the song length, or a backward B. "B to itself" uses the wrong length byte.
   Dxx accepts 64.
10. Timing: no DMA wait in the player. The host waits 10 lines after DMA-on only. Costs per
    section 5.
11. Arpeggio reads past the period table into the player's own variables (not PT's memory).

## 7. Validation

A value model written from sections 2-4 was run against the reference logs. Every logged tick
was compared as an ordered list of (register, value), with sample addresses taken relative to
the PTDT data. Module notes (all from the module corpus, wrapped synthetically except heaVen):
see LOGS.md.

| run | ticks | identical |
|---|---|---|
| MOD3.heaVen (real PT4 file, 60 s) | 4920 | 4920 |
| m01..m07 wrapped M.K. modules (120 s or to song end) | 39 204 | 39 204 |
| s1vbl (flags bit 0 clear), s2t0 (tempo 0), s4t90 (tempo 90) | 8160 | 8160 |
| s5ch8 (8CHN, 8 virtual voices) | 3930 | 3930 |

Timing: the gap table (5.2) applied to the 6 modules it was built from puts 127 529 of 127 764
writes within 1 cc of the reference. On the 2 modules left out (m04, m06), 32 093 of 39 760 are
within 1 cc and 7 512 had a key combination the table had not seen. Timing is therefore specified
for the common paths only. See README.
