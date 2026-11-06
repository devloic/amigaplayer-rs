# MultiMedia Sound (`mms.*`, `sfx20.*`, `SFX2.*`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: modules played by the Eagleplayer "MultiMedia Sound" (the replay routine of mms.library 6.4,
1991-93, by the authors of SoundFX), UADE player file `MultiMedia_Sound`, prefixes `mms` and `sfx20`.
The module format is the SoundFX 2.0 format (mark `SO31`, 31 samples), the same files the
"SoundFX 2.0" Eagleplayer accepts; the replay is a different, later engine (see README for the
differences that matter against the existing SoundFX 2.0 port).

This is a behaviour description derived from register-write logs and execution traces of the
reference 68k player run through the host with DELI_WRITE_LOG. It contains no code of the original.
Cycle numbers are the 68000 cycles the reference machine charges (2 cycles = 1 colour clock, "cc");
they are static (the same every time, whatever the data and whichever way a test goes).

Status (checked by a private model that follows only this text, against the reference):

| item | result |
|---|---|
| 20 corpus runs (13 modules, 7 extra subsongs), about 427 000 register writes | address, value, order and absolute colour clock of every write equal the reference (given the tick start of each log, LOGS.md) |
| 12 synthetic runs (7 modules, 5 extra subsongs) exercising every command, every special note word, sample numbers 16..31, zero-length samples, subsong splitting and its abort, both LED values, speed traps | same, about 30 800 writes |

## 1. File format

All values big-endian. Offsets from the start of the file.

| offset | size | meaning |
|---|---|---|
| 0x000 | 31 x u32 | byte length of samples 1..31 (0 = no sample) |
| 0x07C | 4 | mark `SO31` |
| 0x080 | u16 | tempo: the CIA timer value of the replay interrupt (section 4.1) |
| 0x082 | 14 | not used by this player |
| 0x090 | 31 x 30 | sample records (1.2) |
| 0x432 | u8 | song length `N` (number of order entries used) |
| 0x433 | u8 | filter byte (3.2) |
| 0x434 | 128 x u8 | order list: pattern number per position |
| 0x4B4 | 4 | not used |
| 0x4B8 | | patterns, 1024 bytes each, pattern `p` at `0x4B8 + 1024 * p` |
| `S` | | sample data, samples 1..31 one after the other in that order, each its length from the table at 0 (zero-length samples take no space) |

`S = 0x4B8 + 1024 * (M + 1)`, where `M` is the highest pattern number among the first `N` order
entries (entries `N..127` are ignored even if larger). Sample `i` starts at `S` plus the lengths of
samples `1..i-1`.

### 1.1 Detection

All of these hold:

- each of the 31 lengths at offset 0 is even and at most 0x20000 (unsigned);
- the 4 bytes at 0x7C are `SO31`;
- the u16 at 0x80 is not 0.

The rule is the same as the SoundFX 2.0 rule; the two players accept the same files.

After detection, the player refuses the module ("module too short") if `S` plus the sum of all 31
lengths is larger than the file.

### 1.2 Sample record (30 bytes at `0x90 + 30 * (i - 1)` for sample `i`)

| off | size | meaning |
|---|---|---|
| 0x00 | 22 | name (ASCII, zero padded) |
| 0x16 | u16 | length in words (the value written to AUDxLEN when a note starts) |
| 0x18 | u16 | volume |
| 0x1A | u16 | loop start in bytes from the sample start |
| 0x1C | u16 | loop length in words |

None of these is checked or corrected: a length different from the table at 0, a volume above 64,
a loop past the end are all used as they are.

### 1.3 Pattern data

A pattern is 64 rows of 16 bytes; a row holds 4 entries of 4 bytes, voices 0..3 in that order.
Entry: `W` (u16), `B2` (u8), `B3` (u8).

- `W` in 0x0000..0x7FFF: a period word. Bit 12 is bit 4 of the sample number; the period is
  `W & 0xEFFF`. `W = 0` = no note.
