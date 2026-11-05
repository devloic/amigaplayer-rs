# MIDI - Loriciel: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the music format of Loriciel's games (intro music of "Entity", 1993) as played by the
reference, the Eagleplayer "MIDI - Loriciel" (Wanted Team adaptation V1.0, file name prefix `midi`).
The music is a Standard MIDI File plus a separate sample bank; four Paula voices play one-shot
samples. This is a description of observed behaviour (register-write logs, beam-read traces, module
data) in my own words. "cc" = Amiga colour clocks; "cycles" = CPU cycles of the reference host
(1 cc = 2 cycles).

Status: a model that follows this text reproduces the reference's register writes (address, value,
order and colour-clock stamp, from the start-up writes on, including song end and the ticks after
it) exactly for 7 runs of 6 synthetic module pairs, 7 769 writes (LOGS.md). No real module of the format
was available.

## 1. Files

A module is two files in one directory:

- the song: a Standard MIDI File, named `MIDI.<name>` (or `<name>.mid`);
- the sample bank: `SMPL.<name>` for a song named `MIDI.<name>` (the first four characters are
  replaced by `SMPL`), or `<name>.BSP` for a song named `<name>.mid` (the three suffix letters are
  replaced by `BSP`). Both name tests ignore case. Any other song name makes the second load fail.

### 1.1 Detection (song file)

All of: bytes 0..3 = `MThd`; the 32-bit big-endian value at 4 = 6; the 16-bit format at 8 is 0 or 1
(unsigned); the track count at 10 is not 0; the division at 12 is not 0; bytes 14..17 = `MTrk`.
Nothing else is checked.

### 1.2 Song file contents used

- Track count `N` = word at 10. Division word at 12: if its top bit is clear, `tbase = division *
  10000`; if set (SMPTE), `tbase = 192 * 10000` (the SMPTE fields are ignored).
- Tracks follow at 14: each is `MTrk`, a 32-bit length, then the event data. Every track must start
  with `MTrk` (else the module is refused as corrupt). The track lengths are used only to find the
  next track; playback does not stop at a track's end, only at its end-of-track meta event.
