# GlueMon (Amiga, `glue.*` / `gm.*`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the "GlueMon" module format (GlueMaster / Northstar), as played by the UADE/Eagleplayer player
"GlueMon" (V1.13). This is a behaviour description derived from the reference player's write logs and
execution-cost traces. It contains no code of the original.

No real GlueMon module exists in the available corpus (files named `glue*` there are ordinary
ProTracker/Soundtracker modules that only carry the word in their title). Every statement below was
checked against the reference 68k player on synthetic modules built from this description (LOGS.md).

Status (private model written from this text, compared write by write with the reference):

| item | result |
|---|---|
| 5 synthetic modules, all to the end of the reference run (2 437 register writes) | address, value and absolute colour clock of every write equal |
| audio DMA fetches of the noise buffer and drum wave (drum-mode module, 1 s) | 784 + 80 fetched words equal the buffers of 1.5 / 3.1 |

Units: "cycles" are 68000 cycles as the reference charges them; 2 cycles = 1 colour clock ("cc").

## 1. File format

All multi-byte values are big-endian. Offsets are file offsets.

### 1.1 Detection

Both must hold:

- bytes 0..3 are `GLUE` (0x47 0x4C 0x55 0x45),
- bytes 4..7 are 0xB8 0xB3 0xAA 0xBA.

Nothing else is checked.

### 1.2 Header

| offset | size | meaning |
|---|---|---|
| 0x00 | 8 | magic (1.1) |
| 0x08 | 8 | module name, up to 8 characters, zero-padded (not terminated when all 8 are used) |
| 0x10 + 3*v | 1 | voice v (v = 0, 1, 2): start volume `S_v` |
| 0x11 + 3*v | 1 | voice v: decay step `D_v` |
| 0x12 + 3*v | 1 | voice v: decay floor `F_v` |
| 0x19 | 1 | voice 3: `S_3` (four-voice mode only) |
| 0x1A | 1 | voice 3: `D_3` (four-voice mode only) |
| 0x1B | 1 | voice 3: `F_3` (four-voice mode only) |
| 0x1C | 1 | `speed`: ticks per row (1..255) |
| 0x1D | 1 | `rows`: rows per pattern (1..255) |
| 0x1E | 32 | waveform of voice 0 |
| 0x3E | 32 | waveform of voice 1 |
| 0x5E | 32 | waveform of voice 2 |
| 0x7E | 32 | waveform of voice 3 (in drum mode used by four drum kinds, 3.5) |
| 0x9E | 1 | `N`: order-list count |
| 0x9F | ... | order list, one byte per entry (pattern number), 0xFF = end marker |

**Mode.** If the 16-bit word at 0x1A equals 0x524D ("RM"), voice 3 is a **drum voice** (drum mode); the
bytes 0x19..0x1B are then not used and voice 3 has `D_3 = 1`, `F_3 = 0` (its start volume and decay come
from the drum kind, 3.5, which overwrites `D_3`). Otherwise all four voices are melodic (four-voice mode).

### 1.3 Patterns

- `rowbytes = rows * 4` (computed in 16 bits).
- Pattern area start: `P = 8 + ((N + 0x97) mod 256)`. For `N <= 0x68` this is `0x9F + N`, directly after
  `N` order bytes. For larger `N` the sum wraps and the pattern area starts near the beginning of the file
  (verified, module gmwrap).
- Pattern `p` (0..63) starts at `P + p * rowbytes`. Each row is 4 bytes, one per voice, voice 0 first.
  Pattern numbers 64 and above are not supported (the original reads unrelated memory).

Row byte meaning for voices 0..2 and for voice 3 in four-voice mode:

| byte | meaning |
|---|---|
| 0xFF | nothing |
| 0xFE | note off: the voice's volume becomes 0 (no register write at that moment) |
| other `b` | note: period = `PERIOD[b >> 1]` (bit 0 ignored); valid for `b <= 97` |

Voice 3 in drum mode: 0xFF = nothing, 0xC8..0xCF = drum kinds (3.5), every other value (including 0xFE
and note bytes) = "restart with pattern data" (3.5).

Period table (49 entries, index 0..48):

    3424 3232 3048 2880 2712 2560 2416 2280 2152 2032 1920 1808
    1712 1616 1524 1440 1356 1280 1208 1140 1076 1016  960  904
     856  808  762  720  678  640  604  570  538  508  480  452
     428  404  381  360  339  320  302  285  269  254  240  226
     214

(Indices 49..127, i.e. note bytes 98..253 other than 0xFE, read past the table in the original; not
supported.)

### 1.4 Order list and song length

