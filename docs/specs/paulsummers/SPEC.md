# Paul Summers (Amiga, `snk.*`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Amiga sound driver of Paul Summers and Mike Chilton (Fighting Soccer, Postman Pat, the Everton FC
Intelligensia jingles, P47 Thunderbolt), as played by the UADE/Eagleplayer player "Paul Summers" (file prefix
`snk`). A module file is the game's own sound driver followed by its song data; an open replayer has to read
the data tables out of that file and reimplement the replay.

This is a behaviour description derived from write logs, instruction-level execution traces and black-box
runs of the 68k reference (the Eagleplayer running the unmodified module on the emulated machine). It contains
no code of the original. The numbers in section 5 are 68000 cycle counts as the reference machine charges them
(2 cycles = 1 colour clock, "cc"); every branch has one fixed cost.

Status (all of it checked by a private model that follows only this text, against the reference):

| item | result |
|---|---|
| 4 corpus modules, all 23 subsongs, about 204 700 tick-call register writes plus the start sequences | value, order and absolute colour clock of every write equal the reference |
| 5 synthetic modules (edited copies of a corpus module: pitch effect, loops, deep call stacks, rests, zero durations, out-of-range notes, song end in every voice order, negative speed values, deep voice-0 stack), 21 subsong runs, about 10 500 writes, plus one 20-call-deep run of 1 838 writes | same |
| paths never executed by any reference run | listed in README |

"Absolute colour clock" = the tick's call time (given by the host, 5.1) plus the model of section 5.

## 1. File format

### 1.1 What a module is

A module is a position-independent 68k image, big-endian, no hunks and no header: 650 to about 1700 bytes of
driver code first, then data (period table, song table, instrument tables, note streams, sample data) in an
order that differs between games. The open replayer never runs the code. It needs three things from the
image: the **song table**, the **period table** and the **subsong count**; everything else is reached through
pointers that the data contains.

The four corpus files are 78 358 bytes (`Fighting_Soccer`), 87 472 (`jingles`, the Everton FC Intelligensia
jingles), 102 400 (`P47_Thunderbolt`, exactly 100 KiB, perhaps a truncated dump) and 91 260 (`Postman_Pat`).

### 1.2 Detection

The host checks the file with a pattern search over the driver code. All of the following must hold.

1. The file is larger than 3000 bytes.
2. Starting at file offset 650 and stepping 2 bytes, at most 10 positions `p` are examined (650, 652, ... 668).
   At each, first the 32-bit value at `p` is compared with `0x46FC2700` ("new form"); if it differs, the 16-bit
   word at `p` is compared with `0x41FA` ("old form"); if neither matches, the next position is tried. If all
   10 fail, the file is rejected.
3. New form, `p` the matching position. Start at `p`. Repeat: if the 32-bit value at the current position is
   zero, reject; read the 16-bit word at the current position and advance 2; if it is not `0x4E73`, repeat;
   read the next word and advance 2; if it is not `0x41FA`, repeat; otherwise go to step 5.
4. Old form. Skip 4 bytes (to `p + 4`). The 32-bit values at `p + 4`, `p + 10`, `p + 16` and `p + 22` must all
   be `0x117C0088` (reject otherwise). Continue at `p + 28` with step 5.
5. From the current position read 16-bit words (advancing 2 each) until one equals `0x41FA`. The signed 16-bit
   word that follows it, added to the file offset of that following word, must give exactly 0, the start of the
   file (the driver loads a pointer to its own start). Otherwise reject.

Corpus: `Fighting_Soccer` is new form at 654, `P47_Thunderbolt` new form at 650, `jingles` old form at 662,
`Postman_Pat` old form at 654.

The host also applies one data fix: when the file is longer than 14 000 bytes and the 16-bit word at offset
13 836 equals `0xF87F`, the host sets that word to 0 before playing (Fighting Soccer). No corpus subsong was
seen to read those bytes, so the effect is unobserved.

### 1.3 Locating the tables

Starting at file offset 650 the host scans, in 16-bit word steps, for these markers in this order. "Pointer
at `a`" means: the signed 16-bit word at `a` plus `a` itself.

1. The first word equal to `0x45FA`. Let `a1` be the offset after it. `target1` = pointer at `a1`.
2. Continuing from `a1`, the first word equal to `0xC0FC`. Let `e` be the offset after it. The unsigned 16-bit
   word at `e - 8` is the fallback subsong figure `n0`.