- `W` with bit 15 set: a special word. Defined: 0xFFFD (`PIC`), 0xFFFE (`STP`), 0xFFFC (`BRK`),
  0xFFFB. Any other value with bit 15 set behaves like a note whose period is `W & 0xEFFF` but skips
  the sample/volume part (5.2).
- `B2` high nibble: low 4 bits of the sample number (sample number = nibble + 16 if bit 12 of `W`;
  0 = no sample change).
- `B2` low nibble: command; `B3`: its parameter.

### 1.4 Module information

- No title. Sample names at 1.2; a host may look for an author among them.
- Length: `N` positions. Patterns: `M + 1`. Samples: the number of non-zero lengths.
- Subsongs: section 3.3.

## 2. Tables in the player

Period table (66 words, the order of the searches): 1076, 1016, 960, 906, 856, 808, 762, 720, 678, 640,
604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202,
190, 180, 170, 160, 151, 143, 135, 127, 120, 113, followed by 26 more words 113 (66 in all), then the
terminator 0xFFFF. In front of the first entry lie 20 words 1076, so that an index up to 20 below
the first entry reads 1076. Index `k` below means "the `k`-th word from the first entry"; the
searches start at `k = 0` and run until the terminator. (Exact bytes: `data/period_table.bin`, the
20 pad words, the 66 entries and the terminator, 87 words.)

Vibrato table (32 unsigned bytes): 0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244,
250, 253, 255, 253, 250, 244, 235, 224, 212, 197, 180, 161, 141, 120, 97, 74, 49, 24
(`data/vibrato_sine.bin`).

## 3. Initialisation

### 3.1 Player init (once per module load)

