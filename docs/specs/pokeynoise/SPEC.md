# Pokeynoise (Atari 8-bit SAP / binary-load player, prefix `pn`): behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Eagleplayer "POKEYNoise V0.25alpha" by Achim Haertel (UADE player file `Pokeynoise`).
It plays Atari 8-bit music: the module is Atari 6502 machine code plus data. The player loads it into
a 64 KB emulated Atari memory, runs its 6502 routines in a built-in interpreter once per tick, then
reads the four POKEY channel registers back from that memory and turns each into one Paula voice
playing a waveform from a fixed bank that the player carries.

This text describes behaviour only. It was derived from the reference 68k player and checked with
write logs (LOGS.md). Colour clocks are written "cc" (one PAL line = 227 cc, one tick = 70 940 cc).

Status (checked by a private model written from this text, against the reference):

| item | result |
|---|---|
| 6 real SAP files (types B and C), 11 subsong runs, about 101 000 Paula writes | value and order of every write equal the reference |
| a real TYPE D file and a synthetic TYPE R file | rejected at initialisation, as specified |
| 7 synthetic SAP files, 9 runs (random and swept POKEY settings, CPU behaviour probes, types M and C, hex SONGS/DEFSONG), about 468 000 writes | same; all 33 reachable waveform cases, the random silencing and RANDOM reads included |
| timing | per-write gaps and waits measured (section 6); start of a tick's writes depends on the emulated 6502 run time, given only as an estimate |

## 1. Module files

### 1.1 Detection

A file is accepted by the detection step when either

- its first 16-bit word is `$FFFF` (an Atari binary-load file, "raw" below) and it has a companion
  icon (`<file>.info`) whose tool type `TYPE` is exactly `POKEY`; without such an icon the file is
  rejected; or
- its first three bytes are the ASCII text `SAP` (a SAP file).

