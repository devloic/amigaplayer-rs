# Laxity (`powt.*`, `pt.*`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the module format played by the UADE/Eagleplayer player "Laxity" (player name
"Laxity-Player", Eagleplayer V2.1 adaptation of a Kefrens replayer; UADE prefixes `powt` and `pt`).
Not covered: the NostalgicPlayer "Laxity Tracker" `.lax` files (a different, ProTracker-like layout
that this player rejects) and ordinary ProTracker modules named `pt.*` (also rejected, see 1.2).

This is a behaviour description derived from write logs, DMA logs and execution traces of the 68k
reference (the Eagleplayer run through the host with DELI_WRITE_LOG). It contains no code of the
original. Cycle numbers are the 68000 cycle counts the reference host charges (2 cycles = 1 colour
clock, "cc"); the host charges a fixed count per instruction, so every cost below is a constant.

Status (checked by a private model that follows this text, against the reference):

| item | result |
|---|---|
| 8 synthetic modules (no real module of this format was found, see README), 3 716 ticks, 20 355 register writes | value, order and absolute colour clock of every write equal the reference |
| DMA log of the synth waveforms (48 000 audio words fetched from the synth buffers) | equal to the model's buffer contents (the few exceptions are fetches the host evaluates late, see LOGS.md) |
| a second, blind model written only from this text (no other source) | same result on all eight logs after one ledger typo found by it was corrected (commands 1 and 2, now fixed) |
| code paths | every path of the play routine that can execute has run in at least one synthetic module |

## 1. File format

All values big-endian. Offsets are from the start of the file. A module is one file.

### 1.1 Layout

| offset | size | meaning |
|---|---|---|
| 0 | u16 | `P`: offset of the pattern data (sign-extended 16 bit) |
| 2 | u16 | `S`: size of the instrument area, `24 * n + 16` for `n` instruments |
| 4 | u16 | `L`: song length (number of positions) |
| 6 | u16 | `R`: restart position |
| 8 | 16 bytes | signature area (1.2); also the first 16 bytes of the unused "instrument 0" |
| 24 * i | 24 bytes | instrument `i`, `i = 1 .. n` (1.3) |
| 8 + S | u16 | `Z`: pattern size in bytes (4 bytes per row, so `Z / 4` rows) |
| 10 + S | 4 * L bytes | position lists: voice `v` (0..3) uses bytes `10 + S + v * L + pos`, `pos = 0 .. L-1` |
| P | | patterns: pattern `k` is `Z` bytes at `P + k * Z` (1.4) |
| after the patterns | | sample data (1.5) |

`P` is normally `10 + S + 4 * L` (the player uses `P` for the patterns but computes the sample start
from the lists, 1.5). The number of instruments the player sets up is `floor(S / 24)` (with `S`
taken as a signed 16-bit value, so at most 1364).

Each voice has its own track: a position byte selects, for that voice alone, the pattern it plays.
Position bytes 0xFE and 0xFF are special (3.2). Pattern numbers are meant to be 0..0x7F (a byte of
0x80..0xFD is sign-extended before the multiplication by `Z`, giving a negative offset).

### 1.2 Detection

The module is accepted when:

- the bytes at offsets 0x1F and 0x37 are both 0xFF (the synth flags of instruments 1 and 2: every
  module starts with two synth instruments), and
- one of:
  - the 32-bit words at offsets 0, 0x30, 0x34 are 0x00820048, 0x00010008 and 0x001800FF (one
    specific module that the check lets through by name), or
  - the byte at offset 8 is 0, or
  - the 16 bytes at offset 8 are the ASCII text `YET ANOTHER COOL`.

(The zero test covers a number of bytes that comes from a value the host passes to the check
unintentionally; with the reference host it is one byte, offset 8. If that byte is not zero, the
16-byte text must match.)

This rejects every ProTracker-style file (`pt.*` names in collections are mostly ProTracker
modules), whose bytes 0x1F and 0x37 are sample-name or header bytes. It is a weak test: in the
corpus it would also accept a few unrelated files (README), so a host should try this format after
the formats with strong magic words.

