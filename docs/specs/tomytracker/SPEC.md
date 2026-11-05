# Tomy Tracker (`sg.*`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Amiga "Tomy Tracker" module format (Tom "Tomy" Pakarinen, 1992) as played by the
UADE/Eagleplayer player "Tomy Tracker" (Wanted Team adaptation, V1.1; prefix `sg`). The format is a
compact 4-channel tracker: 31 sample records, a position list, 64-row patterns of 4-byte cells.

This is a description of observed behaviour (register-write logs, beam-read traces, module data) in
my own words. "cc" = Amiga colour clocks; "cycles" = CPU cycles of the reference host (1 cc = 2
cycles).

Status: a model that follows this text reproduces the reference's register writes (address, value,
order and colour-clock stamp, including the start-up writes and the clock of the first tick) exactly
for 12 synthetic modules, about 58 000 writes, including song ends and the ticks after them. The
archive has no real Tomy Tracker module (see README).

## 1. File format

All values big-endian. Offsets are from the start of the file.

| offset | size | meaning |
|---|---|---|
| 0 | u32 | total module size in bytes (song part + sample data) |
| 4 | u32 | song size `S` = offset of the sample data; `S = 704 + 1024 * patterns` |
| 8 | 31 x 14 | sample records, sample 1 first |
| 442 | u16 | restart position, 1-based |
| 444 | u16 | song length `L` = number of positions (1..127) |
| 446 | 2 | unused |
| 448 | 128 x u16 | position list: entry `i` is the byte offset of the pattern to play, counted from offset 704 (pattern `n` has offset `1024 * n`) |
| 704 | 1024 each | patterns |
| `S` | | sample data ("sample base") |

Sample record (14 bytes):

| offset | size | meaning |
|---|---|---|
| 0 | u16 | volume (0..64; the whole 16-bit value is kept, see 3) |
| 2 | u32 | start, byte offset from the sample base |
| 6 | u16 | length in words |
| 8 | u32 | loop start, byte offset from the sample base (not from the sample start) |
| 12 | u16 | loop length in words |

Pattern: 64 rows of 16 bytes; a row is 4 cells (channel 0 first) of 4 bytes:

| byte | meaning |
|---|---|
| 0 | command number times 4 (`cmd = byte / 4`, 0..13) |
| 1 | command parameter |
| 2 | instrument number times 7 (instrument 1..31; 0 = none) |
| 3 | note number times 2 (note 1..36; 0 = none) |

Period table (index = note number; index 0 is 0):

`0, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302,
285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113`

### 1.1 Detection

All of these hold (`size` = file size):

- `size > 1728`;
- the u32 at 0 is non-zero, at most 0x200000 and even;
- the u32 at 4 (`S`) is at most the u32 at 0 and even, and `S - 704` is a positive multiple of 1024
  (strictly: `(S - 704) / 1024` has remainder 0 and quotient at least 1);
- the byte at 444 is 0 and the byte at 445 (`L`) is below 128;
- the largest of the first `L` position entries equals `(patterns - 1) * 1024`, with
  `patterns = (S - 704) / 1024`.

At init the reference also refuses a module whose u32 at 0 is larger than the loaded file.
The prefix `sg` is not used by any other player of the reference set.

### 1.2 Module info

No title, author or subsongs (one song). Length = `L` positions; patterns as above; samples =
number of sample records with a non-zero length.

## 2. Playback state

Global: `counter` (16 bit, starts at 1), `speed` (16 bit, starts at 6), `row` (0..63, starts at 0),
`pos` (current position index, starts at 0), `dmabits` (16 bit), and the CIA-A port value the
filter command modifies (see 4.3).

Per channel (all zero at the first start; they are not cleared when the song is restarted in the
same process): `ins` (last instrument byte, i.e. instrument times 7), `note` (note byte of the last
note that started a sample), `cmd` (byte 0 of this row's cell), `par` (byte 1 of this row's cell),
`vol` (16 bit), `per` (current period, 16 bit), `target` (porta target period, 16 bit), `pspeed`
(porta speed, 8 bit), `pdir` (porta direction, 0 = up in period, non-zero = down), `vibcmd` (8 bit),
`vibpos` (8 bit), `loopstart`, `looplen` (Paula values to load after a note starts).

## 3. Tick