- Song size = offset of the end of the last track; it must not exceed the file size (else "module
  too short").
- No title, author, subsongs or length: one song, numbered 0.

### 1.3 Sample bank

Must start with `BNKS` (else corrupt). All values big-endian.

| offset | size | meaning |
|---|---|---|
| 0 | 4 | `BNKS`; replaced by four zero bytes when loaded (this 4-byte zero block is the "silence" sample) |
| 4 | 4 | `P`, number of programs |
| 8 | 4 * P | offset (from the bank start) of each program record, program 0 first |

Program record: a 16-bit region count `R` (must be at least 1), then `R` regions of 8 bytes:

| byte | meaning |
|---|---|
| 0..3 | sample header reference: negative = minus its offset from the bank start |
| 4 | base note (signed byte) |
| 5 | unused |
| 6 | upper key (signed byte): the region serves notes up to and including this one |
| 7 | unused |

Sample header (8 bytes): 0..3 sample data reference (negative = minus its offset from the bank
start), 4..7 sample length in bytes (only the low 16 bits are used for playback). Sample data are
signed 8-bit. Several regions may share a header.

On load every negative reference is converted to an address (bank address plus offset) in place; a
header is converted once, when the first region that points at it is converted. Positive references
are taken as addresses already converted and left alone (none occur in files).

Samples size = (data address of the header converted last) + (its 32-bit length) - bank address; it
must not exceed the bank file size (else "module too short"). Load size = both file sizes.

## 2. State

- Per track `t` (in file order): `pos` (read position), `active` (set), `count` (a 32-bit signed tick
  counter, initially "unset"), `chan` (current MIDI channel, 0..15).
- `remaining` = number of active tracks (initially `N`).
- 16 MIDI channels: `program` (initially the bank's program 0) and `link` (the voice last started on
  this channel, initially none).
- 4 voice slots (Paula voices 0..3): `owner` (a MIDI channel or free; initially free) and `note`.
- `tbase` (1.2); the timer latch `L` (section 6).

## 3. Tick

The host calls the player once per timer period (section 6). One tick:

1. If `remaining` = 0: song end (3.5); then the tail (3.4); the tick ends.
2. Walk the track slots in order. An inactive track is passed over. Stop after `remaining` active
   tracks (the value at the start of the tick) have been handled. For an active track:
   - If `count` is unset: set it to 0 and go to "read delta".
   - Else `count = count - 4`. If `count > 0` the track is done for this tick. Otherwise (zero or
     negative) go to "event".
   - Read delta: read a variable-length quantity (MIDI rules: 7 bits per byte, most significant
     first, top bit = more bytes follow). If it is not 0: `count = count + delta`, the track is done.
     If it is 0: go to "event".
   - Event: run one event (section 4) at `pos`. If the event ended the track, the track is done; else
     go to "read delta".
   So the track's clock advances 4 MIDI ticks per player tick, a delta's remainder carries over, an
   event whose delta is `d` after the previous one fires `ceil(d / 4)` player ticks later (counting
   the carried remainder), and events with delta 0 fire in the same tick in file order.
3. The tail (3.4).

### 3.4 Tail

For each voice 0..3 whose slot has an owner: write `x.LC` = bank address (the silence block), then
`x.LEN` = 2. Because Paula fetches the new pointer only when the current sample run ends, a sample
started this tick plays once to its end and the voice then loops over 4 zero bytes. A voice keeps
an owner (and gets these two writes every tick) until a matching note-off (4.4) frees it.

### 3.5 Song end

Tell the host the song has ended; parse the song header and tracks again (all tracks active, counts
unset, read positions at the track starts, `remaining = N`); then the reset (5.2), which also sets
the tempo to 500 000 microseconds per quarter. Channel programs return to program 0, links and voice
owners are cleared. The bank is not converted again. The next tick plays the song from the start.

## 4. Events

There is no running status. The status byte `s` is read:

- `s >= 0xF0`: `s = 0xF0` is a SysEx: read a variable-length length, skip that many bytes. `s = 0xFF`
  is a meta event (4.1). Any other value: skip 2 bytes.
- `s < 0xF0`: `chan = s & 15` is stored in the track, then by `s & 0xF0`:
  - 0x90: note number, velocity; velocity not 0: note-on (4.3); 0: note-off (4.4).
  - 0x80: note number, velocity (ignored): note-off (4.4).
  - 0xC0: program byte `p`: the channel's `program` becomes the bank's program `p` (no range check:
    `p >= P` reads past the offset list, undefined).
  - anything else, including data bytes (0x00..0x7F) met where a status is expected and the
    one-data-byte message 0xD0: skip 2 bytes. (0xA0, 0xB0, 0xE0 are therefore skipped correctly; 0xD0
    and a running-status data byte desynchronise the stream as described.)

### 4.1 Meta events

Read the type byte, then:

- 0x2F end of track: skip 1 byte (the length); the track becomes inactive, `remaining - 1`.
- 0x51 tempo: skip 1 byte (the length), read 3 bytes `us` (big-endian); set the tempo (6.1).
- 0x20 channel prefix: skip 1 byte, read one byte, sign-extend it, store it as the track's `chan`.
  It has no audible effect: every channel event stores its own channel first.
- Any other type: read a variable-length length and skip that many bytes.

The fixed-length types ignore the length byte (they always take 1, 1 + 3 and 1 + 1 bytes).

### 4.3 Note-on (channel `c`, note `n`, velocity `v`)

1. Voice: the first free slot (0..3). If none is free: the highest-numbered slot whose owner is `c`;
   if none, slot 0. The slot gets `owner = c`, `note = n`; channel `c`'s `link` = this slot.
2. Region: in channel `c`'s program, the first region with `n <= upper key` (signed compare). If no
   region qualifies, the 8 bytes after the last region are used as a region (undefined).
3. Writes, voice `x`: `x.LC` = the header's data address; `x.LEN` = (low 16 bits of the length) / 2
   (rounded down); `x.PER` = `period[n - base]` (6.3); `x.VOL` = the volume of 4.5; `DMACON = 0x8000 |
   (1 << x)`; then the wait (7.3).

DMA is not switched off first: a stolen voice that is already running picks up the new sample when
its current run (normally the silence loop) ends.

### 4.4 Note-off (channel `c`, note `n`)

Only channel `c`'s `link` slot is considered. It is freed when it exists, its owner is `c` and its
note is `n`: `owner` = free, `link` = none, then `DMACON = 1 << x` and the wait (7.3). Otherwise
nothing happens (a voice that was stolen, or an older note of the same channel, stays owned).

### 4.5 Volume

`i = v >> 1` (0..63); `k = i` for `i < 16`, `16 + (i - 16) / 2` for `16 <= i < 32`, `24 + (i - 32)
/ 4` for `i >= 32` (integer division); `vol = 2 * k` (0..62). The written value is `(vol & 0x7F) *
level / 64`, `level` = the host's level for the voice's side (voices 0 and 3 left, 1 and 2 right:
balance * master / 64, 64 by default, which leaves `vol` unchanged). A voice muted by the host writes
0. All reference logs use the defaults.

## 5. Start and reset

### 5.1 Start

The start call: clear the host-visible voice info, parse the song (as in 3.5), then the reset (5.2).

### 5.2 Reset

Write `VOL0..VOL3 = 0` (0xDFF0A8, B8, C8, D8 in this order), `DMACON = 0x000F`, `ADKCON = 0x00FF`;
free all four slots; every channel: `program` = program 0, `link` = none; set the tempo to 500 000
(6.1).

### 5.3 Stop

On stop the reference writes `VOL0..VOL3 = 0` and `DMACON = 0x000F` (not in the logs).

## 6. Tempo and timer

### 6.1 Tempo set

With `us` in microseconds per quarter note (16-bit unsigned divisions, as the original computes):

1. `q = us / 100`; if `q > 65535`, `q = us & 0xFFFF` instead.
2. `r = tbase / q`; if `r > 65535`, `r = tbase & 0xFFFF` instead (`q = 0` is not handled).
3. `f = r >> 2` (player ticks per second); `L = 716000 / f`; if that exceeds 65535, `L = 716000 &
   0xFFFF = 60640` (`f = 0` is not handled).
4. Hand `L` to the host as the new timer latch.

Example: division 96, tempo 500 000: `q = 5000`, `r = 192`, `f = 48`, `L = 14916`.

### 6.2 Timer

The host's timer period is `(L + 1) * 5` cc. The first tick comes one period (with the `L` set by the
start call) after the start call ends; then every period. A latch handed over during a tick takes
effect one tick later: the interval that follows the tick that set it still has the old length, the
next one the new length. (So a tempo event changes the spacing from the tick after next.) Example:
`L = 14916` gives 74 585 cc.