3. `song` (the song table) = pointer at `e + 4`.
4. If `target1 >= song`: `n0 := floor((target1 - song) / 20)`. Subsong count `N := n0 - 1`.
5. Continuing from `e + 4`, the first word equal to `0x4480`; let `f` be the offset after it. `per` (the period
   table base) = pointer at `f + 2`.

| module | `song` | `per` | `target1` | fallback word | `n0` | N |
|---|---|---|---|---|---|---|
| Fighting_Soccer | 3714 | 2444 | 3364 | 10 | 10 (target1 < song) | 9 |
| jingles | 3202 | 2670 | 3342 | 7 | 7 = (3342 - 3202) / 20 | 6 |
| P47_Thunderbolt | 3260 | 2236 | 3156 | 6 | 6 (target1 < song) | 5 |
| Postman_Pat | 2996 | 2070 | 3076 | 5 | 4 = (3076 - 2996) / 20 | 3 |

There is no title, no author and no length field. The subsong numbers offered by the host are 1 to N; song
table entry `k` is used for subsong `k` (entry 0 is never selected). The host's own limit is 15 subsongs and
the replay indexes the table with the low 4 bits of the subsong number only (3.3).

### 1.4 Song table

`song` points at entries of 20 bytes, entry `s` at `song + 20 * s`:

| off | size | meaning |
|---|---|---|
| 0, 4, 8, 12 | s32 each | stream start of voice 0, 1, 2, 3, as an offset from `song` (not from the file start): stream offset = `song` + value. A value of 0, a value of `0x00001EF8`, or a stream offset of 0 or below (not inside the file) means "this voice is unused" |
| 16 | s8 | speed value (3.2) |
| 17 | u8 | speed counter start (0 in all corpus entries) |
| 18 | u16 | timer value; 0 = keep the host default |

The tick period in colour clocks is `5 * (timer + 1)`; with timer 0 the host default is 14 187, which gives
70 940 cc (50 Hz). Corpus entries (timer: period in cc): Fighting_Soccer 0x2531: 47 610, 0x3038: 61 725,
0x3A44: 74 585, 0x2FE4: 61 305, 0x2BB3: 55 940; jingles 0: 70 940 (speed values 0xE7, 0xF4, 0xF8); P47
0x35C9: 68 850, 0x31F1: 63 930, 0x2E9D: 59 670; Postman Pat 0x37F0: 71 605.

A subsong with an unused voice never has a song end (3.9). Fighting Soccer subsong 9 and the P47 subsongs have
3 voices; all other corpus subsongs have 4.

### 1.5 Note streams

A stream is a byte sequence read from its start; there is no track-list level. A byte below 0x80 is a note
number followed by one duration byte. A byte of 0x80 or more is a command; only its **low 4 bits** select the
command (0x92 and 0xF2 are both command 2).

| low nibble | command | operand bytes | meaning |
|---|---|---|---|
| 0 | rest | 1 (duration) | stop the voice, then wait `duration` ticks |
| 1 | jump | 4 | stream continues at (offset of the first operand byte) + s32 operand |
| 2 | call | 4 | push the stream offset after the operand, then jump as 1 |
| 3 | return | 0 | pop a stream offset and continue there |
| 4 | loop start | 2 | u16 repeat count (big-endian); push (count, offset after the operand) |
| 5 | loop end | 0 | see 3.6 |
| 6 | instrument table | 4 | instrument table = (offset of the first operand byte) + s32 operand |
| 7 | duration high | 1 | pending high part of the next duration: `value * 256` |
| 8 | pitch effect | 2 | step byte, delay byte (3.7) |
| 9 | count | 1 | u8 `n`, used by the pitch effect (3.7, 3.8) |
| 10 to 13 | undefined | 0 | the reference never leaves such a command (endless loop); do not produce them |
| 14 | volume | 1 | AUDxVOL value |
| 15 | voice end | 0 | the voice ends (3.9) |

Jump and call target = offset of the command byte + 1 + signed 32-bit operand. The streams of real modules
consist of a few call/return subroutines per voice, jumps for repeats, and a final end command or an
endless jump.