### 1.3 Instrument (24 bytes at offset `24 * i`)

| off | size | sample instrument | synth instrument |
|---|---|---|---|
| 0 | u16 | length in words | low byte: synth speed (ticks between waveform updates, plus one) |
| 2 | u16 | loop start in words | low byte: start (and minimum) pulse width |
| 4 | u16 | loop length in words; 0 = one-shot | low byte: maximum pulse width |
| 6 | s8 | finetune (signed factor, 3.3) | finetune |
| 7 | u8 | 0 = sample instrument | non-zero = synth instrument |
| 8..23 | | unused by the player | unused |

The word at offset 0 is also used for synth instruments when the sample start addresses are
computed (1.5).

### 1.4 Pattern cell (4 bytes)

| byte | bits | meaning |
|---|---|---|
| 0 | 7-6 | instrument number bits 5-4 |
| 0 | 5-0 | note 1..63, 0 = none |
| 1 | | volume column: 0x00..0x7F set volume; 0x8X volume up by X every tick; 0x9X volume down by X every tick; 0xFE pattern break; 0xFF and 0xA0..0xFD nothing |
| 2 | 7-4 | instrument number bits 3-0 (instrument 0 = none) |
| 2 | 3-0 | command 0..15 |
| 3 | | command parameter |

### 1.5 Sample data and the synth buffers

Init computes the sample start of every instrument: start at `10 + S + 4 * L + D`, where
`D = ((M + 1) * Z)` taken as a signed 16-bit value and `M` is the largest byte of all `4 * L`
position bytes, the 0xFE/0xFF markers included. Instruments 1, 2, ... then follow each other: the
start of instrument `i + 1` is the start of `i` plus twice the word at offset 0 of instrument `i`
(synth instruments included). Sample data is signed 8-bit.

Synth instruments play a 32-byte waveform that the player writes into memory at module start +
0x1166 + 32 * voice (4 buffers, 128 bytes; this is inside or beyond the module whatever its size,
the player does not check). A port must reproduce this memory as Paula reads it (5).

### 1.6 Title, subsongs, length

No title, author or subsong in the format: one song. The song is `L` positions long; when the
position counter reaches `L` the player signals the song end to the host and continues at `R`
(3.6). Position jumps can make a song that never reaches `L`.

## 2. Player state and initialisation

### 2.1 State block

The player keeps its state in a 190-byte block whose initial content is part of the player file,
not of the module (`data/initial_state.bin`, exact bytes; the reference player never clears most of
it). Layout (offsets in the block):

| off | size | meaning | initial |
|---|---|---|---|
| 0x00 | u16 | a zero word; one-shot samples loop on it (3.3) | 0 |
| 0x02 | u16 | pattern size `Z` | set by init |
| 0x04 | u16 | speed (ticks per row) | 6 |
| 0x06 | u16 | tick counter | 2 |
| 0x08 | u16 | position | set by init (0) |
| 0x0A | u16 | restart position | set by init |
| 0x0C | u16 | row offset (row * 4) | set by init (0) |
| 0x0E | u16 | length `L` | set by init |
| 0x10 | u8 | break flag | 0 |
| 0x12 | u8 | slow-slide counter (command 5) | 0 |
| 0x14 | u16 | row DMA mask | (cleared at every row) |
| 0x16 | 4 x 10 | synth records, voice 0..3 (2.2) | from the file |
| 0x3E | 4 x 32 | voice records, voice 0..3 (2.3) | from the file |

The file's values are left-overs of a session of the adapter's author: the voice and synth records
hold absolute addresses (0x00057D3E, 0x00057D5E, 0x00057D7E, 0x00057D9E) and periods, volumes and
cells. They take effect (1) on the first tick, which is an effect tick of these cells (3.1), (2) in
the loop-register writes of 3.5 for a voice that has not yet had an instrument, and (3) in the
synth update, which keeps writing waveforms to the absolute addresses of synth records 0, 1, 2
(record 3 is off) until each voice gets an instrument. A port reproduces the register values
exactly by starting from this block; the memory writes at the absolute addresses land after the
module in the reference host (module at 0x45E0) unless the module is larger than about 340 KiB.

