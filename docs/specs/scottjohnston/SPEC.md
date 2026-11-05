# Scott Johnston ("SoundPlayer", `SJ.*`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the module format played by the UADE/Eagleplayer player "Scott Johnston" (player file name
`ScottJohnston`, version 1.0, adapted by "??? & Eagleeye/DEFECT"; internal name "SoundPlayer by Scott
Johnston"; file prefix `SJ.`). A module is a small score; its sounds are IFF 8SVX files that the
player loads from the `Instruments:` volume by names it keeps in its own table.

This is a behaviour description derived from the original player's reading notes, register-write logs
and execution traces of the 68k reference (the reference 68k player run through the host with
DELI_WRITE_LOG). It contains no code of the original. Cycle numbers in section 9 are the 68000 cycles
that the reference machine charges; 2 cycles = 1 colour clock ("cc").

Status: no real module of this format exists in the corpus (the only `sj` file is a ProTracker module
named `MOD.sj`, which this player rejects). All checks use five synthetic modules built from this
format description (LOGS.md). A private model that follows this text reproduces every register write
of the five reference logs (value, order and absolute colour clock, 6 111 interrupt ticks, about
91 000 writes) with zero differences.

## 1. Module file

### 1.1 Detection

The first 16 bytes are the ASCII text `Scott Johnston 1` (no terminator). Nothing else is checked
(no size check). A file of this format is at least 0x2F bytes long (header and song header).

### 1.2 Layout

All multi-byte values big-endian unless stated.

| offset | size | content |
|---|---|---|
| 0x00 | 16 | magic `Scott Johnston 1` |
| 0x10 | 12 | song name, ASCII, NUL-terminated (the reference shows the bytes from 0x10 up to the first NUL) |
| 0x1C | 16 | instrument slots 1 to 16: one byte each, an index into the player's instrument file name table (2.1), 1-based; 0 = see 2.2 |
| 0x2C | 1 | timer, low byte |
| 0x2D | 1 | timer, high byte (the timer is stored little-endian) |
| 0x2E | 1 | voice mask: bit `i` set = voice `i` (0 to 3) plays this song |
| 0x2F | 12 * n | rows |

If the timer's low byte (0x2C) is 0xFF the song keeps the default timer 0xFFFF (the high byte is then
ignored).

A row is 12 bytes: four 3-byte cells, one per voice, voice 0 first. A cell is `note, instrument,
command`. Rows have no count and no terminator: the song is ended or looped by commands (section 6).
The module has one song, one subsong, no author field and no length; the player never reports a song
end (section 8).

Module info shown by the reference: song name (0x10), number of loaded instrument files, maximum 16
samples, prefix `SJ.`.

### 1.3 Cell

- `note`: 0 = no note; 1 to 39 index the period table (7). Values above 39 read past the table
  (undefined, not specified).
- `instrument`: instrument number used with the note (2.3). Ignored when `note` is 0.
- `command`: 0 = none; see section 6. A cell's command is executed on the row it is read, after
  every voice's note of that row has been set up.

## 2. Instruments

### 2.1 File names

The player holds a table of 95 instrument file names (`data/instrument_names.bin`: 95 entries of 12
bytes, each a NUL-terminated lower-case name padded with zeros; the last byte of an entry is 0 or 0xFF
and has no effect on playback). Slot byte `k` (1 to 95) names entry `k - 1`. The first entries are
`acoustic`, `blocks`, `brass1`, the last `organ2`. Slot bytes above 95 name no valid entry (undefined).

### 2.2 Loading

Before playing, the player loads one file per non-zero slot byte, in slot order: the file
`Instruments:<name>` (the host maps the volume to the `Instruments` folder of the module's directory,
names in any case). A missing file makes the module fail to load.

The 16 instruments are then made from the slots in order: a slot with a non-zero byte uses the file
loaded for it; a slot whose byte is 0 uses the same file as the slot before it (so trailing zero
slots all repeat the last named file). If slot 1 is 0 the reference uses an unrelated memory address
as the file (undefined; such modules must not be produced).

### 2.3 Instrument records