The host calls the player once per tick (every 70 940 cc, a CIA timer at 50 Hz). Each tick:
`counter = counter - 1` (16-bit wrap). If `counter` is not 0 it is an effect tick (5).
Otherwise it is a row tick (4): `counter = speed` first.

With speed 6: row tick, five effect ticks, row tick, ... A row tick never runs effects.

## 4. Row tick

`dmabits = 0`. The row's cells are read from pattern `704 + position[pos]` at byte `16 * row`.
Channels are processed in order 0..3. For channel `c` (`x` = Paula registers of voice `c`):

1. `cmd = byte 0`, `par = byte 1` (stored for every channel every row, empty cells give 0, 0).
2. Instrument: if byte 2 is non-zero, `ins = byte 2` and `vol = volume word of that sample`.
   The sample used below is that of `ins` (the new one, or the channel's previous one).
3. Note: if byte 3 is 0 go to 4.1. Else `p = period[byte 3 / 2]`; if `cmd` is 1 or 2 go to 4.2,
   else 4.3.

### 4.1 No note

If `cmd >= 9`, run the row command (4.4). Then write `x.PER = per`, then the volume write (6).

### 4.2 Note with command 1 or 2 (tone portamento set-up)

`pdir = 0`; `pspeed = high byte of vol` (normally 0; the parameter takes effect on the next tick,
see 5.1); `target = p`. Compare `p` with `per` (signed 16 bit): equal: `target = 0`; `p < per`:
`pdir = 0xFF`; `p > per`: nothing more. Then the volume write (6).
No period write, no sample start, no DMA change, `note`, `vibpos`, `loopstart`, `looplen` keep
their values. The row command is not run (commands 1 and 2 have none).

### 4.3 Note that starts the sample

`per = p`; `note = byte 3`; `dmabits |= 1 << c`; `vibpos = 0`.
Write `x.LC = sample base + sample start` (a 32-bit write, logged as two word writes at the same
clock), then `x.LEN = sample length`. Store `loopstart = sample base + loop start`,
`looplen = loop length`. If `cmd >= 9` run the row command (4.4). Then write `x.PER = per`, then the
volume write (6).

### 4.4 Row commands

| cmd | effect |
|---|---|
| 9 | none |
| 10 (0xA) | set volume: low byte of `vol` = `par` (high byte kept) |
| 11 (0xB) | pattern break: `row = 63`, so the row advance below goes to the next position, row 0 (the remaining channels of this tick still read the row the tick started with) |
| 12 (0xC) | filter: the CIA-A port value loses bit 1, then is ORed with `par` (two writes, see 4.5) |
| 13 (0xD) | set speed: low byte of `speed` = `par` and low byte of `counter` = `par` (immediate) |

Commands 1..8 have no row action (only effect-tick actions); 0 is nothing.

### 4.5 Filter command

The reference writes the CIA-A port register (0xBFE001) twice: first the current port value with
bit 1 cleared, then that value ORed with `par`. Bit 1 set = audio filter (power LED) off, clear =
on. Other bits of `par` are ORed into the port as well (bit 0 is the overlay bit on real hardware;
only bit 1 is meaningful for audio). In the reference logs the port value starts at 0 and reads back
as the last value written (e.g. `par = 0x41` then `0xFC` logs 0x00, 0x41, 0x41, 0xFD).

### 4.6 End of the row tick

After the four channels, in this order:

1. write `DMACON = dmabits` (turns off the voices that start a sample; 0 is written too);
   `dmabits |= 0x8000`.
2. `row = row + 1`. If `row == 64`: `row = 0`, and if `pos` is the last position (`L - 1`) the
   song-end notification is made and `pos = restart - 1`, else `pos = pos + 1`.
3. wait (7.3);
4. write `DMACON = dmabits` (0x8000 plus the started voices; 0x8000 alone when none);
5. wait (7.3);
6. for channels 0..3: write `x.LC = loopstart`, then `x.LEN = looplen` (all four channels on every
   row tick, whether a sample started or not; a channel that never started a sample writes 0 and 0).

## 5. Effect tick

For channels 0..3, by `cmd` (the value stored at the last row). Commands 0 and 9..13 do nothing
here. `x.PER` is the voice's period register.

### 5.1 Commands 1 and 2: tone portamento (identical)

If `par != 0`: `pspeed = par` and `par = 0` (so only the first effect tick of the row loads it;
with `par = 0` the previous speed is used). If `target == 0`: nothing. Else:

- `pdir == 0`: `per = per + pspeed`; if `target > per` (signed) write `x.PER = per`; else
  `per = target`, `target = 0`, write `x.PER = per`.
- `pdir != 0`: `per = per - pspeed`; if `target < per` (signed) write `x.PER = per`; else
  `per = target`, `target = 0`, write `x.PER = per`.

Consequence of 4.2: on a row with a note and command 1/2 and parameter 0, `pspeed` becomes 0 and
the slide does not move (but writes `per` every effect tick until a later row changes it).

### 5.2 Command 3: period down (pitch up)

`v = per - par` (signed); if `v < 0x71` then `v = 0x71`. `per = v`, write `x.PER = per`.

### 5.3 Command 4: period up (pitch down)

`v = per + par`; if `v >= 0x358` (signed compare) then `v = 0x358`. `per = v`, write `x.PER = per`.

### 5.4 Commands 5 and 6: vibrato

If `par != 0`: `vibcmd = par`. `s = sine[(vibpos >> 2) & 31]`,
`d = (s * (vibcmd & 15)) >> 7`. Write `x.PER = per + d` if `vibpos < 0x80`, else `per - d`
(`per` itself is not changed). Then `vibpos = (vibpos + ((vibcmd >> 2) & 0x3C)) & 0xFF`
(adds 4 times the high nibble). Command 6 then also does the volume slide (5.5) with the same
`par` (one parameter drives both).

`sine` (32 entries): `0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253,
255, 253, 250, 244, 235, 224, 212, 197, 180, 161, 141, 120, 97, 74, 49, 24`.

### 5.5 Command 7: volume slide

If `par & 15 != 0`: `vol = vol - (par & 15)`; if the result is negative (signed 16 bit) `vol = 0`.
Else `vol = vol + (par >> 4)`; if `vol >= 64` (signed) `vol = 64`. Then the volume write (6).
(A volume above 64 from command 10 is clamped to 64 by the first slide up.)

### 5.6 Command 8: arpeggio

`k = speed - counter` (counter after this tick's decrement; 1 on the first effect tick after a
row). `k mod 3` (valid for `k` up to 31): 0: write `x.PER = per`. 1: `n = par >> 4`. 2:
`n = par & 15`. For 1 and 2: `i = (note + 2 * n) & 0xFF`; if `i > 0x48` then `i = 0x48`; write
`x.PER = period[i / 2]` (`note` is the byte value, twice the note number; `per` is not changed).
A channel that never started a sample has `note = 0`, so it writes `period[n]` (0 for `n = 0`).

## 6. Volume write

Write `x.VOL = (vol & 0x7F) * level / 64`, where `level` is the host's volume for the voice side
(voices 0 and 3 left, 1 and 2 right: `level = balance * master / 64`, both 0..64; 64 by default,
which gives `vol & 0x7F`). A voice muted by the host writes 0. All reference logs use defaults.

## 7. Timing

### 7.1 Clock rule

For a call made by the host at clock `C`, keep a cycle count `T` from 0. Each step below adds its
cost; a register write (or beam read) gets the stamp `C + floor(T / 2)`, where `T` already includes
the cost of the step that makes it. At the song-end notification `T` is rounded down to an even
number before the host call is charged (7.4). Costs do not depend on data except where stated.

### 7.2 Row tick ledger (cycles)

| step | cost |
|---|---|
| entry and counter test | 277 |
| row set-up | 102 |
| per channel, channels 1..3 only | 20 |
| cell read | 53 |
| instrument: byte 2 = 0 / non-zero | 34 / 40 |
| note test | 29 |

No note (4.1): 25; if `cmd >= 9`: + 26 + row command; then 16 -> `x.PER`; 42; volume write
(table below); 22.

Note (4.2 / 4.3): 27, then (only if `cmd != 1`) 13.

- 4.2: 73; then equal: 12, greater: 9, smaller: 25; then 42; volume write; 22; channel 3 only: + 10 (this extra 10 exists only on this path).
- 4.3: 62 + start cost (channel 0..3: 123, 154, 185, 193); 20 -> `x.LC`; 12 -> `x.LEN`; 57; if
  `cmd >= 9`: + 26 + row command; then 16 -> `x.PER`; 42; volume write; 22.

Row command costs: 9: 10; 10: 30; 11: 26; 13: 50; 12: 24 -> first CIA write; 32 -> second CIA
write; 10.

Volume write (6), channel 0..3: 160, 183, 206, 229 -> `x.VOL`; then 38, 38, 38, 28.

End of row (4.6): 20 -> `DMACON` (off); 61; if the row wraps to 0: + 53, then if it was the last
position: + 56 + 72, round `T` down to even, + 16 (host notification) + 44; then (any wrap) + 24.
Then 456; wait; 20 -> `DMACON` (on); 38; wait; then `LC`/`LEN` pairs: channel 0: 20 -> `LC`,
12 -> `LEN`; channels 1..3: 24 -> `LC`, 16 -> `LEN`.

### 7.3 Wait

28; then 9 times: { 16, read the line `L0`; repeat { 16, read the line; stop if it differs from
`L0`; 9 }; 19 }; then 44. "Read the line" = `floor(stamp / 227)` with the stamp of 7.1 (lines of
227 cc counted from clock 0 of the run; only the change matters). Each wait therefore ends just
after the ninth line boundary, at a time that depends on the beam phase; in the logs the `DMACON` on write
comes 2146 to 2491 cc after the off write, and the first loop `LC` 2032 to 2045 cc after the on
write.

### 7.4 Effect tick ledger (cycles)

Entry and counter test 277; set-up 28. Per channel: 17; `cmd = 0`: + 30, done. Else 17;
`cmd >= 9`: + 30, done. Else 30 + command + 30.

| cmd | cost |
|---|---|
| 1, 2 | 17; if `par != 0`: + 20; 21; `target == 0`: 10. Else 33; 49; not reached: 16 -> `x.PER`, 10; reached: 24 + 16 -> `x.PER`, 10 |
| 3 | 41; + 4 if clamped; 20 -> `x.PER`; 10 |
| 4 | 37; + 8 if clamped; 20 -> `x.PER`; 10 |
| 5, 6 | 17; + 12 if `par != 0`; 175; then `vibpos >= 0x80`: 4, else 14; 8 -> `x.PER`; 54; 11; command 6 continues with the slide |
| 7 (and the slide of 6) | 29; down: 25, + 12 if clamped to 0; up: 55, + 16 if clamped to 64; then 62; volume write (the channel cost, the `x.VOL` write and its trailing cost, as in 7.2); then a further 38 |
| 8 | 55; `k mod 3 = 0`: 16 -> `x.PER`, 10. Else 13; 1: 24, 2: 8; 33; + 4 if clamped; 18 -> `x.PER`; 10 |

### 7.5 Start-up

The host makes these calls one after the other, the first at clock 100; each call lasts
`floor(T / 2)` cc of its own cycle count `T`, and the next starts when it ends:

1. detection (1.1): `452 + 35 * L + 8 * U` cycles, `U` = the number of position entries (in list
   order, first `L`) that are greater than or equal to the largest earlier entry (the first always
   counts);
2. init: `4624 + 8 * N + 42 * L` cycles, `N` = sample records with a non-zero length;
3. structure query: 24;
4. volume: 1392 cycles; writes `VOL0..VOL3 = 0` (0xDFF0A8, B8, C8, D8) at `T` = 434, 707, 1003, 1322;
5. balance: the same as 4 (again four writes of 0);
6. start: 1188 cycles; sets the state of 2 (no writes).

The first tick is called 70 940 cc after the end of call 6, then every 70 940 cc.

### 7.6 Stop

On stop the reference writes `VOL0..VOL3 = 0` and `DMACON = 0x000F` (not in the logs).

## 8. Undefined inputs

The reference does not check these; behaviour is the reading of other memory and is not specified:
command byte not a multiple of 4 or above 13 * 4 (byte 0 >= 0x38; a byte >= 0x80 is also treated as
negative by the effect dispatch); instrument byte not a multiple of 7; note number above 36; a note
on a channel that never had an instrument (the reference then reads its own tables as a sample
record); restart 0 or above `L`; set speed 0 (the counter wraps to 65535); arpeggio with `k > 31`
(speed above 32).