Also persistent: a DMA word (the last DMA-on value, initially 0) and the CIA filter bit.

### 2.2 Synth record (10 bytes)

| off | size | meaning |
|---|---|---|
| 0 | u32 | waveform address; the record is off when bit 31 is set (the player marks "off" by writing 0xFF to byte 0) |
| 4 | s8 | countdown |
| 5 | u8 | reload (instrument byte 1) |
| 6 | s8 | width step, +1 or -1 |
| 7 | u8 | current width |
| 8 | s8 | minimum width (instrument byte 3) |
| 9 | s8 | maximum width (instrument byte 5) |

### 2.3 Voice record (32 bytes)

| off | size | meaning |
|---|---|---|
| 0x00 | 4 bytes | current cell (byte 2 keeps only the command nibble after the row is read) |
| 0x04 | u32 | sample start address |
| 0x08 | u16 | sample length (words) |
| 0x0A | u32 | loop start address |
| 0x0E | u16 | loop length (words) |
| 0x10 | u16 | period |
| 0x12 | u8 | volume |
| 0x13 | u8 | vibrato parameter (speed in the high nibble, depth in the low nibble) |
| 0x14 | u8 | vibrato position |
| 0x15 | u8 | note * 2 |
| 0x16 | u16 | tone portamento target: never written by the player (0 in the file for all voices) |
| 0x19 | u8 | tone portamento direction: 0 = none, 0xFF = towards lower periods |
| 0x1C | s16 | finetune offset of the current note |
| 0x1E | u16 | the voice's DMA bit (1, 2, 4, 8) |

### 2.4 Init

Init sets: synth buffer base = module + 0x1166; length = word 4; restart = word 6; position = 0;
row offset = 0; pattern size = `Z`; the sample start of every instrument (1.5). Nothing else: speed,
tick counter, break flag, slow-slide counter and all records keep their values. No register write.

## 3. The tick

The play routine runs once per tick (vertical blank, 50 Hz). It first decrements the tick counter
(16 bit). If the result, as a signed number, is greater than 0, the tick is an effect tick (3.1);
otherwise it is a row tick (3.2). Then the end-of-tick steps (3.6) follow.

Speed 0 and 1 make every tick a row tick.

### 3.1 Effect tick

For voice 0, 1, 2, 3 in this order: the volume column of the current cell (3.4), then the command's
tick effect (4). Then the synth update (5). There is no DMA write on an effect tick except the
retrigger command's.

### 3.2 Row tick

The position and the row offset are taken once at the start of the row (a position jump on voice 0
does not change what voices 1-3 read in the same row). The row DMA mask is cleared. For voice
0, 1, 2, 3 in this order, read the voice's position byte `b`:

- `b = 0xFE`: nothing happens on this voice (no write; its old cell stays and its tick effects keep
  running on the following effect ticks).
- `b = 0xFF`: write volume 0 (AUDxVOL = 0) and switch the voice's synth record off. The old cell
  stays (its tick effects keep running, and a volume command in it can make the voice audible again).
- otherwise: copy cell `row` of pattern `b` into the voice record and process it (3.3).

After the four voices: the synth update (5). Then, if the row DMA mask is non-zero: write DMACON =
mask (DMA off for those voices), remember `mask | 0x8000` as the DMA word, and do the DMA restart
(3.5). Then: tick counter = speed; row offset += 4; if the row offset is no longer below `Z`
(unsigned), go to the next position (3.6).

### 3.3 Processing a cell (row tick)

1. If volume column byte < 0x80, volume = that byte. Keep only the command nibble in byte 2.
2. Instrument `i` (bits as in 1.4). If `i != 0`:
   - synth instrument: the voice's synth record gets: waveform address = synth buffer of this voice,
     countdown 0, reload = instrument byte 1, step +1, width = minimum = instrument byte 3,
     maximum = instrument byte 5. Sample start = loop start = that buffer; length = loop length =
     16 words.
   - sample instrument: synth record off; sample start = the instrument's start (1.5); length =
     word 0; loop length = word 4; if the loop length is 0, loop length = 1 and loop start = the
     zero word of the state block (an address inside the player, 0x0095505C in the reference
     host); otherwise loop start = sample start + 2 * word 2.