The order list is read from 0x9F onwards **without a bound**: an entry index past `N - 1` reads whatever
byte lies there (pattern data, see module gmstart). Playback starts at entry `N - 1` (2.2): with the usual
layout, where the last of the `N` bytes is the 0xFF end marker, the first pattern played is entry 0.

Subsongs: one. Title: the module name (1.2). Song length (ticks until the song-end report, 4.3): with the
usual layout and `k` entries before the end marker, `k * rows * speed` ticks after the first tick (the
first tick's row is row 0 of entry 0).

Info value: "number of patterns" = 1 + the largest entry among the entries scanned from entry 0 up to the
first 0xFF (entry 0 is always included).

### 1.5 Player-side data

Two 32-byte sample buffers belong to the player, not the module:

- **noise buffer** (32 bytes), rewritten at the start of every tick (3.1);
- **drum wave** (32 bytes): one cycle of a 4-byte square (two high, two low bytes), followed by a rising ramp
  from 0 in steps of 10 (20 bytes) and a falling ramp from 180 in steps of 20 (8 bytes). The exact bytes are
  in `data/drum_wave.bin`.

In the reference the drum wave lies directly after the noise buffer (address of the noise buffer + 32).
Keep that relation in a port: the log comparison tool expects one constant offset between the reference's
and the port's player-side addresses.

## 2. Initialisation

### 2.1 Hardware set-up (register writes, in this order)

1. DMACON = 0x000F.
2. Voice 0: LC = address of file offset 0x1E; LEN = 16. Voice 1: LC = 0x3E, LEN = 16. Voice 2: LC = 0x5E,
   LEN = 16. Voice 3: LC = 0x7E, LEN = 16. (LC is a 32-bit write, logged as two 16-bit writes, high word
   first, with the same stamp.)
3. DMACON = 0x800F (all four voices run from now on; the waveforms loop continuously, they are never
   restarted by notes).
4. AUD0VOL, AUD1VOL, AUD2VOL, AUD3VOL = 0 (this order).

No period is written at initialisation.

### 2.2 State

- Voice volumes `vol_0..vol_3` = 0 (16-bit unsigned each).
- Tick counter `tc = speed - 1` (8-bit).
- Row position `pos = rowbytes - 4` (16-bit), order position `ord = (N - 2) mod 256`.
- Current pattern: none yet (the first tick always starts a new pattern, 3.3).
- `ended = false` (song-end report suppression, 4.3).
- Noise state `R = 0x986FA0B9` (32-bit).
- Drum (voice 3, drum mode): period `dp = 0`, period step `ds = 0`, sample = voice 3 waveform.
- Volume factors: `M` = master volume, `BL` = balance level of voices 0 and 3, `BR` = of voices 1 and 2
  (0..64 each). In the reference all three are 64.

## 3. The tick

Called at 50 Hz (5.1). Steps in order:

### 3.1 Noise

Eight times: `R = (rotate_left_32(R, 7) + 0x6ECA756D) mod 2^32`, then `R = R XOR 0x9E59A92B`, then store `R`
big-endian into the noise buffer at byte 4*i (i = 0..7). The buffer is player memory that voice 3 may be
playing. In the reference the new contents are visible to audio DMA from the moment of the tick call
(all 784 noise-buffer fetches of a drum-mode run agree with this; with the stores spread over the noise
step's cycles, 3 fetches early in a tick would disagree). A port should replace the whole buffer at the
call time.

### 3.2 Decay (voices 0, 1, 2, 3 in order)

If `vol_v > F_v` (unsigned 16-bit compare): `vol_v = (vol_v - D_v) mod 65536`. A decay larger than the
remaining distance wraps around to a large value (e.g. 5 - 7 = 65534); the decay continues from there
on the next ticks. This is observable (4.1) and must be kept.

### 3.3 Row step

`tc = tc + 1`. If `tc < speed` there is no row this tick: go to 3.6 (drum mode) or 3.7 (four-voice mode).

Otherwise `tc = 0`, `pos = pos + 4`. If `pos >= rowbytes`:

- `pos = 0`, `ord = ord + 1` (16-bit), `e = order[ord]`;
- if `e == 0xFF`: `ord = 0`, `e = order[0]`, and the song end is reported unless `ended` is false; in that
  case `ended` becomes true instead (the first wrap after init is silent);
- the current pattern becomes pattern `e`.

### 3.4 Row, voices 0..2 (and voice 3 in four-voice mode)

For v = 0, 1, 2 (then 3 in four-voice mode) with row byte `b = pattern[pos + v]`:

- 0xFE: `vol_v = 0`;
- 0xFF: nothing;
- otherwise: write AUDvPER = `PERIOD[b >> 1]`, then `vol_v = S_v`.

In four-voice mode the tick continues with 3.7.

### 3.5 Row, voice 3 in drum mode

With `b = pattern[pos + 3]`:

- 0xFF: nothing, continue with 3.6.
- A drum kind sets the drum state:

| byte | sample | `dp` | `ds` (16-bit, subtracted each tick) | `vol_3` | `D_3` |
|---|---|---|---|---|---|
| 0xC8 | drum wave | 0x0710 | 0xFF01 (period rises by 255) | 64 | 8 |
| 0xC9 | noise buffer | 0x0710 | 0x0014 (falls by 20) | 64 | 4 |
| 0xCA | voice 3 waveform | 0x01C4 | 0xFFCE (rises by 50) | 64 | 4 |
| 0xCB | voice 3 waveform | 0x025C | 0xFFCE | 64 | 4 |
| 0xCC | voice 3 waveform | 0x0388 | 0xFFCE | 64 | 4 |
| 0xCD | voice 3 waveform | 0x04B8 | 0xFFCE | 64 | 4 |
| 0xCE | noise buffer | 0x025C | 0xFFF6 (rises by 10) | 48 | 16 |
| 0xCF | noise buffer | 0x025C | 0xFFF6 | 40 | 2 |

  then restart voice 3: DMACON = 0x0008, AUD3LC = sample address, DMACON = 0x8008 (AUD3LEN stays 16).
- Any other byte ("restart with pattern data"): no drum state changes; voice 3 is restarted the same way
  with AUD3LC = start address of the **current pattern** (module data played as a sample).

Continue with 3.6.

### 3.6 Drum sweep (drum mode, every tick)

`dp = (dp - ds) mod 65536`, write AUD3PER = `dp`. This happens on every tick, also on ticks without a row.
(Before the first drum kind `dp` and `ds` are 0, so AUD3PER = 0 is written.)

### 3.7 Volumes (every tick)

`G_L = (BL * M) >> 6`, `G_R = (BR * M) >> 6`. For each voice the written value is
`((vol_v * G) mod 65536) >> 6` with `G = G_L` for voices 0 and 3 and `G_R` for voices 1 and 2.
Write order: AUD0VOL, AUD3VOL, AUD1VOL, AUD2VOL.

With `G = 64` the written value is `vol_v mod 1024`; after a wrapped decay values up to 1023 are written
(Paula then plays at full volume).

## 4. Reference-host behaviours visible in the output

### 4.1 Large volume values

See 3.2 and 3.7: the reference writes AUDxVOL values above 64 when a decay wraps; this is the original's
behaviour and appears in the logs (module gmdrums, voice 1).

### 4.2 Period before the first note

No period is written before a voice's first note (2.1); in drum mode AUD3PER = 0 is written every tick
until the first drum kind. Paula's behaviour for period 0 is the emulator's business.

### 4.3 Song end

The song end is reported at the row tick on which the order list reaches an 0xFF for the second (and every
further) time. The replay itself continues: entry 0 is played again. The reference host stops the run at
the end of the 0.1-second output block (354 689.5 cc) in which the report falls; the log therefore ends
1 to 4 ticks after the reporting tick (all 5 runs agree with this rule).

### 4.4 Stop

The player's stop entry writes AUD0VOL..AUD3VOL = 0 and DMACON = 0x000F; the reference host does not call
it in the logged runs.

## 5. Timing

### 5.1 Frame

The tick routine is called every **70 940 cc** exactly. The first call is **70 940 cc after the
initialisation routine returns** (every reference run). In the reference runs the initialisation routine is
entered at cc 415 (host property), so the first call is at cc 73 092 in four-voice mode and 73 081 in drum
mode (the init is 22 cycles shorter in drum mode, 5.2).

### 5.2 Clock model

All costs are static. For a routine call keep a cycle counter `T`, 0 at the call. A register write is
stamped `call_clock + floor(T / 2)`, where `T` already includes the step that makes the write. Both words of
an LC write carry the same stamp. The song-end report (a host callback) costs nothing beyond the step
`SE` below; `T` is always even when it is made, so no re-basing is needed.

Initialisation routine (2.1), from its call:

| id | cycles | step |
|---|---|---|
| I_HEAD | 366 | up to and including DMACON = 0x000F |
| I_LC0 | 36 | up to the voice 0 LC write; I_LEN 16 to its LEN write |
| I_LCn | 32 | for voices 1, 2: up to the LC write; then 16 to the LEN write |
| I_LC3 | 48 | voice 3: up to the LC write; then 16 to the LEN write |
| I_DMA | 16 | DMACON = 0x800F |
| I_BODY | 2 513 (four-voice) / 2 491 (drum) | up to and including the AUD0VOL = 0 write |
| I_VOL | 16 | each of the AUD1VOL, AUD2VOL, AUD3VOL = 0 writes |
| I_END | 320 | to the return |

Checks: the init writes are stamped (relative to the call) 183, 201, 209, 225, 233, 249, 257, 281, 289, 297,
then 1553, 1561, 1569, 1577 (four-voice; drum mode 11 cc less for the last four) and the routine ends at
1737 (1726).

### 5.3 Tick costs

| id | cycles | condition / step |
|---|---|---|
| HEAD | 194 | always, first |
| NOISE | 76 per value, 8 values | (buffer visibility: see 3.1) |
| HEAD2 | 28 | after the noise |
| DEC | 33 | per voice 0..3, the compare |
| DEC_SUB | 28 | added when the voice decays |
| CNT | 49 | counter increment and test |
| NOROW | 23 | no row this tick; then four-voice: go to VOL; drum: add 10, go to SWEEP |
| ROW | 69 | row tick: position step and test |
| WRAP | 99 | `pos` wrapped: order read and end-marker test |
| END | 173 | the entry was 0xFF |
| END_FIRST | 26 | `ended` was false (sets it) |
| SE | 64 | `ended` was true: the song-end report including the callback's return |
| END_RESTORE | 132 | after END_FIRST or SE |
| PAT | 60 | after WRAP (with or without END): pattern selection |
| ROWHEAD | 28 | every row tick, after ROW (and WRAP/PAT) |
| V_TEST | 31 | voices 0..2, and voice 3 in four-voice mode: byte read and 0xFE test |
| V_OFF | 26 | byte 0xFE |
| V_FF | 17 | not 0xFE: 0xFF test (voices 0..2); 19 for voice 3 in four-voice mode |
| V_NOTE | 76 | note: up to and including the AUDxPER write |
| V_VOL | 20 | note: after the PER write (sets `vol_v`) |
| V3_TAIL | 10 | voice 3, four-voice mode, after V_VOL |
| MODE | 21 | after voice 2 (mode test) |
| DR_TEST | 33 | drum mode: byte read and 0xFF test (0xFF: go to SWEEP) |
| DR_CMP | 17 | per comparison: drum kind 0xC8 + j needs j + 1 comparisons, "other" bytes need 8 |
| DR_SET | 86 (C8, C9, CE, CF) / 90 (CA..CD) / 0 (other) | drum state set-up |
| DR_OFF | 16 | DMACON = 0x0008 write |
| DR_LC | 16 | AUD3LC write |
| DR_ON | 16 | DMACON = 0x8008 write |
| SWEEP | 48 | drum mode: up to and including the AUD3PER write |
| VOL_HEAD | 116 | start of the volume stage |
| VOL_A | 96 | each of the AUD0VOL and AUD3VOL writes (step includes the write) |
| VOL_MID | 92 | between the AUD3VOL and the AUD1VOL step |
| VOL_B | 96 | each of the AUD1VOL and AUD2VOL writes |
| CLOSE | 192 | from the AUD2VOL write to the return |

Path order on a row tick: HEAD, NOISE, HEAD2, DEC x4, CNT, ROW, [WRAP, [END, END_FIRST or SE,
END_RESTORE], PAT], ROWHEAD, voices 0..2 (V_TEST, then V_OFF, or V_FF and possibly V_NOTE + V_VOL), MODE,
then voice 3 (four-voice: V_TEST, V_OFF / V_FF(19) / V_FF(19) + V_NOTE + V_VOL + V3_TAIL; drum: DR_TEST,
[DR_CMP..., DR_SET, DR_OFF, DR_LC, DR_ON], SWEEP), then VOL_HEAD, VOL_A (AUD0VOL), VOL_A (AUD3VOL),
VOL_MID, VOL_B (AUD1VOL), VOL_B (AUD2VOL), CLOSE.

Checks (drum mode, first tick, all voices with a note, drum kind 0xC8, no decay, first wrap): writes at
`T` = 1722 (AUD0PER), 1866, 2010, 2203 (DMACON), 2219 (LC), 2235 (DMACON), 2283 (AUD3PER), 2495, 2591,
2779, 2875; return at 3067. Four-voice mode, tick without a row, all four voices decaying: AUD0VOL at 1358.

## 6. How to check

Run the reference 68k player through the host with DELI_WRITE_LOG on the modules described in LOGS.md
and compare with the port's log; addresses, values and stamps should be equal. The synthetic modules are in
`modules/` and are rebuilt by `logs/mkmods.py` (LOGS.md).