There are 38 instrument records, numbered 1 to 38; records 1 to 16 are filled from the slots, records 17
to 38 stay empty (all zero). An instrument number of 0 also reads an all-zero record. Numbers above 38
are undefined.

Each file is an IFF 8SVX file. The record is filled from it:

- the 4 bytes `VHDR` are searched byte by byte from the file's start, within its first 0x400 bytes; the
  4 bytes after them (the chunk size) are skipped; then `oneShot = u32`, `repeat = u32` (the first two
  fields of the VHDR data);
- the 4 bytes `BODY` are searched the same way within the first 0x800 bytes; the 4 size bytes after
  them are skipped; `data` = the address of the next byte;
- `start = data`; `length = (oneShot >> 1)` cut to 16 bits (words); `loop = data + sext16(oneShot & 0xFFFF)`
  (the low 16 bits of `oneShot` as a signed number); `loopLength = (repeat >> 1)` cut to 16 bits.

Nothing else of the file is used (no volume, no rate, no octaves). A record whose `length` is 0 never
starts a note (5.2): a repeat-only sample (oneShot 0) is silent unless its loop is reached through the
loop refresh of a voice whose DMA is already on.

## 3. Player state

Per voice (0 to 3):
- `start` (address), `pending` (16-bit length in words; non-zero = a note waits to be started),
  `period`, `volume` (0 to 63), `loop` (address), `loopLength` (words), the voice's current track
  (3.1), its instrument record, the `note/instrument/command` of the current cell;
- `hold` (flag, initially off): when set, the voice's loop refresh (5.1) is skipped;
- fade: `fadeDown` (flag), `fadeSpeed` (0 = no fade), `fadeCount`;
- `lock` (flag): set only by the player's stop routine; while set, volume commands are ignored. It is
  off during normal playback.

Global: the row divider `counter` (initially 0), 20 "sync" flags and a "played" flag per instrument
record (both only readable by a game; no effect on sound).

### 3.1 Tracks

A song has 4 tracks, one per voice. All four start at the song's first row; each keeps its own:
`row` (current row), `start` (first row), `loopSave` (row), `loopCount` (0xFF = no loop active),
`delay` (0 = none). Voice `i` always reads cell `i` of its own track's row, so voices can drift apart
(delays and loops are per track).

Two songs exist: song 0, the player's built-in silent song (timer 0xFFFF, mask 0x0F, two rows: an
empty row, then a row whose four commands are 0xDE), and song 1, the module's song.

## 4. Start

### 4.1 Player init (once, after loading)

1. All voice state, the counter and the sync flags are cleared (volumes 0, no fades, no holds, nothing
   pending, no loops).
2. Song 0 is set up and the voice mask 0x0F applied: DMACON writes 0x0001, 0x0002, 0x0004, 0x0008 in
   this order (each a separate write), then song 0's tracks are reset and voices 0 to 3 are put on
   song 0's tracks.
3. One tick body runs (5.1 to 5.3): four loop refreshes with the empty word (5.1), no notes, and the
   counter goes from 0 to 5.
4. DMACON 0x800F (with the DMA wait of 9.2).
5. The timer is set to 0xFFFF.

### 4.2 Song start

1. Song 1's tracks are reset: `row = start = first row` (offset 0x2F; internally the row pointer is
   kept one row before and advanced before each read, which is equivalent), `loopCount = 0xFF`,
   `delay = 0`.
2. For each voice `i` whose bit is set in the song's voice mask: the voice is put on song 1's track
   `i`, its volume is set to 0 and its `lock` cleared. Voices whose bit is clear keep playing song 0
   (silently: its rows have no notes, and its second row restarts it).
3. If the timer low byte is not 0xFF, the timer is set to the song's timer value.

No register is written at song start. The counter keeps the value 5 from 4.1, so the first row is
read on the 5th interrupt tick.

## 5. Tick

Each interrupt runs these phases in order.

### 5.1 Loop refresh and fades (voices 0, 1, 2, 3)