### 6.3 Period table

`period[d]` for `d = n - base` from -29 to +30 (values outside this range read unrelated memory):

d = -29..-1: `1991, 1882, 1772, 1678, 1584, 1491, 1408, 1329, 1254, 1184, 1117, 1055, 995, 940,
887, 837, 790, 746, 704, 664, 627, 592, 559, 527, 498, 470, 443, 419, 395`

d = 0..30: `373, 352, 332, 314, 296, 279, 264, 249, 235, 222, 209, 198, 186, 176, 166, 157, 148, 140,
132, 124, 117, 111, 105, 99, 93, 88, 83, 78, 74, 70, 66`

## 7. Timing

### 7.1 Clock rule

For a call made by the host at clock `C`, keep a cycle count `T` from 0. Each step below adds its
cost; a register write (or beam read) gets the stamp `C + floor(T / 2)`, where `T` already includes
the cost of the step that makes it. A host service call (song-end notice, timer latch, file load,
cache flush, ...) rounds `T` down to an even number and then adds 16. Costs do not depend on data
except where stated. A long write to `x.LC` gives two word writes (high, low) with the same stamp.

### 7.2 Tick ledger (cycles)

Entry 273. If `remaining` = 0: song end (7.8). Else 4, then the tracks:

- inactive track slot: 37;
- active track: 68; then
  - `count` unset: 24, then read delta;
  - else 33 (count - 4); if the new count is not 0: + 9; if it is positive: + 10 and the track is done;
    else event.
- read delta: 4, the quantity (V), 13; delta not 0: + 50, done; 0: event.
- event: 26 + event cost (7.4) + 17; if the track ended: done; else 10 and read delta.
- done: 18.

V (a variable-length quantity of `b` bytes): `53 * b + 44 * (b - 1) + 12`.

Tail: 42; per voice 0..3: 21; owned: 24 -> `x.LC`, 16 -> `x.LEN`; then 26.

### 7.3 Wait

28; then 9 times: { 16, read the line `L0`; repeat { 16, read the line; 9; stop if it differs from
`L0` }; 10 }; then 44. "Read the line" = `floor(stamp / 227)` with the stamp of 7.1 (lines of 227 cc
counted from clock 0 of the run; only the change matters). Each wait therefore ends shortly after the
ninth line boundary, at a time that depends on the beam phase.

### 7.4 Event costs (cycles)

Matching against a list: per candidate examined: 21 + 37; the matching candidate + 8; a candidate that
does not match + 18; when the list is exhausted + 21.