3. Write AUDxVOL = volume (every processed cell writes it, also without note or instrument: an empty
   cell with volume byte 0 silences the voice).
4. If note `n != 0`:
   - note*2 is stored.
   - If the sample length is 0: only set the voice's bit in the row DMA mask (no further register
     write for the note); go to step 5.
   - The finetune byte `f` is read from the instrument given in this cell, or, when the cell has no
     instrument, from offset 6 of the file (the high byte of `R`). Finetune offset =
     `f * FT[n]` (signed 16 bit; 0 when `f = 0`); new period = `PER[n] +` finetune offset (16 bit).
   - Command 3 (tone portamento): no register write and the period is not stored. If the new
     period equals the current period, stop here (the volume column and the row command of this
     cell are not processed). Otherwise direction = 0xFF if new period < current period (signed),
     else 0; go to step 5.
   - Otherwise: period = new period; write AUDxLC = sample start (high word, low word, same time),
     AUDxLEN = sample length, AUDxPER = period; vibrato position = 0; set the voice's bit in the row
     DMA mask.
5. The volume column (3.4) and the row command (4.1).

Tables (index = note `n`, 1..60):

`PER[n]` = 3424 3232 3040 2880 2720 2560 2416 2288 2160 2032 1920 1808,
1712 1616 1520 1440 1360 1280 1208 1144 1080 1016 960 904,
856 808 760 720 680 640 604 572 540 508 480 452,
428 404 380 360 340 320 302 286 270 254 240 226,
214 202 190 180 170 160 151 143 135 127 120 113.

`FT[n]` = 24 24 20 20 20 18 16 16 16 14 14 12 12 12 10 10 10 9 8 8 8 7 7 6 6 6 5 5 5 4 4 4 4
3 3 3 3 3 2 2 2 2 2 2 2 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 (60 values; `FT[60] = 1`).

Out of range (notes 61..63, and arpeggio indexes up to 78): the two tables are contiguous with other
data. `PER[0] = 1`; `PER[61] = 252`, `PER[62] = 3648`, `PER[63] = 0`; `PER[64..]` are the state
block's words from offset 0x02 on (`PER[64]` = pattern size, `PER[65]` = speed, `PER[66]` = tick
counter, `PER[67]` = position, ..., `PER[73]` = row DMA mask, `PER[74..78]` = synth record 0).
`FT[61] = 3424`, `FT[62] = 3232`, `FT[63] = 3040` (the first words of `PER`).

### 3.4 Volume column (every tick, before the command)

Byte 1 of the current cell:

- below 0x80 or 0xFF: nothing (on a row tick the volume was already set in 3.3).
- 0xFE: set the break flag (to 0xFF). This happens on every tick the cell stays current, so the
  position advances once on the row tick and once more on each following effect tick (3.6).
- 0x8X: volume = volume + X (8 bit); if the result, as a signed byte, is above 64 it becomes 64
  (0x7F + 1 = 0x80 is negative and is kept); write AUDxVOL = volume.
- 0x9X: if (volume as a signed byte) - X < 0, volume = 0, else volume -= X; write AUDxVOL = volume.
- other values 0xA0..0xFD: nothing.

The volume written is the byte value as it is (0..0xFF): no masking, no scaling.

### 3.5 DMA restart

Write DMACON = DMA word (0x8000 | mask); the host's DMA-on call then waits for the beam (6.3).
Then for voices 0, 1, 2, 3: AUDxLC = loop start, AUDxLEN = loop length, of every voice (also
voices that are not in the mask, also voices that never had an instrument: their stale record
values are written).

### 3.6 End of the tick, positions, song end

"Next position": row offset = 0, break flag = 0, position += 1 (16 bit); if position equals `L`,
signal the song end to the host and set position = `R`. (No other bound: a position past `L` keeps
counting.)

At the end of every tick (after 3.1 or 3.2): while the break flag is non-zero, do "next position".
Then the host's end-of-tick notification (no register write).