Corpus statistics (static walk of all reachable streams): the commands used are rest, jump, call, return,
instrument table, count, volume, end and notes. **Loop start/end, duration high and the pitch effect are not
used by any corpus module**; they are specified from synthetic modules.

### 1.6 Instrument table

The offset given by command 6 holds a u16 entry count `n`, then `n` entries of 14 bytes, big-endian:

| off | size | meaning |
|---|---|---|
| 0 | s32 | sample data offset = (offset of this entry) + value |
| 4 | u32 | the low 16 bits (bytes 6 and 7 of the entry) are the length in words for AUDxLEN; the high 16 bits are unused |
| 8 | u16 | base note: the period table index is `note - base` |
| 10 | s16 | lowest note handled by this entry |
| 12 | s16 | highest note handled by this entry |

Corpus tables are "drum kit" style (one sample per note, min = max = base) or small ranges. The first entry,
in file order, whose range contains the note is used. Before the first command 6 of a voice the table is
undefined.

### 1.7 Period table

32-bit big-endian entries holding 16.16 fixed-point periods. The entry for table index `i` is at file offset
`per - 4 * i`: index 0 at `per`, index 1 one entry below it in the file, a negative index above it. Higher
index means higher pitch and a smaller value; the ratio between neighbours is 2^(1/12). Index 0 holds 233.0
(`0x00E90000`) in Fighting_Soccer, 229.0 (`0x00E50000`) in jingles and P47_Thunderbolt and 334.0
(`0x014E0000`) in Postman_Pat; each table is 97 to 132 entries long on either side of index 0. A sample
start uses only the integer part (bits 31 to 16); the pitch effect uses the fraction too.

## 2. State

Per voice `v` (0 to 3):

| field | width | meaning |
|---|---|---|
| `ptr` | offset | next stream byte |
| stack | list | return/loop records (3.6), 4 bytes per call record, 8 bytes per loop record |
| `dur` | u16 | ticks left of the current note or rest |
| flags | 8 bits | bit 0 pitch effect active; bit 1 rest in progress (DMA already stopped); bit 5 running (a note or rest is being timed); bit 6 "end already reported"; bit 7 voice ended; bit 3 unused (set together with bit 7 at the start) |
| `hi` | u16 | pending duration/delay high part, already multiplied by 256 |
| `ins` | offset | current instrument table |
| `n` | u8 | pitch step count (command 9), `div` u8 the divisor derived from it |
| `idx` | s32 | period table index of the last note started |
| `step` | u8 | current pitch effect step; `edel` u16 delay counter; `env` offset of the effect byte stream |

Global: the song word, `speed` (s8), `speedcnt` (u8), four "ended" marks for the song end (3.9), and a 32-bit
**carry word** `C` (3.8), 0 at the start of every tick.

## 3. Replay

### 3.1 Start of a subsong