For each voice:
1. Unless `hold` is set: if `loopLength` is non-zero, write AUDxLC = `loop` and AUDxLEN = `loopLength`;
   otherwise write AUDxLC = the empty word (a 4-byte zero area of the player's memory) and AUDxLEN = 1.
   This happens on every tick for every voice, playing or not.
2. Fade: if `fadeSpeed` is non-zero, `fadeCount` is decremented (8-bit); when it reaches 0 it is reloaded
   with `fadeSpeed` and the volume steps:
   - fading down: if the volume is 0 the fade stops (`fadeSpeed = 0`, no write); else volume - 1 and
     write AUDxVOL;
   - fading up: if the volume is 63 the fade stops; else volume + 1 and write AUDxVOL.

### 5.2 Note starts (voices 0, 1, 2, 3)

For each voice with `pending` non-zero: write AUDxLC = `start`, AUDxLEN = `pending`, AUDxVOL = `volume`,
AUDxPER = `period`, then DMACON = 0x8000 | (1 << voice) (with the DMA wait, 9.2); then `pending = 0` and
the instrument's played flag is set.

A note read on a row is therefore started on the next tick, after that tick's loop refresh has already
written the new instrument's loop (or the empty word). The loop refresh of the tick after that makes
Paula continue with the loop.

### 5.3 Row divider

If `counter` is 0 it is set to 5 and the tick ends here. Otherwise it is decremented; if the result is
not 0 the tick ends here. When it reaches 0, the row is processed (5.4). So a row is read every 6
ticks (the speed is fixed).

### 5.4 Row

1. Advance (voices 0 to 3, each on its own track): if the track's `delay` is non-zero it is decremented,
   and if it is still non-zero the track does not advance. Otherwise (delay was 0, or just reached 0)
   the track moves to its next row.
2. Read (voices 0 to 3): if the voice's track's `delay` is 0, the voice takes `note`, `instrument` and
   `command` from its cell of the track's current row. A voice whose track is delayed reads nothing (and
   has no note or command this row).
3. Notes (voices 0 to 3): for each voice with a non-zero note: write DMACON = 1 << voice (DMA off; no
   wait), then `period = periodTable[note - 1]`, take instrument record `instrument` and copy its
   `start`, `length` (into `pending`), `loop`, `loopLength` into the voice; the voice's note is then
   cleared. The volume is not changed.
4. Commands (voices 0 to 3): execute the voice's command (section 6), then clear it.

A note with no instrument sample (length 0) turns the voice's DMA off and nothing starts; the voice
stays silent but keeps refreshing that instrument's loop registers.

## 6. Commands

Ranges are inclusive. `v` = the voice, `t` = the voice's track.

| command | effect |
|---|---|
| 0x00 | none |
| 0x01 | audio filter (LED) off |
| 0x02 | audio filter (LED) on |
| 0x03 to 0x42 | unless `lock`: volume = command - 3 (0 to 63), stored and written at once to AUDxVOL (a direct write, see 9.4) |
| 0x43 | DMA off: write DMACON = 1 << v for voices 1, 2, 3. On voice 0 the player writes DMACON = 0x0002 (voice 1 off; voice 0 keeps playing) |
| 0x44 to 0x56 | none |
| 0x57 to 0x88 | row delay: if `t.delay` is 0, `t.delay = command - 0x56` (1 to 50). The track then stays on this row for `delay - 1` more rows (a delay of 1 has no effect); while it waits the voice reads no cells |
| 0x89 to 0xA6 | none |
| 0xA7 to 0xB0 | fade up: `fadeDown` off, `fadeSpeed = fadeCount = command - 0xA6` (1 to 10) |
| 0xB1 to 0xBA | fade down: `fadeDown` on, `fadeSpeed = fadeCount = command - 0xB0` (1 to 10) |
| 0xBB to 0xCE | set sync flag `command - 0xBB` (no sound effect) |
| 0xCF | `hold` on (stop the loop refresh of this voice) |
| 0xD0 | `hold` off |
| 0xD1 | clear all 20 sync flags (no sound effect) |
| 0xD2 to 0xDB | loop start: if `t.loopCount` is 0xFF: `t.loopSave = t.row` (this row), `t.loopCount = command - 0xD1` (1 to 10); otherwise nothing |
| 0xDC | loop end: if `t.loopCount` is 0: `t.loopCount = 0xFF` (loop done, go on); otherwise `t.loopCount - 1` and `t.row = t.loopSave` (the next advance moves to the row after the loop start row) |
| 0xDD | `t.row` moves back one row: the next advance comes back to this same row, so this row is read again on every row tick, for ever (a "stop here" marker; its note and other commands repeat) |
| 0xDE | `t.row = t.start`: the next advance reads the song's first row again (song loop) |
| 0xDF to 0xE4 | none |
| 0xE5 to 0xF8 | clear sync flag `command - 0xE5` (no sound effect) |
| 0xF9 to 0xFF | none |