The break flag is set by the volume column 0xFE (0xFF), command 11 (0xFF) and command 13 (plus
one, 8 bit; so 0xFE in the same row followed by 13 wraps it to 0).

## 4. Commands

### 4.1 Row commands (row tick, after the volume column)

| cmd | action |
|---|---|
| 5 | slow-slide counter = 1 |
| 7 | tick counter = speed * parameter; overwritten by the reload at the end of the row tick, so no effect |
| 11 | position jump: the low byte of the position = parameter - 1 (8 bit), break flag = 0xFF; with the "next position" this gives position = parameter (parameter 0 gives 0x100) |
| 12 | `x` = parameter, at most 0x40; the vibrato parameter byte = `x` (not the volume); write AUDxVOL = `x` (a second volume write after the one of 3.3; the stored volume is unchanged) |
| 13 | pattern break: row offset = parameter * 4, break flag += 1; "next position" then sets the row offset to 0, so the next position always starts at row 0 |
| 14 | filter: clear bit 1 of CIA-A PRA (address 0xBFE001) by a read-modify-write, then write it again OR 0 (two byte writes, same value; the parameter is not used; the filter ends up on; PRA reads 0 in the reference host, so both writes are 0x00) |
| 15 | speed: the low byte of the speed = parameter (the high byte stays 0) |
| others | nothing |

### 4.2 Tick effects (effect ticks, after the volume column)

"Current period" is the voice's period field.

| cmd | action |
|---|---|
| 0 | arpeggio, if the parameter is non-zero: `t = ARP[tick counter]`; `t = 0`: write AUDxPER = current period; `t = 1`: high nibble `h`; `t = 2`: low nibble `h`; then write AUDxPER = `PER[(2*h + note*2) mod 256 / 2]` + finetune offset (16 bit). `ARP[i] = i mod 3` for the tick counter values 0..31 (higher counters read unrelated bytes; not supported). Parameter 0: no write |
| 1 | period -= parameter; if below 0x71 (signed), period = 0x71; write AUDxPER |
| 2 | period += parameter (16 bit, no limit); write AUDxPER |
| 3 | tone portamento: if direction is 0, nothing. Otherwise (direction 0xFF): period -= parameter; if target (always 0) > period as signed numbers (the period went negative): write AUDxPER = target (0) and direction = 0; else write AUDxPER = period. The upward direction cannot occur |
| 4 | vibrato: a non-zero parameter updates the vibrato byte: low nibble 0 → only the high nibble is replaced; high nibble 0 → only the low nibble is replaced; else the whole byte. Then `d = SINE[(pos & 0x1F) >> 2] * (vib & 0xF) >> 7`; write AUDxPER = period + `d` (vibrato position bit 7 clear) or period - `d` (set); the period field is unchanged; then pos += (vib >> 2) & 0x3C (8 bit). `SINE[0..7]` = 0 24 49 74 97 120 141 161 |
| 5 | slow slide: the slow-slide counter (one byte shared by all voices) -= 1; when it reaches 0: if the parameter's high nibble is non-zero, counter = high nibble and volume up by 1 (as 3.4 0x8X, with its write); else counter = low nibble and volume down by 1 (as 0x9X). If it was 0 it wraps to 0xFF and counts down from there |
| 6 | retrigger, if the parameter is non-zero and the tick counter mod parameter is 0: write DMACON = the voice's bit (off), DMA word = bit | 0x8000, write AUDxLC = sample start, AUDxLEN = sample length (this voice), then the DMA restart of 3.5 (all four voices' loop registers) |
| 7, 8, 9, 11..15 | write AUDxPER = current period |
| 10 | write AUDxPER = current period, then volume slide: high nibble non-zero: up by the high nibble; else down by the low nibble (as 3.4, with its AUDxVOL write) |

Commands 0..6 write no period when they do nothing (arpeggio 0, tone portamento without
direction, retrigger at other ticks).

## 5. Synth update (every tick, after the voices)