The first test wins: a file starting `$FF $FF` is never treated as SAP. Everything else (TYPE value,
required keywords) is checked only when the module is initialised; a failure there makes the
module unplayable (the host reports that the player's initialisation failed).

### 1.2 SAP header

The header is the text from the start of the file up to (not including) the first byte pair
`$FF $FF` anywhere in the file. A SAP file without such a pair fails.

Keywords are found by a plain, case-sensitive substring search over the header: the first place
where the keyword's letters occur, anywhere (also inside another line such as a NAME string),
counts. After the keyword, spaces and tabs are skipped and the value starts.

Numbers (`INIT`, `PLAYER`, `MUSIC`, `SONGS`, `DEFSONG`) are always read as hexadecimal, including
`SONGS` and `DEFSONG` (so `SONGS 10` means 16 songs). Reading stops at a comma, NUL, LF or CR; it
does not stop at a space. Each character `ch` becomes a digit value `d = (ch - 0x30) mod 256`; if
`d >= 0x31` then `d -= 0x27`, else if `d >= 0x11` then `d -= 7` (so `0-9`, `A-F` and `a-f` give 0..15;
any other character gives some other value). The number is built as `n = (n << 4)` with the low
8 bits then or-ed with `d` (32-bit arithmetic; addresses use the low 16 bits).

`TYPE`: the first character of its value decides:

| TYPE | handling |
|---|---|
| `B` or `M` | `PLAYER` (required) is the play address, `INIT` (required) the init address. B and M are treated the same. |
| `C` | `PLAYER` (required) is the base address of the player; the init entry is `PLAYER + 3`, the play entry `PLAYER + 6`. `MUSIC` (required) is the music data address. |
| anything else (`R`, `D`, `S`, ...) or no `TYPE` | the module fails to initialise. |

`SONGS` (optional, default 1) gives the number of subsongs; `DEFSONG` (optional, default 0) the
default subsong, 0-based, used as is. All other header lines (`FASTPLAY`, `STEREO`, `NTSC`, `TIME`,
`AUTHOR`, `NAME`, `DATE`, ...) are ignored: there is no title extraction, the tick rate is never
changed and a second POKEY is never read.

### 1.3 Binary part

For SAP files the binary part starts at the first `$FF $FF` (the header end); for raw files at the
file start. It is a sequence of segments, read until the file ends:

- if the next two bytes are `$FF $FF` they are skipped (optional marker, allowed before any segment);
- start address (16-bit little endian), end address (16-bit little endian, inclusive);
- data bytes, copied to the emulated memory from the start address upwards. Copying stops when the
  address just written equals the end address, or when the file ends. Addresses wrap at 16 bits; a
  segment whose end is below its start copies exactly one byte; an end of `$FFFF` copies to the end
  of the file (wrapping through address 0).

A segment header cut off by the end of the file reads past the data (no check). Nothing is executed
while loading (no `$02E2` init calls).

### 1.4 Emulated memory

64 KB, all zero before loading, flat: every address is plain RAM to the 6502 apart from the two
special reads of section 3.5. The player also keeps per-channel history inside this memory
(section 4.4). The memory is not cleared again when a subsong is (re)started.

### 1.5 Raw binary-load files (with icon)

Parameters come from the icon's tool types (only these are used):

- `SONGS=a,b,c` (optional, hexadecimal, fields separated by commas): maximum subsong `a - 1`,
  default `b - 1`, minimum `c - 1` (each subtraction acts on the low 8 bits of the value). Missing
  fields leave 0.
- `AUDIOCORRECT=RH` switches the high-tone mute of section 4.6 on (it is off for raw files
  otherwise; for SAP files it is always on).

The init address is the 16-bit word at memory `$02E0` (the Atari run address) after loading. After
the init call the play address is the 16-bit little-endian word at `$0222` (the immediate vertical
blank vector), which the init code normally sets through the set-vector entry (section 3.4).

Raw files are described from the player's logic only; no run of the reference with an icon was made
(see README).

## 2. Host interface

- Subsongs: minimum 0, maximum `SONGS - 1`, default `DEFSONG` (SAP), or as in 1.5.
- Tick: the player does not set a tick rate; it runs at the host's default 50 Hz (70 940 cc). See
  6.5 for ticks that overrun.
- Song start (each time a subsong is started; the 6502 runs of 3.1 apply):
  - type B/M and raw: call the init address with A = subsong, X = Y = 0;
  - type C: call `PLAYER + 3` with A = `$70`, X = low byte of MUSIC, Y = high byte; then call
    `PLAYER + 3` again with A = 0, X = subsong, Y = 0;
  - if a call fails (3.2) the song does not start;
  - raw only: read the play address from `$0222/$0223` now;
  - each of the four voice records (4.8) gets waveform = bank offset 0 (silence) and length 4 words;
    period, volume and the period-changed flag keep their previous values (0 at first load).
- Every tick:
  1. increment the 24-bit Atari real-time clock: byte `$14` + 1; if it wrapped to 0, `$13` + 1; if
     that wrapped, `$12` + 1;
  2. call the play address with A = X = Y = 0;
  3. if the call failed: nothing else happens this tick (no Paula writes);
  4. otherwise convert the four POKEY channels (section 4), then write Paula (section 5).
- Song stop: for each voice 0..3: location = bank offset 0, length 4, period `$100`, then volume 0
  and data 0 written together as one 32-bit write (logged as VOL then DAT). DMA is not touched.
- There is no song-end detection, no volume or balance support, no position or pattern information.
- Switching subsongs keeps the whole emulated memory (module variables, POKEY values, history),
  the previous AUDCTL (4.4) and the random generator state (4.5).

## 3. 6502 execution

### 3.1 A call

At the start of every call: the byte at `$D01E` is set to `$01` and the byte at `$D40F` to `$40`
(the module may read them). Registers: A, X, Y as given by the caller; stack pointer S = `$FF`; all
flags clear (P = `$20`, only the unused bit set). Execution starts at the given address and runs,
without any instruction or cycle limit, until one of these:

- success: an `RTS` that pulls its return address from above the initial stack top (the routine's
  final return); an `RTI` doing the same; a `JMP` to `$E462` or `$E45F` (the OS vertical-blank exit
  vectors) either absolute or indirect;
- failure: an undocumented opcode (all 105 of them), a push or `JSR` below the bottom of the stack
  page, a `PLA` or `PLP` with the stack already at its top.

An endless loop in the module hangs the player.

### 3.2 Stack

The stack lives at `$0100-$01FF` but is not a wrapping 8-bit stack. Think of a pointer `sp`
(16 bits) that starts at `$0200` and always points at the last pushed byte; S = `sp - $101`
(low 8 bits, as seen by `TSX`); `TXS` sets `sp = $101 + X`.

- push (PHA, PHP): `sp -= 1`, store; then if `sp < $0100` the call fails (the byte is already
  stored at `$00FF`).
- `JSR`: return address minus one is stored, low byte at `sp - 2`, high byte at `sp - 1`, then
  `sp -= 2`; if `sp < $0100` the call fails.
- pull (PLA, PLP): if `sp > $0200` the call fails; else read at `sp`, `sp += 1`. A pull at
  `sp = $0200` (empty stack) reads the byte at `$0200`.
- `RTS`: read low byte at `sp`, high byte at `sp + 1`, `sp += 2`; if now `sp > $0200` the call ends
  (success); else continue at the address read plus one.
- `RTI`: pull P (as `PLP`, without the check), read the address as `RTS` does, `sp += 2`; if
  `sp > $0200` the call ends; else continue at the address read (no plus one).

### 3.3 Instruction set

All 151 documented opcodes, with the usual 6502 addressing and results, except:

- No decimal mode: `SED` sets the D flag, but `ADC` and `SBC` always work in binary.
- `BRK` does not interrupt: it skips the byte after it (a two-byte no-op).
- Zero flag after `ADC` and `SBC`: when the 8-bit result is non-zero, Z = 0; when it is zero, Z keeps
  the value it had when the carry was last set by an instruction of this list: `ADC`, `SBC`, `CMP`,
  `CPX`, `CPY`, `ASL`, `LSR`, `ROL`, `ROR` (any mode), `PLP`, `RTI` (Z from the pulled P), and at the
  start of a call (Z = 0). `CLC`, `SEC`, `INC`, `DEC`, loads, logic operations and transfers do not
  change that remembered value. (So adding 1 to `$FF` with the carry clear gives Z = 0 unless, for example,
  the last compare found equality.) N, V and C after `ADC`/`SBC` are correct. Every other instruction sets
  Z normally.
- `PHP` pushes bit 4 (B) as it is in P, which is 0 unless a `PLP`/`RTI` loaded a 1; bit 5 is always 1.
- Indirect zero-page addressing `(zp,X)` and `(zp),Y` reads the pointer's high byte from `zp + 1`
  without wrapping: a pointer at `$FF` takes its high byte from `$0100`.
- `JMP ($xxFF)` takes the high byte from `$xx00` (the real 6502 page bug).
- Program counter and branches are not wrapped at `$FFFF` (undefined; no module needs it).

### 3.4 Special addresses

- `JSR $E45C` (OS set-vector): if A = 6, store Y at `$0222` and X at `$0223`; if A = 7, store Y at
  `$0224` and X at `$0225`; otherwise nothing. Nothing is pushed; execution continues after the JSR.
- `JMP $E45C` (absolute only): the same stores, then behave as `RTS`.
- `JMP $E462`, `JMP $E45F`, and `JMP (ind)` arriving at either: end of call (success). `JSR` to them
  is not special.
- No other OS entry points exist; the memory there is whatever the module loaded (normally zero,
  i.e. `BRK` pairs).

### 3.5 Hardware reads

Only these read forms are checked: `LDA` absolute, absolute,X, absolute,Y, (zp,X), (zp),Y; `LDX`
absolute, absolute,Y; `LDY` absolute, absolute,X; `CMP`, `CPX`, `CPY` absolute. When their effective
address is in `$D000-$D7FF`:

- `$D20A` (RANDOM): one step of the random generator of 4.5, value = bits 5..12 of its result
  (`(r & $FFFF) >> 5`, low 8 bits);
- `$D40B` (VCOUNT): the Amiga's current raster line divided by 2 (lines 0..312) at the moment of the
  read;
- anything else: the memory byte.

All other instructions (also `BIT`, `AND`, `ORA`, `EOR`, `ADC`, `SBC`, the other `CMP` modes,
`INC`/`DEC` and shifts) read plain memory. All writes are plain memory writes (writing POKEY,
`WSYNC` and so on has no side effect; the values just stay in memory).

## 4. POKEY to Paula conversion

### 4.1 Inputs

For channel k = 0..3: AUDF = byte at `$D200 + 2k`, AUDC = byte at `$D201 + 2k`; AUDCTL = byte at
`$D208`. "Previous AUDF" means the byte at `$D200 + 2k - 2` (for channel 0 that is `$D1FE`, an
ordinary memory byte). The result of a channel is a tuple (waveform offset, length in words,
period, volume).

### 4.2 Volume-only and silence

If AUDC bit 4 is set, the channel gives SILENCE = (offset 0, length 4, period `$100`, volume 0),
and nothing else of this section is done for it (history of 4.4 not touched). Otherwise
`vol = AUDC & 15` and the distortion `dist = AUDC >> 5`.

### 4.3 Clock mode and frequency value

Per channel two properties are derived from AUDCTL; note that they deliberately follow the
player, not the real POKEY:

| channel | clock mode | joined |
|---|---|---|
| 0 | bit 0 set: A, else B; then if bit 6: (bit 4 ? C : D) | bit 7 |
| 1 | bit 0 set: A, else B; then if bit 6: C | bit 4 or bit 7 |
| 2 | bit 0 set: A, else B; then if bit 5: (bit 3 ? C : D) | bit 7 |
| 3 | bit 0 set: A, else B; then if bit 5: C | bit 3 or bit 7 |

and, when AUDCTL bit 7 (the 9-bit poly flag) is set, the mode is then changed: A becomes C, B
becomes D (C and D stay). The high-pass bits have no effect.

`M` = AUDF if not joined, else `AUDF * 256 + previous AUDF`. Frequency value `f` (units: 1/256 of a
64 kHz step) and divider count `c` (1.79 MHz cycles, used only for dist 2 and 6):

| mode | f for dist 2 and 6 | c (dist 2, 6) | f for the other dists |
|---|---|---|---|
| A | `floor((M + 1) * 29184 / 28)` | `(M + 1) * 114` | same as dist 2/6 |
| B | `(M + 1) * 256` | `(M + 1) * 28` | same |
| C | `floor((M + 7) * 256 / 28)` | `M + 7` | `floor((M * 256 + 7) / 28)` |
| D | `floor((M + 4) * 256 / 28)` | `M + 4` | `floor((M * 256 + 4) / 28)` |

### 4.4 History and change detection

Each converted channel (not volume-only) compares with and then overwrites a history kept in the
emulated memory: a tag byte at `$D260 + 2k`, `f` as a 16-bit big-endian word at `$D240 + 2k` (low
16 bits of f), and `vol` as a 16-bit big-endian word at `$D250 + 2k`. Tags: dist 0 → 0, 1 and 3 → 1,
2 → 2, 4 → 4, 5 and 7 → 5, 6 → 6. "changed" = the stored tag differs from this channel's tag, or the
stored f word differs from the low 16 bits of f. (The player also checks AUDCTL against the value of
the previous tick and the volume, but nothing depends on that outcome.) After all four channels the
current AUDCTL is remembered as "previous AUDCTL" (outside the emulated memory, initially 0).

### 4.5 Random generator

Two 32-bit words, initially `s1 = $3F3F751F`, `s2 = $17179834`, kept for the life of the loaded
player (not reset by song start). One step:

1. `r = s1 + s2` (mod 2^32); `s1 = r`;
2. rotate `r` right by 4 bits (32-bit);
3. the low 16 bits of `r` become `(low16(r) - high16(s2))` mod 2^16 (the upper 16 bits unchanged);
4. `r ^= (J << 16) | J` where J is the host's JOY1DAT register (reads 0 in this host);
5. `s2 ^= r`; `s1 = s1 + $56565311` (mod 2^32); the step's result is `r`.

Steps are taken by RANDOM reads (3.5) and by the "random silence" test below, in execution order.

Random silence (only where the table says so): if "changed" is true, take one step; if the low
8 bits of the result are below `$50`, the channel gives SILENCE. If "changed" is false no step is
taken.

### 4.6 Cases

Notation: `P(x, k) = floor(x * k / 256)` (if `x * k` reaches 2^32 the result is 0; not reachable in
practice); `lo(x) = max(x, 124)`; `hi(x) = min(x, 65535)`; divisions are integer divisions.
`AC` = the high-tone mute (on for SAP files): when on and `f < $2F0`, the volume becomes 0 (the
waveform and period are still chosen normally). Waveform offsets are byte offsets into the bank
(section 7), lengths in 16-bit words. Ranges are tested top to bottom; "f ≤ X" means the first
row whose condition holds.

dist 0 (poly5 + poly17):

| condition | waveform | length | period |
|---|---|---|---|
| any | `$277E` | 10247 | `lo(hi(P(f, $89)))` |

dist 4 (poly17 noise), with AC:

| condition | waveform | length | period |
|---|---|---|---|
| any | `$778C` | 9151 | `lo(hi(P(f, $84)))` |

dist 5 and 7 (pure tone), with AC:

| condition | waveform | length | period |
|---|---|---|---|
| f ≤ `$500` | `$0008` | 1 | `P(f, $38)` |
| f ≤ `$C00` | `$000A` | 2 | `P(f / 5, $8C)` |
| f ≤ `$C000` | `$000E` | 4 | `P(f / 10, $8C)` |
| else | `$0016` | 13 | `P(hi(2f / 65), $8C)` |

dist 1 and 3 (poly5):

| condition | waveform | length | period |
|---|---|---|---|
| f ≤ `$280` | `$01A4` | 11 | `lo(P(f, $9E))` |
| f ≤ `$A00` | `$0030` | 31 | `P(2f / 5, $8C)` |
| f ≤ `$2000` | `$006E` | 62 | `P(f / 5, $8C)` |
| else | `$00EA` | 93 | `P(hi(2f / 13), $8C)` |

dist 6 (poly4): `r = c mod 15`.

| r | condition | waveform | length | period |
|---|---|---|---|---|
| 0 | | SILENCE | | |
| 5, 10 | random silence first, then: f ≤ `$A00` | `$030A` | 3 | `lo(P(2f / 5, $8C))` |
| | f ≤ `$2000` | `$0310` | 3 | `P(f / 5, $8C)` |
| | else | `$0316` | 6 | `hi(P(f / 10, $8C))` |
| 3, 6, 9, 12 | f ≤ `$A00` | `$0322` | 5 | `lo(P(2f / 5, $8C))` |
| | f ≤ `$2000` | `$032C` | 5 | `P(f / 5, $8C)` |
| | else | `$0336` | 10 | `hi(P(f / 10, $8C))` |
| other | f ≤ `$280` | `$01BA` | 3 | `lo(P(f, $8C))` |
| | f ≤ `$A00` | `$01C0` | 15 | `P(2f / 5, $8C)` |
| | f ≤ `$1800` | `$01DE` | 15 | `P(f / 5, $8C)` |
| | f ≤ `$3800` | `$01FC` | 30 | `P(f / 10, $8C)` |
| | else | `$0238` | 105 | `hi(P(f / 35, $8C))` |

dist 2 (poly5 + poly4): first `c mod 31`, then `r = c mod 15`.

| case | condition | waveform | length | period |
|---|---|---|---|---|
| c mod 31 = 0 | random silence first, then: f ≤ `$280` | `$1136` | 5 | `lo(P(f, $8C))` |
| | else | `$1140` | 10 | `hi(P(f / 10, $8C))` |
| r = 0 | | SILENCE | | |
| r = 5, 10 | random silence first, then: f ≤ `$A00` | `$0B8E` | 93 | `lo(P(2f / 5, $8C))` |
| | f ≤ `$2000` | `$0C48` | 93 | `P(f / 5, $8C)` |
| | else | `$0D02` | 186 | `hi(P(f / 10, $8C))` |
| r = 3, 6, 9, 12 | f ≤ `$280` | `$0E76` | 42 | `lo(P(5f / 7, $90))` |
| | f ≤ `$A00` | `$0ECA` | 155 | `lo(P(2f / 5, $8C))` |
| | else | `$1000` | 155 | `hi(P(f / 5, $8C))` |
| other r | f ≤ `$280` | `$0390` | 93 | `lo(P(f, $8C))` |
| | f ≤ `$A00` | `$044A` | 465 | `P(2f / 5, $8C)` |
| | else | `$07EC` | 465 | `hi(P(f / 5, $8C))` |

Mind where `hi` sits: for dist 5/7 and 1/3 the last row clamps before scaling, elsewhere after.
Periods are 16-bit values; tone periods below 124 are written as computed.

### 4.7 Volume

Paula volume = `floor(vol * 1093 / 256)`: 0, 4, 8, 12, 17, 21, 25, 29, 34, 38, 42, 46, 51, 55, 59, 64.
(Zero where SILENCE or AC applies.)

### 4.8 Voice records

Each Paula voice keeps: waveform offset, length, period, volume, a "period changed" flag, a
"restart" flag. Per tick, with the channel's new tuple:

- if the waveform offset differs from the stored one: restart = yes, period changed = yes, store
  offset and length; else restart = no (the length is not compared);
- if the period differs from the stored one: store it, period changed = yes;
- store the volume.

## 5. Paula writes per tick

Voices 0 to 3 in order (voice n = channel n, DMA bit `1 << n`):

1. if restart:
   1. wait for a new raster line, then until the horizontal position is at least 22 cc; write
      period = 1;
   2. read DMACON: if this voice's DMA is on, write INTREQ = `bit << 7`, write DMACON = `bit` (off),
      then wait until INTREQR has `bit << 7` set; if it is off, write INTREQ = 0 and DMACON = 0 and
      do not wait;
   3. write location (32-bit, bank base + offset), length, period, volume (stored values);
   4. wait for a new raster line and position ≥ 22 cc; DMA is now off, so write INTREQ = `bit << 7`
      and DMACON = `$8000 | bit` (on); wait until INTREQR has `bit << 7` set;
   5. wait for a new raster line and position ≥ 22 cc; write location and length again (same
      values);
2. if period changed: write period; clear the flag;
3. write volume.

A restarted voice therefore always shows: PER=1, INTREQ, DMACON, LOC, LEN, PER, VOL, INTREQ,
DMACON, LOC, LEN, PER, VOL. A voice without changes writes only its volume.

## 6. Timing (colour clocks, measured)

### 6.1 Tick

Tick due times are on a 70 940 cc grid. The writes of a tick start after the 6502 run and the
conversion: an emulated 6502 instruction costs very roughly 30 to 40 cc (a `NOP` about 31 cc), and
the conversion of four channels about 3 500 to 4 000 cc. Only estimates were made; tick-relative
timing below is exact as measured.

### 6.2 Voice without restart

Period (if written) to volume: 18 cc. Volume to the next voice's first write, when that is a period
or volume write: 41 cc. So an idle tick shows four volume writes 41 cc apart.

### 6.3 Restart

| from | to | gap |
|---|---|---|
| previous write | PER=1 | line sync; PER=1 lands at horizontal position 34..46 |
| PER=1 | INTREQ | 26 |
| INTREQ | DMACON | 6 |
| DMACON (DMA was off) | LOC | 20 or 21 |
| DMACON (DMA was on) | LOC | after the interrupt flag rises (host Paula, at most one sample period of the old period after the PER=1 write, i.e. up to 65 535 cc), plus about 15 |
| LOC high word | LOC low word | 0 (one 32-bit write) |
| LOC | LEN | 6 |
| LEN | PER | 12 |
| PER | VOL | 10 |
| VOL | INTREQ | line sync; INTREQ lands at position 58..70 |
| INTREQ | DMACON | 6 |
| DMACON | LOC | wait for the flag, then line sync; LOC at position 32..42 (typically 417..432 cc after DMACON) |
| LOC | LEN | 6 |
| LEN | PER | 28 or 29 |
| PER | VOL | 18 |

"Line sync" = wait for the line number to change, then for horizontal position ≥ 22; the 68k
polling adds the spread given (positions are colour clocks from the line start).

### 6.4 Song stop writes

Location to length 10, length to period 10, period to volume/data 14, next voice 14.

### 6.5 Overrunning ticks

A restart of a voice whose old waveform plays at a long period can block for up to about 65 000 cc,
and four of them can exceed a tick. Ticks are never dropped: a tick that became due during a long
tick runs as soon as that one ends (also two in a row), and later ticks are on the grid again.

## 7. Waveform bank

The player carries a bank of 48 908 bytes of 8-bit signed samples (the file `data/pokeynoise_waves.bin`
next to this spec; whether it is embedded is decided elsewhere, see README). Offsets in section 4
are byte offsets into it. Offset 0 holds 8 zero bytes (SILENCE). The other reachable waveforms are
one-period renderings of the POKEY outputs (pure tone at several sample rates, poly4, poly5,
poly5+poly4, poly5+poly17, poly17), filtered; their structure is only what section 4 says (offset,
length). The bank also contains waveforms only reachable through paths the player never takes
(offsets `$1154`, `$141A`, `$1CF2`, `$1EA4`, `$20A4`, `$2212`, `$2610`, `$263E`, `$26D0`, `$2770`)
and two unused gaps; they need not be kept if the bank is cut.

The Paula location is the address of the bank in chip memory plus the offset; any fixed base works.