- Read the status: 25.
- `s >= 0xF0`: 8; match `s` against the list (0xF0, 0xFF). Not found: 24.
  - SysEx: 4 + V + 24.
  - Meta: 16; match the type against the list (0, 1, 2, 3, 4, 5, 6, 7, 0x20, 0x2F, 0x51, 0x54, 0x58,
    0x59, 0x7F) in this order. End of track: 56. Tempo: 80, tempo set (7.7), 16. Channel prefix: 48.
    Any other type (in the list or not found): 4 + V + 24.
- `s < 0xF0`: 36; match `s & 0xF0` against (0x90, 0x80, 0xC0). Not found: 24.
  - 0x90: 21; velocity not 0: 78 + 186 + note-on (7.5) + 148 + 16; velocity 0: as 0x80 below.
  - 0x80: 78 + 186 + note-off (7.6) + 148 + 16.
  - 0xC0: 66 + 176 + 138 + 148 + 16.

### 7.5 Note-on (voice `x`)

`sel(x)` = 43, 74, 105, 113 for voices 0..3.

1. 18 + 24; per slot examined (in order, stopping at the first free one): 17; a slot that is not free
   + 31, + 4 more when its owner is the channel; when all four are owned: + 4. Then 118.
2. 58; per region examined: 21; a region passed over (note above its upper key) + 18.
3. 70 -> `x.LC`; 32 -> `x.LEN`; `sel(x)` + 78; 50 -> `x.PER`; `sel(x)` + 64 + 16.
4. 82 + 31 + 23 * min(x, 2) + 103, + 10 for voices 0..2; 30 -> `x.VOL`; 16 + 18 + `sel(x)` + 36 + 16.
5. 74 -> `DMACON`; 18; wait (7.3); 16.

### 7.6 Note-off

18 + 65; if the channel has a link: + 21; if that slot's owner is the channel: + 29. Not freed (any
test failed): 20 + 13 + 16. Freed: 72 + 13 + 70 -> `DMACON`; 18; wait (7.3); 16.

### 7.7 Tempo set

558, host service call (7.1), 60.

### 7.8 Song end at a tick

After the entry (273): 90, host service call (song end), 78, parse (7.9), 18, reset (7.10), 10, then
the tail (7.2; no voice is owned, so 42 + 4 * (21 + 26)).

### 7.9 Parse

97; division bit clear: + 88, set: + 28; 28; per track 293; 20.

### 7.10 Reset

176; then six writes of 16 each: `VOL0`, `VOL1`, `VOL2`, `VOL3`, `DMACON`, `ADKCON`; 24 + 4 * 54 + 70 +
16 * 62 + 194; tempo set (7.7) with 500 000; 16.

### 7.11 Start-up

The host makes these calls one after the other, the first at clock 100; each call lasts
`floor(T / 2)` cc of its own cycle count `T`, and the next starts when it ends:

1. detection (1.1): 180;
2. second-file load: 60, host service call, 62 + 17 * (D + 1) + 21 * (F + 1) + 235, host service call
   (the load), where `D` = length of the directory path the host hands over (with its trailing
   separator) and `F` = length of the song file name (this is the cost for a name starting with
   `MIDI.` in capitals; the other spellings take other paths, not measured);
3. init: 36, host service call, 78, parse (7.9), 116, host service call, 91, bank conversion, 16, 117,
   host service call; bank conversion = 60 + per program 54 + per region 39 (+ 51 when the region's
   reference is converted, + 30 more when that converts its header);
4. structure query: 24;
5. volume: 240; per voice 0..3: 26, then the volume write of 4.5 with value 0 (its 7.5 step 4 costs:
   31 + 23 * min(x, 2) + 103, + 10 for voices 0..2, 30 -> `x.VOL`, 16), then 18; finally 16;
6. balance: the same as 5 (four more writes of 0);
7. start (5.1): 1178, parse (7.9), 55, host service call (cache flush), 10, reset (7.10).

The first tick is one timer period (6.2) after the end of call 7.

In the reference logs (directory length 95, name `MIDI.m1`, one track, the bank of LOGS.md): the
volume writes are at 2558, 2686, 2826, 2961 and 3206, 3334, 3474, 3609; the reset writes at 4622 to
4662 (8 cc apart); the start call ends at 5735 and the first tick is called at 80 320.

## 8. Edge cases (not reproduced by the logs)

- A program record with region count 0 makes the region search run 65 536 times over memory.
- Notes more than 29 below or 30 above the region's base note, program numbers `>= P`, and
  channel-prefix values outside 0..15 read memory outside the defined tables (undefined).
- A track without an end-of-track event runs on into whatever follows it in memory; a song whose
  tracks never all end never reaches song end.
- `count` is 32-bit; deltas up to 0x0FFFFFFF are handled.