The host selects subsong `k` (1 to N). Before the first tick it (a) clears all state and sets every voice
flag byte to 0x88 (voice ended, unused), (b) sets the song word to `0xFF00 + k` (bit 15 means "start the song
at the next tick"), (c) loads speed and counter and, when non-zero, the timer value from song table entry `k`,
(d) writes AUDxVOL = 0 for channels 0 to 3 twice (the host's volume set-up, 7.1), (e) writes DMACON = 0x000F.
The first call of the tick routine then performs 3.3.

### 3.2 Speed value

The tick routine is called by the host every `5 * (timer + 1)` colour clocks, however long a call takes.
First a call checks the speed value:

- speed 0: normal tick.
- speed negative (bit 7 set), `speedcnt` = 0: `speedcnt := speed` (as a byte) and **the call does nothing
  else**.
- speed negative, `speedcnt` != 0: `speedcnt := speedcnt + 1` (byte arithmetic), normal tick.

With speed `-s` this plays `s` ticks, then skips one (and the first tick is skipped when `speedcnt` starts at
0). With speed 0xE7 (-25) 25 of 26 ticks play. A **positive** speed (1 to 127) crashes the reference (it
returns to a wrong address after running the tick body twice); no corpus module has one and the behaviour is
not specified.

### 3.3 Song start (the first tick that plays)

If bit 15 of the song word is set: write DMACON = 0x000F; the song word := its low 4 bits; use song table
entry `song word` (so subsong numbers above 15 alias); for each voice 0 to 3 in order: flags := 0x88; if the
stream is valid (1.4) then `ptr` := its offset, flags := 0x01, `dur` := 0, `hi` := 0; the voice stack is empty.
Then write AUD0VOL, AUD1VOL, AUD2VOL, AUD3VOL = 0 and DMACON = 0x000F. The same call continues with the voice
pass (3.4).

(Flags 0x01 has the pitch bit set, but the fetch of the first event clears bits 0, 1 and 5, so it has no
effect.)

### 3.4 Voice pass

For voice 0, 1, 2, 3 in order (DMACON bit `m` = 1, 2, 4, 8; Paula channel base `0xDFF0A0 + 0x10 * v`):

1. **Ended voice** (flags bit 7): if flags bit 6 is clear, set it and write DMACON = `m`. Nothing else; the
   pass for this voice is over.
2. **Running voice** (bit 5 set): go to step 5.
3. **Fetch** (bit 5 clear): if flags bit 1 is clear, write DMACON = `m`; clear flags bits 0, 1 and 5. Then
   read events as in 3.5 until one sets a duration or ends the voice. A voice that ended is over.
4. After a note or rest event has set the duration (3.5): continue with step 5.
5. **Duration step**: if `dur` = 0, go to step 3 (this is how zero-length events chain inside one tick).
   Otherwise `dur := dur - 1`; if that makes it 0, flags &= 0xDC (clears bits 0, 1, 5) and write
   DMACON = `m`. Then, if flags bit 0 is set, run the pitch effect (3.7).

A note of duration `d >= 1` therefore starts in tick `t` and its DMACON stop is written in tick `t + d - 1`
(with `d = 1` it stops in the same tick as it starts, after the start sequence). In tick `t + d` the voice
fetches again and writes a further DMACON stop at once (bit 1 is clear) before reading the next event.

### 3.5 Events

An event reads the byte at `ptr` and advances `ptr` past it.

- **Command** (>= 0x80): by its low nibble (1.5, 3.6).
- **Rest** (nibble 0): set flags bit 1, write DMACON = `m`, then *set duration*.
- **Note** (< 0x80): look at the instrument table `ins` (entry count `n`). For each entry in order skip it if
  `note < min` or `note > max` (signed 16-bit compares). If none matches, including `n` = 0, no sound is
  started: *set duration*. Otherwise, for the first match:
  1. `idx := note - base` (32-bit signed, `base` unsigned 16-bit).
  2. Sample address = file offset of the entry plus the s32 at the entry; length = the 16-bit word at entry + 6.
  3. `P` := period table entry at `per - 4 * idx`; `period := P >> 16`.
  4. Write AUDxLC = absolute address of the sample (both halves, same clock), AUDxLEN = length, AUDxPER =
     `period`.
  5. Raster wait (5.5); write DMACON = `0x8200 | m`; raster wait.
  6. For voices 0, 1 and 2 always, and for voice 3 only when bit 6 of the low byte of voice 0's stack pointer
     is clear (3.10): write AUDxLC = the silent block (a 4-byte zero block in the player's data) and AUDxLEN =
     1. Every sample is thereby a one-shot: after one pass the channel plays the silent block.
  7. The carry word: `C := (P << 16) | (P >> 16)` (the entry rotated by 16 bits).
  Then *set duration*.
- **Set duration**: `dur := hi | b`, where `b` is the next stream byte (the duration byte; for a note, the byte
  after the note number; for a rest, the byte after the command); `hi := 0`; `ptr` moves past `b`; flags bit
  5 := 1. The duration step (3.4 step 5) follows at once in the same tick. A duration of 0 therefore makes the
  voice fetch the next event in the same tick (after a DMACON stop, except after a rest).

### 3.6 Commands

- **1 jump**: `ptr := (offset of the operand) + s32 operand`. Continue reading events.
- **2 call**: push the offset after the operand (4 bytes of stack), then as jump.
- **3 return**: pop into `ptr`.
- **4 loop start**: `count :=` the u16 operand; push (`count`, offset after the operand), 8 bytes of stack;
  continue after the operand.
- **5 loop end**: pop (`count`, `start`); `count := count - 1` (16-bit); if it is 0, continue after the loop-end
  byte; otherwise push (`count`, `start`) back and continue at `start`. (A start count of 0 runs 65 536
  times.)
- **6 instrument table**: `ins := (offset of the operand) + s32 operand`.
- **7 duration high**: `hi := operand * 256`.
- **8 pitch effect**: if flags bit 0 is clear: set bit 0; `step := operand 1`; `edel := hi | operand 2`; `hi := 0`;
  `env :=` the offset after the two operand bytes. If bit 0 is already set: only skip the two operand bytes.
  Either way the stream continues after them. (The effect bytes of 3.7 are the stream bytes that follow.)
- **9 count**: `n := operand`. If `n != 0`, `div` is computed as in 3.8 (and `C` changes); if `n` = 0 `div` stays.
- **14 volume**: write AUDxVOL = `operand & 0x7F` (before the host's gain, see below). Voices 0 and 3 are
  scaled by the host's left gain, voices 1 and 2 by the right gain: `v := (v * gain) >> 6`; gains are 64 in all
  logs, so the value is written unchanged (a value above 64 is written as it is).
- **15 voice end**: song end test (3.9), then flags bit 7 := 1; the pass for this voice is over.
- **10 to 13**: undefined.

### 3.7 Pitch effect

Runs only while flags bit 0 is set, at the end of the duration step (3.4 step 5). In each such tick:

1. If `edel` != 0: `edel := edel - 1`; if still non-zero, nothing more this tick.
2. Compute and write a period:
   - `t := idx - n` (32-bit signed), `r := step`.
   - While `r` taken as a signed byte is **not less than** `div` taken as a signed byte: `r := r - div` (byte
     arithmetic), `t := t + 1`. (With `div` = 0 and a non-negative `r` this never ends: the reference hangs.)
   - `w := (n * r) & 0xFFFF`.
   - `Q := entry(t)`, `Qn := entry(t - 1)` (the next entry towards larger periods: at `per - 4 * t + 4`).
     `diff := (Qn - Q) mod 2^32`, `hiw := diff >> 16`, `low := diff & 0xFFFF`.
   - `x := (((hiw * w) & 0xFFFF) << 16) | ((low * w) & 0xFFFF)`; then `x := x >> 6`. (The carry from the low
     product into the high part is lost; this reproduces the reference.)
   - `Z := (Q - x) mod 2^32`; write AUDxPER = `Z >> 16`; `C := (Z << 16) | (Z >> 16)` (32 bit).
3. Read bytes at `env`, advancing:
   - `0xF7`, then `b`: `hi := b * 256`; read the next byte.
   - `0xF8`, then `s`, `d`: `step := s`; `edel := hi | d`; `hi := 0`; `env :=` the offset after `d`; this tick's
     effect is done.
   - any other byte: clear flags bit 0 (the effect ends; the last period stays).

The effect bytes lie in the note stream right behind the command-8 operand, where the stream parser reads them
too: the byte 0xF7 is command 7 (it sets `hi`), the byte 0xF8 is command 8 (skipped while the effect is
active). Modules put `0x88 s d [0xF7 b] 0xF8 s d ...` in front of a note.

### 3.8 The carry word and the divisor of command 9

The divisor for command 9 is computed with a 32-bit division whose dividend is `(C & 0xFFFF0000) | 0x40` and
whose divisor is `n`:

- If the quotient is above 65 535 the division fails: `div := 0x40` and `C := (C & 0xFFFF0000) | 0x40`.
- Otherwise `div :=` the low 8 bits of the quotient and `C := (remainder << 16) | quotient`.

`C` is 0 at the start of every tick; it is also set by a note start (3.5 step 7) and by the pitch effect (3.7).
It matters only to the pitch effect. With `C` = 0 the divisor is `floor(64 / n)` (0 for `n` > 64, which
hangs the effect).

### 3.9 Song end

Four marks are set at the start of a subsong. Command 15 in voice `v` clears mark `v`; when all four are
zero the host's song end function is called, once, and all marks are set again. A voice that has no stream
never clears its mark, so a subsong with fewer than 4 voices never ends. The call itself only costs cycles
(5.4). After the call the reference host runs three more ticks and then ends the run.

A voice that has executed command 15 writes DMACON = `m` once in a later pass (3.4 step 1) and nothing
afterwards. Corpus song ends: Fighting_Soccer subsongs 3, 4 and 8, jingles subsongs 1 to 5, Postman_Pat
subsong 2.

### 3.10 Voice 3 and the stack of voice 0

Note start step 6 tests **bit 6 of the low byte of voice 0's stack pointer** to decide whether voice 3 gets
the silent one-shot. Each voice's stack is a 128-byte area inside the player's data that fills downwards;
voice 0's pointer starts with low byte 0x1E in the reference host, and each pushed call record lowers it by
4, each loop record by 8. The bit is set exactly when voice 0's stack holds from 31 to 94 bytes (8 to 23 call
records) and clear for 0 to 30 and for 95 to 128 bytes. Deeper nesting overwrites the neighbouring data
(unspecified). In the corpus voice 0's stack is at most 1 call record deep, so voice 3 always gets
the one-shot there (the other case is covered only by synthetic modules).

## 4. Paula write order

Within a call the writes follow the voice pass, voice 0 to 3. Per voice:

| situation | writes, in order |
|---|---|
| song start | DMACON 0x000F, AUD0VOL..AUD3VOL = 0, DMACON 0x000F, then the normal pass |
| ended voice, first time | DMACON `m` |
| fetch | DMACON `m` (unless flags bit 1), then per event: volume: AUDxVOL; rest: DMACON `m`; note: AUDxLC (two words), AUDxLEN, AUDxPER, DMACON `0x8200\|m`, then (if 3.5 step 6 applies) AUDxLC, AUDxLEN |
| duration expiry | DMACON `m` |
| pitch effect | AUDxPER |

No other register is written.

## 5. Timing

All costs are 68000 cycles. A counter `T` starts at 0 when the host calls the tick routine. A register write
is stamped `call + floor(T / 2)`, where `T` includes the cost of the writing operation itself (that cost is
part of the step that makes the write, in the tables below). The two halves of a 32-bit AUDxLC write get the
same stamp. All costs are static, including taken and not-taken branches.

### 5.1 Frame

The host calls the tick routine at colour clock `t0 + k * 5 * (timer + 1)`, `k = 0, 1, ...`; `t0` is a host
property (LOGS.md lists the values; it differs per subsong). The raster waits (5.5) depend on the absolute
clock, so a port needs the real call clock. A call that overruns the next call time was not seen (the longest
call in the corpus is 23 025 cc, the shortest period 47 610).

### 5.2 Skeleton

| step | cycles |
|---|---|
| host entry wrapper until the tick routine starts | 242 |
| tick routine entry | 113 |
| speed value non-zero (negative): extra | 30, then 26 (either outcome); a skipped tick ends here (the remaining 108 cycles of return are not needed) |
| song word test | 39 |
| song start only: DMACON 0x000F write | 20 |
| song start only: set-up, then per voice (unused / marker 0x1EF8 / offset not above the file start / valid) | 130, then 91 / 114 / 169 / 229 each |
| song start only: AUD0VOL, AUD1VOL, AUD2VOL, AUD3VOL = 0 writes | 20 each |
| song start only: DMACON 0x000F write | 20 |
| voice pass set-up | 24 |

Then the four voice passes (5.3); each ends with 39 cycles.

### 5.3 Voice pass

| step | cycles |
|---|---|
| slot head (every voice) | 25 |
| ended voice (bit 7): test | 31; if bit 6 was clear: DMACON write 16, then 10; the pass ends (39) |
| not ended: running test | 27 (if running: duration step) |
| fetch head | 27; then DMACON `m` write 16 if flags bit 1 is clear; then 60 |
| each event byte read | 39 |
| command byte: dispatch | 62 (after the 39), then the cost in 5.4 |
| duration step: zero test | 23 (if `dur` = 0: fetch head again) |
| duration step: decrement | 25; if it reached 0: 20, then DMACON `m` write 16 |
| pitch test | 27 (if flags bit 0 is set: 5.6) |
| set duration | 80 |
| end of the pass of one voice | 39 |

Rest event: 39, 62, then 20, DMACON write 16, 10, then set duration (80), then the duration step.

Note event: 39 for the note byte; then 39; if the entry count is non-zero 4, then per entry tested: 41 if it is
skipped because `note < min`, 62 if skipped because `note > max`, 44 for the matching one (the search ends).
If no entry matches: 10, then set duration. With a match, in order:

| step | cycles |
|---|---|
| 52, 48, 108, 4 | 212 |
| AUDxLC write | 12 |
| AUDxLEN write | 12 |
| AUDxPER write | 12 |
| 18 + SETALL(v), then 18, then raster wait (5.5) | |
| 12, DMACON `0x8200\|m` write | 16 |
| 18, then raster wait | |
| 53 | |
| if 3.5 step 6 applies: 12, AUDxLC write 12, AUDxLEN write 16 | |
| 12 | |

then set duration. SETALL(v) = 107, 138, 169, 177 for v = 0, 1, 2, 3. A count of 0 skips the 4 and the entry
loop.

### 5.4 Command costs (cycles after the 39 + 62)

| command | cycles |
|---|---|
| 1 jump | 142 |
| 2 call | 182 |
| 3 return | 74 |
| 4 loop start | 126 |
| 5 loop end: finished / repeating | 109 / 139 |
| 6 instrument table | 158 |
| 7 duration high | 68 |
| 8 pitch effect: started / already active | 131 / 59 |
| 9 count: `n` = 0 / `n` != 0 | 59 / 203 |
| 14 volume | 30, then VOLB(v), then the AUDxVOL write 12, then 28 + 26 |
| 15 voice end | 18 + 32 + CHAIN(v) + 21 (+ 68 + 16 when this completes the song end) + 44 + 20 + 10 |

VOLB(v) = 169, 192, 215, 205 for v = 0, 1, 2, 3. CHAIN(v) = 45, 72, 95, 108.

### 5.5 Raster wait

A note start waits twice for nine changes of the video line number. Let `clock = floor(cycles / 2)` with cycles
counted from system start (equal to `2 * call + T`) and `line = floor(clock / 227)` (227 colour clocks per
line); only the low 8 bits of the line are compared. The wait:

1. 28 cycles of entry.
2. Nine rounds. In each: one read of the line counter (16 cycles; the value is the line at the end of those 16
   cycles); then repeat { a read (16 cycles; line taken at its end), then 9 cycles } until the line read
   differs from the first read of this round; then 10 cycles.
3. 44 cycles of exit.

Over the corpus the span from the AUDxPER write to the DMACON write is 1 963 to 2 224 cc, and similar from the
DMACON write to the silent-block AUDxLC write.

### 5.6 Pitch effect cost

The pitch test (27) is followed, when flags bit 0 is set, by: 21 (delay test); if `edel` != 0, 27 for the
countdown, and if it is still non-zero the pass ends. Then 52; per test of the division loop 21 and per
subtraction 30; 32; (`n` + 1) * 14; 40; 90; 192; AUDxPER write 12; 16; then per effect byte read: 29; for 0xF7
52 more and the next byte follows; otherwise 17 and, for 0xF8, 90 more (done); for any other byte 20 more
(the effect ends).

## 6. What is not in this format

There is no vibrato, envelope, arpeggio, filter control or sample modification. The only writes outside note
starts are the volume command, the DMACON stops and the pitch effect.

## 7. Reference-host facts

### 7.1 Start sequence

Before the first tick the reference run performs: the volume set-up twice (AUD0VOL..AUD3VOL = 0 each time),
then DMACON 0x000F. Relative to the first write `ts` (host dependent: 9 600 for Fighting_Soccer, 6 663 for
jingles, 6 975 for P47_Thunderbolt, 5 456 for Postman_Pat): channel 0 volume at `ts`, channel 1 at `ts + 138`,
channel 2 at `ts + 288`, channel 3 at `ts + 432`; the second set at `ts + 686`, `+ 824`, `+ 974`, `+ 1 118`;
DMACON 0x000F at `ts + 6 240`.

For `--subsong k` with k > 0 (0-based host numbering) the run contains k extra passes in front of the real
one. Each extra pass is the start sequence above, followed by a song-start burst (DMACON 0x000F at about
`ts + 6 576`, then the four volume writes 8, 14, 20 and 26 cc after it), and the next pass begins about 6 830 cc
after the previous one began (6 827 to 6 833 for three corpus modules, irregular for Fighting_Soccer and for
some P47 subsongs). These are host artefacts of the reference run, not part of the replay. Compare
only the ticks (the call times in logs/*.calls); LOGS.md lists the measured first-call times.