1. Sample addresses: for each sample `i` (1..31), its start address as in section 1. If its length
   (table at 0) is non-zero, the first 4 bytes of its data are set to zero in memory (this is
   permanent; a zero-length sample's address is computed but its bytes are not touched).
2. Subsong table (3.3).

### 3.2 Song init (each start of a subsong)

1. The host writes AUDxVOL = 0 for voices 0, 1, 2, 3, twice (its volume and balance calls go through
   the volume stage of 5.4 with a stored volume of 0). The logs contain these 8 writes before the
   first tick; their timing is the host's.
2. Filter byte (offset 0x433): 0 -> CIA-A port A bit 1 cleared (filter on); 0xFF -> bit 1 set
   (filter off); any other value -> the byte in the module is set to 0 and bit 1 is cleared. The log
   shows it as a write to 0xBFE001 (value 0x0000 or 0x0002 in the corpus and synthetic logs).
3. All voice state is zero (5.1) except the voice's DMA bit (1, 2, 4, 8 for voices 0..3).
4. The interrupt rate is set from the tempo word (4.1).
5. `speed = 6`, `timer = 0`, `pos = start of the subsong`, `row offset = 0`, `break = no`.
   The end of the subsong (`end`) is taken from the subsong table.

Nothing is written to Paula by steps 3 to 5.

### 3.3 Subsongs

The subsong table holds (`start`, `end`) position pairs; `end` is exclusive. The split is skipped
(one subsong, `0..N`) when any of these holds: the module size computed in 1.1 is exactly 95 688
bytes (one specific corpus module, "Working In The Mine"), the tempo word is at most 0x1BB6, or
`N = 1`.

Otherwise positions `p = 1, 2, ...` are visited while `p < N`:

- A pattern is "empty-or-stopped" when each of its 256 entries, read as one u32, is 0 or 0xFFFE0000.
- If the pattern at `p` is not empty-or-stopped: go on with `p + 1`.
- If it is, and `p + 1 = N`: stop scanning.
- If it is, and the pattern at `p + 1` is entirely zero (here 0xFFFE0000 counts as content): stop
  scanning and abort (below).
- Otherwise: the current subsong ends at `p` (exclusive), a new subsong starts at `p + 1`, and the
  scan continues at `p + 2`.

The first subsong starts at 0, the last one ends at `N`. Count = number of splits + 1.

Abort: the end of the first subsong is overwritten with `N`, but the count still includes the splits
found before the abort and their (`start`, `end`) entries stay as they were; the last of them has no
end written (0). Such a subsong runs past `N`: its position keeps increasing (5.6) and reads order
bytes and patterns beyond the list (synthetic log `syn_twoempty.s1`; a port may treat it as it likes,
see README).

Corpus: LeonardoSong 5 subsongs (0-21, 22-24, 25-38, 39-48, 49-52), Clown-O-Mania 2 (0-9, 10-27),
Pace Of Pleasure 2 (0-15, 16-21), all other corpus modules 1.

## 4. The tick

### 4.1 Interrupt rate

The replay routine runs once per interrupt of a CIA timer loaded with the tempo word `t`: ticks are
`(t + 1) * 5` colour clocks apart, exactly (for example 14 187 -> 70 940 cc, 13 437 -> 67 190 cc).

### 4.2 Speed

Every tick: `timer = timer + 1` (16 bits). If `timer == speed`: `timer = 0` and the tick is a row
tick (section 5). Otherwise it is an effect tick (section 6).

So the first row is played on the 6th tick, then every `speed` ticks. The speed can only change on
effect ticks (command F). The test is an equality, so a speed at or below the current `timer` is not
reached until `timer` wraps around (65 536 ticks later); in practice that happens only with a
speed of 1 (6.7).

## 5. Row tick

### 5.1 Voice state

Per voice: the current entry (`W`, `B2`, `B3`, reloaded every row), `start` (sample address), `len`
(words), `period`, `volume`, `loop` (address), `looplen` (words), `step` (signed 16-bit), `stepcur`,
`target`, `vibparam` (byte), `vibpos` (byte). All zero at song init; addresses 0 mean address 0.

### 5.2 Per voice, voices 0, 1, 2, 3 in that order

The entry of the voice is the 4 bytes at `pattern(order[pos]) + row offset + 4 * voice`.

1. Load the entry into the voice state.
2. Sample and volume part, only when `W` is not 0xFFFD and bit 15 of `W` is clear (this includes
   `W = 0`):
   - `vibpos = 0`.
   - `v = volume`. If the sample number `s` is not 0: from sample `s`: `start` = its address, `len` =
     record length, `volume` = record volume, `loop = start + record loop start`, `looplen` = record
     loop length; `v` = record volume.
   - Command C: `v = B3`. Command 5: `v = v + B3`, at most 64. Command 6: `v = v - B3`, at least 0
     (16-bit, a negative result becomes 0). Other commands: `v` unchanged.
   - Write AUDxVOL through the volume stage (5.4); `volume` = the value written.
3. Special words and notes:
   - `W = 0xFFFD`: `B2` and `B3` of the stored entry are cleared (no effect on the following
     effect ticks). Nothing else.
   - `W = 0xFFFB`: the stored `W` is cleared; nothing else (the command in `B2` stays and runs on the
     effect ticks).
   - `W = 0xFFFC`: `break = yes`; the stored `W` is cleared (the command stays, as for 0xFFFB).
   - `W = 0`: nothing.
   - Any other `W` (a period, 0xFFFE, or another bit-15 word): `step = 0`; `period = W & 0xEFFF`;
     write DMACON = the voice's bit (DMA off for this voice). Then:
     - `W = 0xFFFE` (`STP`): `volume = 0`; write AUDxVOL = 0 (directly, not through the volume
       stage).
     - otherwise: write AUDxLC = `start` (one 32-bit store: the high and the low word carry the same
       time), AUDxLEN = `len`, AUDxPER = `W & 0xEFFF`.
     - In both cases the voice's bit is added to the row's DMA mask.

   A note without a sample number uses the `start`/`len` of the last sample set on that voice
   (address 0, length 0 if none yet). `STP` keeps the voice in the DMA mask: it is switched on again
   at volume 0 (with the loop of 5.3).

### 5.3 After the four voices

1. Raster wait (8.6).
2. Write DMACON = 0x8000 | DMA mask (always, also when the mask is 0).
3. Raster wait.
4. For voices 3, 2, 1, 0: write AUDxLC = `loop` (32-bit store), AUDxLEN = `looplen`. This happens for
   every voice on every row, whatever the row did.

### 5.4 Volume stage

The value `v` is masked with 0x7F, multiplied by the channel gain and shifted right by 6 (gain: left
for voices 0 and 3, right for voices 1 and 2; 64 each by default, so the stage is `v & 0x7F`). A voice
switched off by the host writes 0. The stored `volume` is the written value (so with a gain below 64
the stored volume shrinks on every row without a sample number). All logs use the defaults.

### 5.5 Rows and positions

After 5.3:

- If `break`: row offset = 0x3F0 (the last row), `break = no`.
- Row offset += 16. If it reaches 1024: row offset = 0, `pos = pos + 1`; if `pos == end` of the
  subsong: `pos = start` of the subsong and the song end is reported to the host (the replay goes on).

So `BRK` makes the next row the first row of the next position; it does not end the current row.
A `BRK` on the last row of a pattern changes nothing.

### 5.6 Song end

The song end is the row tick on which the position wraps (5.5). The reference host stops a few ticks
after it; the logs end there.

## 6. Effect tick

For voices 0, 1, 2, 3 in that order:

### 6.1 Step

If `step` is not 0: `stepcur = stepcur + step`; `period = stepcur`.

- `step` positive (bit 15 clear): if `target` is greater than `period` (unsigned), keep going;
  otherwise `step = 0` and `period = target`.
- `step` negative: if `target` is less than `period` (signed), keep going; otherwise `step = 0` and
  `period = target`.

Write AUDxPER = `period`; `stepcur = period`. Nothing else for this voice on this tick (the command
is not looked at while a step runs). A step therefore runs across rows (only a note or another
period-carrying word, 5.2 step 3, clears it) until it reaches its target.

### 6.2 Commands

When `step` is 0, the command in `B2 & 15` of the stored entry runs:

| cmd | action on effect ticks |
|---|---|
| 0 | nothing |
| 1 | arpeggio (6.3) |
| 2 | pitch bend (6.4) |
| 4 | vibrato (6.5) |
| 7 | step set-up, towards lower notes (6.6) |
| 8 | step set-up, towards higher notes (6.6) |
| F | speed (6.7) |
| 3, 5, 6, 9..E | nothing (5, 6 and C act on the row, 5.2) |

### 6.3 Arpeggio (1)

By `timer` (the tick counter after its increment, 1 .. speed-1):

- `timer` = 3: write AUDxPER = `period`.
- otherwise `n` = `B3 >> 4` when `timer` is 1 or 5 and above, `n` = `B3 & 15` when `timer` is 2 or 4.
  Search the period table from index 0 for `period`; at the first match at index `k` write AUDxPER =
  table[`k + n`]. If the terminator is reached first, nothing is written.

`period` is not changed.

### 6.4 Pitch bend (2)

`hi = B3 >> 4`, `lo = B3 & 15`. If `hi` is not 0: `d = +hi`; else if `lo` is not 0: `d = -lo`; else
nothing. With `p = period`: `q = ((p & 0xEFFF) + d) & 0xFFFF`; write AUDxPER = `q`; `period = q | (p &
0x1000)`. (Bit 12 of `period` is kept across the bend; it is never set in practice.) A positive `d`
lowers the pitch.

### 6.5 Vibrato (4)

If `B3` is not 0: `vibparam = B3`. `x` = vibrato table[`(vibpos >> 2) & 31`]; `delta = (x * (vibparam &
15)) >> 7`. If `vibpos` is 0x80 or more: write AUDxPER = `period - delta`, else `period + delta` (16-bit).
`vibpos = vibpos + ((vibparam >> 4) * 4)` (8-bit wrap). `period` is not changed. `vibparam` is
kept across rows and notes (only song init clears it); `vibpos` is cleared by 5.2 step 2 on every row
whose `W` is not negative.

### 6.6 Step set-up (7 and 8)

`stepcur = period`. `step = B3 & 15` for command 7, `-(B3 & 15)` for command 8. `k = B3 >> 4`.
Search the period table from index 0 for `period`; at the first match at index `i`: `target` =
table[`i - k`] for command 7 (a larger period, reaching into the 20 pad words of 1076 if needed),
table[`i + k`] for command 8. Not found: `target = period`. Nothing is written on this tick.

So a step command spends its first effect tick in set-up, then steps by `B3 & 15` per tick (the
direction comes from the command, the stop test from 6.1) until `target`. When the target is
reached while the command is still the current one, the next effect tick sets up a new step from
the new period: the slide continues by another `k` notes. A target equal to the period (`k = 0`, or
a period not in the table) makes set-up and a single write of the period alternate.

### 6.7 Speed (F)

`speed = B3 & 0x1F`; 0 becomes 1. It runs on every effect tick of the row. A result of 2 or more
takes effect at once (`timer` is 1 on the first effect tick). A result of 1 is not met by `timer`
again until it wraps: the next 65 535 ticks (`timer` 2 .. 65 535, then 0) are all effect ticks of
the same row (its steps and commands keep running; no row is played), then the song goes on at
speed 1 for ever (with speed 1 there are no effect ticks, so command F never runs again).
(Synthetic log `syn_speed1`: the row with F01 plays, then nothing for the rest of the log.)

## 7. Write order summary

Row tick, per voice 0..3: [AUDxVOL] then [DMACON off, AUDxLC (2 words), AUDxLEN, AUDxPER] or
[DMACON off, AUDxVOL = 0]; then DMACON on; then AUDxLC/AUDxLEN for voices 3, 2, 1, 0.

Effect tick, per voice 0..3: at most one AUDxPER.

## 8. Timing

### 8.1 Clock model

Tick `j` starts at `E(j) = E0 + j * (t + 1) * 5` cc, the moment the host enters the player's interrupt
routine (`E0` is the host's business; LOGS.md lists it per log). Within the tick keep a cycle counter
`u` starting at 0. A write (or a beam read, 8.6) is stamped `E + floor(u / 2)`, where `u` already
includes the cost of the step that makes the write; the tables give the cycles to add before each
write and after it. The beam line at clock `c` is `floor(c / 227)` (log clock); only its changes matter.

### 8.2 Tick head

| step | cycles |
|---|---|
| entry up to the speed decision | 415 |
| row tick: row head | 194 |
| effect tick: effect head | 24 |
| every voice slot (both kinds): before the voice's own work | 18 |
| every voice slot: after it | 34 |

### 8.3 Row voice (5.2)

In order, add:

| step | cycles |
|---|---|
| entry fetch | 98 |
| `W` is not 0xFFFD: sign test | 19 |
| sample/volume part (only if it runs): head | 71, plus 8 if bit 12 of `W` is set, then 27 |
| sample number not 0 | 176, then 97 if the record's loop start is not 0, else 87 |
| command C | 55 |
| command 5 | 93, plus 14 if clamped to 64 |
| command 6 | 106, plus 14 if clamped to 0 |
| any other command | 73 |
| volume write | `46 + VCH(v) + 133` up to and including the AUDxVOL write, `VCH = 23, 46, 69, 92` for voices 0..3 |
| after the volume write | 50 (voices 0..2) or 40 (voice 3), then 12 |

Then the special-word part:

| case | cycles |
|---|---|
| 0xFFFD | 79, end |
| 0xFFFB | 80, end |
| 0xFFFC | 121, end |
| `W = 0` | 112, end |
| note / STP / other: DMACON-off write | 156 (including the write) |
| STP | 53 to the AUDxVOL = 0 write, then 10 |
| note | 45 to the AUDxLC write, 20 to AUDxLEN, 28 to AUDxPER, then `SA(v)` = 145, 176, 207, 215 for voices 0..3 |
| note and STP, after the above | 68, end |

"End" closes the voice; the slot's 34 (8.2) follow.

### 8.4 After the voices of a row

| step | cycles |
|---|---|
| to the first raster wait | 30 |
| raster wait | 8.6 |
| to the DMACON-on write | 24 (including the write) |
| to the second raster wait | 18 |
| raster wait | 8.6 |
| loop writes head | 24 |
| per voice 3, 2, 1, 0: AUDxLC write | 24 |
| AUDxLEN write | 20 |
| to the next voice | 34 |

What follows (position bookkeeping, song-end report) writes nothing.

### 8.5 Effect voice (section 6)

| step | cycles |
|---|---|
| head | 21 |
| step running | 74; 32 more if the target is reached; 12 to the AUDxPER write; 28, end |
| no step, command 0 | 49, end |
| no step, commands 3, 5, 6, 9..E | 98, end |
| no step, commands 1, 2, 4, 7, 8, F | 102, then the command (below), then 16, end |

Commands (cumulative from the end of the 102):

| command | cycles |
|---|---|
| arpeggio | 43; plus 40 if `timer` is at most 4. `timer` = 3: 24 to the write, then 16. Otherwise: 40 (`n` from the high nibble) or 34 (low nibble), then 24, then per table entry examined: 31, and then: terminator -> 16, end; else 17, and then: match -> 12 to the write, 16, end; no match -> 18 and the next entry |
| pitch bend | 43; `hi` not 0: 80 to the write; else `lo` not 0: 117 to the write; both 0: 53, end. After the write 32 |
| vibrato | 25; plus 12 if `B3` is not 0; 175; 14 (adding) or 4 (subtracting); 12 to the write; 62 |
| speed | 33; plus 4 if the result was 0; 32 |
| step set-up | 4 (command 7) or 15 (command 8); 69; plus 4 for command 8; 62; then per table entry examined: 27, and then: terminator -> 28, end; else 13, and then: match -> 17, plus 4 for command 7, then 42, end; no match -> 18 and the next entry |

### 8.6 Raster wait

Polling the beam line, waiting for 9 changes of it. Started at `u`:

```
u = u + 28
repeat 9 times:
    u = u + 16;  L = line at u
    loop:
        u = u + 16
        if line at u differs from L: leave the loop
        u = u + 9
    u = u + 19
u = u + 44
```

"line at u" is `floor((E + floor(u / 2)) / 227)`. The wait lasts between about 8 and 9 lines; the
exact length depends on where the tick lies against the line grid, which changes from tick to tick
because the tick spacing is not a multiple of 227 cc. A port that starts its ticks at another phase
gets other wait lengths (the DMA-off to DMA-on gap and the time of the loop writes move by up to one
line), with the same values and order.

### 8.7 Worked example

ElefantPolka, tick 6 (the first row), voice 0 plays sample 1 (loop start 0) with no command:
415 + 194 + 18 + 98 + 19 + 71 + 27 + 176 + 87 + 73 + 46 + 23 + 133 = 1 380 cycles: AUDxVOL at `E +
690` (log: tick call 410 428, write at 411 118).

## 9. How to check

`tools/replay-cmp.py REF.log PORT.log` (in the repo) compares logs tick by tick (tick-relative times).
The logs in `logs/` are the references (LOGS.md), each with its `E0` and tick spacing, so a model that
starts its ticks at `E0` can compare absolute clocks too, as the validation did.