For synth record 0, 1, 2, 3: if it is off, skip. Countdown -= 1; if the result is still >= 0
(signed), skip. Otherwise countdown = reload, then: width += step (8 bit); if width < minimum
(signed): width = minimum and step = -step; else if width > maximum (signed): width = maximum and
step = -step. Store the width `w` and write the 32-byte waveform at the record's address:
`16 - floor(w/2)` bytes 0x80, then `w` bytes 0x00, then `16 - ceil(w/2)` bytes 0x7F (for
`w <= 32`; a larger width writes far past the buffer). There is no register write: Paula reads
the changed bytes through its DMA (the buffers are the voices' sample and loop data). The first
update after a synth instrument starts happens in the same row tick (countdown 0 - 1 = -1), with
width = minimum + 1 (or the clamp).

## 6. Timing

### 6.1 Frame

The play routine is called every 70 940 cc. The first call's clock is host business (76 320 to
77 426 in the reference logs; it depends on the host's init work). The first write of the first
tick is 427 cc after the call (the stale state block's effect tick).

### 6.2 Clock model

Per tick keep a cycle counter `T`, 0 at the call, and an epoch `(E, TE)` = (call clock, 0).
Every register write goes through a host call (an Eagleplayer ENPP function) that takes no time
itself; its write is stamped `E + floor((T - TE) / 2)` with `T` including the cycles listed as
"to" for that call. After every host call (also those without a register write: the song-end
signal and the end-of-tick notification), set `E` = the stamp (for a DMA-on call: the clock after
its wait, 6.3) and `TE = T`, then add the "back" cycles. The CIA writes of command 14 are not host
calls: they are stamped the same way but do not change the epoch.

Host calls ("to" includes the call itself; a "tail" call is one that a step enters at its very end,
whose return goes straight back to the step's caller):

| call | to | to (tail) | back |
|---|---|---|---|
| VOL (AUDxVOL) | 102 | 84 | 60 |
| LC (AUDxLC, both words at once) | 102 | | 60 |
| LEN (AUDxLEN) | 134 | | 68 |
| PER (AUDxPER) | 110 | | 60 |
| DMA (DMACON) | 112 | | 68 |
| song end | 64 | | 44 |
| end-of-tick notification | 34 | | 164 |

### 6.3 DMA-on wait

After the DMACON write with bit 15 set, the clock moves to the start of the tenth raster line after
the current one: `clock = (floor(clock / 227) + 10) * 227` (absolute colour clocks of the host; the
line phase of a tick changes from tick to tick because 70 940 is not a multiple of 227). A DMA-off
write has no wait.

### 6.4 Cycle ledger

All numbers are cycles added to `T` in the order given; in "X; condition: Y" the cost X is paid before the test and Y only when the condition holds. "VC" is the volume column ledger, "W(x)"
a host call of 6.2.

Tick: 157. Then:

- Effect tick: voice 0: 38, voices 1-3: 34 each, each followed by the voice's tick-effect ledger;
  then 18, synth ledger, 10, end ledger.
- Row tick: 150, then voice 0's row ledger; voices 1-3: 58 each, then their row ledgers; then 18,
  synth ledger, 29. If the row DMA mask is non-zero: W(DMA off), 38, restart ledger. Then 65; if
  the row ends: next-position ledger. Then the end ledger.

End: 29; while the break flag is set: next-position ledger, 29. Then W(notification).

Next position: 81; on reaching `L`: W(song end), 20.

Restart: 52, W(DMA on), 36, voice 0: W(LC), 12, W(LEN); voices 1-3: 24, W(LC), 12, W(LEN); then 60.

Row ledger (one voice):

| case | cycles |
|---|---|
| start | 25 |
| position byte 0xFE | 15, 16, done |
| position byte 0xFF | 15, 16, W(VOL 0), 12, 126, done |
| other: cell read | 195; +33 if the volume byte is below 0x80; 35 |
| instrument given | 85; synth: 18 + 352, 70; sample: 18 + 126, 99, then 50 (loop length 0) or 60 |
| volume | 32, W(VOL), 39 |
| no note | tail |
| note | 39; sample length 0: 32, tail |
| | 43; +72 if finetune non-zero; 61 |
| command 3 | 27; same period: 16, done (no tail); else 27, tail |
| other commands | 40, W(LC), 12, W(LEN), 12, W(PER), 60, tail |

Tail: 18, VC, 31; command 0: 16. Otherwise the commands 5, 7, 11, 12, 13, 14, 15 are tested in this
order at 15 cycles per test, up to and including the matching one; no match (1, 2, 3, 4, 6, 8, 9,
10): 7 * 15 + 16. Then:

| cmd | cycles |
|---|---|
| 5 | 36 |
| 7 | 114 |
| 11 | 68 |
| 12 | 33, +4 if the parameter is above 0x40, 43, W(VOL, tail) |
| 13 | 78 |
| 14 | 24, CIA write, 20, CIA write, 16 |
| 15 | 40 |

VC (volume column): 23; below 0x80: 16. Else 19; 0xFF: 16. Else 15; 0xFE: 36. Else 41; 0x8X: UP.
Else 15; 0x9X: DOWN. Else 16.

UP: 41; +26 if clamped to 64; 26, W(VOL, tail). DOWN: 25; +16 if cleared to 0; 26, W(VOL, tail).

Tick-effect ledger (one voice): 18, VC, 23; then the command's dispatch cost and handler:

| cmd | dispatch | handler |
|---|---|---|
| 0 | 0 | 23; parameter 0: 16. Else 37; phase 0: 24, W(PER), 28; phase 1: 13 + 14, 58, W(PER), 16; phase 2: 13, 58, W(PER), 16 |
| 1 | 15 | 57; +12 if clamped; 20, W(PER), 28 |
| 2 | 30 | 46, 20, W(PER), 28 |
| 3 | 43 | 23; direction 0: 16. Else 37, 49; target above period: 24, W(PER), 44; else 24, W(PER), 28 |
| 4 | 58 | 21; parameter non-zero: 23, then low nibble 0: 46, else 29 and (high nibble 0: 46, else 12); then 175; +14 (position bit 7 clear) or +4 (set); W(PER); 62 |
| 5 | 73 | 31; counter not 0: 16. Else 35; high nibble non-zero: 30, UP (by 1); else 50, DOWN (by 1) |
| 6 | 88 | 27; parameter 0: 16. Else 159; remainder non-zero: 16. Else 12, W(DMA off), 52, W(LC), 12, W(LEN), 22, restart ledger |
| 7..15 | 88 | 24, W(PER), 27; command 10: 39, then high nibble non-zero: UP (by it); else 20, DOWN (by the low nibble); other commands: 16 |

Note that UP and DOWN reached from the tick effects end with a tail VOL call (84 to, 60 back).

Synth ledger: 16; per record: 21; off: 18. Else 25; countdown still >= 0: 18. Else 57; if the step
is non-zero: 33, then below minimum: 38; else 21 (+28 if above maximum). Then
`67 + 18 * a + 10 + 13 + 22 * w + 21 + 18 * c`, where `a = 16 - floor(w/2)`, `c = 16 - ceil(w/2)`
(0 if negative), then 18. After the four records: 16.

### 6.5 Observed gaps (cc, from the logs)

Note start on a voice: VOL -> LC 192 (228 with a non-zero finetune), LC -> LEN 103, LEN -> PER 95
(the synth set-up comes before the VOL write and adds nothing between these writes). Last write of
the voices -> DMACON off: 272 to 1 609 depending on the voices and the synth work; DMACON off -> on
135; DMA on -> first loop LC 2 150 to 2 373 (the 10-line wait); loop writes LC -> LEN 103, LEN ->
next LC 97. Retrigger: DMACON off -> LC 111, LEN -> DMACON on 127. Command 14: the two CIA writes
are 10 apart, 160 after the voice's VOL write.

## 7. How to check

`tools/replay-cmp.py REF.log PORT.log` compares logs tick by tick. The synth buffers are inside or
at a fixed offset from the module (module + 0x1166 + 32 * voice), so replay-cmp shows them as
`M+off`; the one-shot loop address (the zero word) and the stale addresses of 2.1 are absolute
values of the reference host. The logs in `logs/` are the references (LOGS.md).