So with loop start `0xD1 + n` on row R and loop end on row E (same voice column), the rows R to E are
played once, then the rows R + 1 to E are played `n` more times (the jump back lands on R and the next
advance moves on to R + 1; R itself is not repeated). Loop start and end act on the track of the voice
whose cell holds them; a loop start while a loop is active on that track is ignored.

The fade commands do not write anything themselves; the fade steps from the next tick on (5.1). The
volume of a fade is the voice's stored volume, which a volume command also sets.

## 7. Period table

Note 1 to 39:

1076, 1016, 960, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339,
320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113

(the first three are not a semitone ladder with the rest: there is no 906 between 960 and 856).

## 8. Song end

The player never signals a song end: 0xDE loops the song, 0xDD stops the track on a row for ever, and a
song without either runs its tracks into the bytes after the module data (undefined). The reference
plays until the host stops it. A port should report no end (or let the host's time limit apply).

## 9. Timing

### 9.1 Ticks

The interrupt is called every `5 * (timer + 1)` colour clocks, exactly (70 940 cc for timer 14187,
55 005 for 11000, 35 005 for 7000, 327 680 for the default 0xFFFF; checked over all ticks of the five
logs). The first interrupt call comes one period after the end of the song start call (4.2), with
the song's timer. Times below are from the call of the interrupt (its colour clock in the reference is
not logged; LOGS.md lists the first one per log).

### 9.2 Clock model

Keep a counter `T` of cycles and a base clock `B`. At the interrupt call: `B` = call clock, `T = 0`.
Costs are added to `T` in the order of section 9.3. Writes happen in two ways:

- **through a host helper** (every write except the volume command's): add the helper's cost before the
  call (9.3, table H) to `T`; the write is stamped `B + floor(T / 2)` (both words of an AUDxLC write get
  the same stamp); then the clock is re-based: `B` = that stamp, `T = 0` (an odd half cycle is
  dropped); then add the helper's cost after the call. For a DMACON write with bit 15 set the host
  also waits for the audio DMA before the re-base: the clock moves to the start of the 10th raster line
  after the one of the stamp, `B = (floor(stamp / 227) + 10) * 227`, `T = 0`.
- **direct** (volume command): the write is stamped `B + floor(T / 2)` with the cost of the write
  included as listed; no re-base.

The filter commands also call a host helper (no register write): add its before-cost, re-base, add its
after-cost.

### 9.3 Costs (cycles)

Table H, host helpers:

| helper | before | after |
|---|---|---|
| AUDxLC (both words) | 84 | 60 |
| AUDxLEN | 116 | 68 |
| AUDxVOL | 84 | 60 |
| AUDxPER | 92 | 60 |
| DMACON (on or off) | 94 | 68 |
| filter (01, 02) | 74 | 68 |

Tick, in order (each "write X" below is a helper call as in 9.2 with its before and after costs):

Entry: 246.

Phase 5.1, for each voice: 18 for voice 0 and 34 for voices 1 to 3, then 39; then
- hold on: 16;
- hold off, with loop: 21 + 46, write LC, 30, write LEN, 28;
- hold off, no loop: 21 + 34, write LC (empty word), 22, write LEN (1), 28.

Then the fade: 21; then
- no fade: 16;
- fade, counter not yet 0: 37 + 16;
- counter reaches 0: 37 + 41, then down: 21, up: 25; then at the limit (stop): 32; otherwise
  58, write VOL, 38.

Between phases: 34.

Phase 5.2, for each voice: 42 for voice 0 and 34 for voices 1 to 3, then 21; then
- nothing pending: 24;
- note start: 62, write LC, 30, write LEN, 30, write VOL, 30, write PER, 46, write DMACON (on, with
  the wait), 72.

Phase 5.3: 108. No row: the tick ends (no more writes).

Row (5.4): 18, then
- advance, each voice: 63, then delay 0: 48; delay counted down and still non-zero: 37 + 16; delay
  counted down to 0: 37 + 48;
- 34, then read, each voice: 67, plus 82 if the voice reads its cell, then 16;
- 66, then notes, each voice: 47, then no note: 24; note: 30, write DMACON (off, no wait), 248;
- 46, then commands, each voice: 114, then the command:

| command | cycles |
|---|---|
| 0x00, 0x44 to 0x56, 0x89 to 0xA6, 0xFE, 0xFF | 16 |
| 0xDF to 0xE4, 0xF9 to 0xFD | 26 |
| 0x01, 0x02 | 102, then the filter helper's re-base (no write), then 84 |
| 0x03 to 0x42 | 31, then (not locked) 28 and the direct AUDxVOL write stamped here, then 16; (locked) 16 |
| 0x43 | 10, then for each of the four voices k = 0 to 3 in order: 23, and for k = v: 34, write DMACON (off), 12; then, once after the four steps, 16 |
| 0x57 to 0x88 | 39, plus 12 if the delay is set, then 16 |
| 0xA7 to 0xBA | 74 |
| 0xBB to 0xCE, 0xE5 to 0xF8 | 68 |
| 0xCF, 0xD0 | 42 |
| 0xD1 | 192 |
| 0xD2 to 0xDB | 35, plus 48 if the loop starts, then 16 |
| 0xDC | 31, then 32 (loop done) or 60 (jump back) |
| 0xDD | 58 |
| 0xDE | 54 |

For 0x01/0x02 the 102 is the command's own 28 plus the filter helper's before-cost 74, and the 84 is
the helper's after-cost 68 plus 16.

For 0x43 on voice 0 the DMACON write (value 0x0002) happens in the k = 0 step.

After the commands the tick ends (no more writes).

### 9.4 Resulting gaps

These follow from 9.3 and are what the logs show (cc):

| from | to | gap |
|---|---|---|
| interrupt call | first write (voice 0 loop refresh, no hold): empty word / loop | 221 / 227 |
| LC | LEN (refresh with loop / empty word / note start) | 103 / 99 / 103 |
| refresh LEN | next voice's refresh LC (with loop / empty word), no fade | 178 / 172 |
| refresh LEN | next voice's refresh LC with loop, fade counting | 197 |
| note start LEN, VOL, PER | next write of the start | 91, 91, 100 |
| DMACON on | next voice's note start LC | 170 cc after the line start (210 if one idle voice lies between) |
| note DMACON off | next voice's DMACON off (adjacent voice / one voice between) | 243 / 279 |
| volume commands of adjacent voices | | 94 or 95 |

### 9.5 Player init and song start timing

Player init (4.1) is a host-level event; its writes in the reference: the four DMACON off writes 129 cc
apart; 999 cc after the fourth, the loop refresh of the four voices with the gaps of a tick (99 and 172);
433 cc after the last AUDxLEN, DMACON 0x800F followed by the DMA wait. The time from the start of the
player init to the first DMACON write depends on loading the instruments (about 2 600 to 2 900 cc in
the logs). The song start call (4.2) lasts 1 148 cc when it sets a timer and 897 cc when the song keeps
the default timer; the first interrupt follows one period after its end (9.1).

## 10. Host notes

- Filter: 0x01 sets the LED off (filter off), 0x02 sets it on; the reference starts with the LED on.
  Nothing is written to Paula for these.
- Sample addresses written to AUDxLC are guest addresses: the instruments' data inside the loaded files,
  and the player's empty word. LOGS.md gives the reference's addresses.
- Host volume, mute and balance settings do not change the logged values.
