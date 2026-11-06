# CustomMade (`cm.*`, raw `rk.*`): behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Eagleplayer "CustomMade" (V1.1, prefixes `cm`, `rk`, `rkb`), which plays Ron Klaren /
Ivo Zoer game music from **raw memory dumps** of a game's sound driver. "CustomMade" is the name the
Eagleplayer gives the Ron Klaren format. It is the same music data as the hunk-wrapped
`RON_KLAREN_SOUNDMODULE!` files already handled by `src/replay/ronklaren.rs`, but in a different
container, and played by different code:

- `ronklaren.rs` (reference: Eagleplayer "RonKlaren") takes only AmigaDOS hunk executables with
  the `RON_KLAREN_SOUNDMODULE!` header and runs the game's own replay code.
- CustomMade takes only raw dumps (no hunk header; see 1.2). It does **not** run the dump's code.
  It finds the music tables in the dump, translates the dump's absolute pointers, and plays them
  with one fixed, generic replayer of its own (the old style, like Battle Squadron's, with no voice
  backup). That fixed replayer is what this spec describes.

Neither reference player accepts the other's files. All raw dumps in the corpus (seven `cm.*`
files and five raw `rk.*` files in `Other_RK_Versions` folders) are refused by the open replayers
today.

This text describes behaviour seen in write logs of the reference 68k player run through the host
with `DELI_WRITE_LOG`. It contains no code of the original. "cc" = colour clock; costs in section 8
are in CPU cycles (2 cycles = 1 cc).

Status: a private model that follows this text reproduces, for 12 corpus dumps and all their
subsongs (37 runs of 180 s, about 1.73 million register writes), the address, value and order of
every write. It also reproduces the tick-relative time of every write in all 127 178 ticks. See
LOGS.md.

## 1. File format

### 1.1 What a file is

A file is a byte image of part of a game's memory: the sound driver's code, then its data
(period table, song table, voice records, arpeggio table, instrument records, track lists,
tracks, sample descriptors, vibrato tables, sample data). The image is not relocatable. Every
pointer stored in its data is an absolute address from the game's memory map. **Origin** is the
absolute address at which offset 0 of the image sat in the game (it is found as described in 1.3).
To translate a stored pointer `p` into an image offset: `p - Origin`, computed with 32-bit
wrap-around. This document calls that `rel(p)`.

All multi-byte values are big-endian. "word" = 16 bits, "long" = 32 bits.

### 1.2 Detection

All of these must hold:

1. file size > 3000 bytes;
2. one of:
   - word at 0 is `0x4EF9` or `0x4EB9`, and word at 6 is `0x4EF9`;
   - word at 0 is `0x6000` and word at 4 is `0x6000`;
3. the 12-byte sequence `42 28 00 30 42 28 00 31 42 28 00 32` starts at some even offset `o` with
   8 <= o < 408.

AmigaDOS hunk files (`00 00 03 F3` at 0) therefore fail rule 2. Those are Ron Klaren hunk modules
for `ronklaren.rs`.

### 1.3 Locating the tables (init scan)

The player scans the image for fixed byte sequences of the game driver and derives seven values:
`songs`, `arps`, `instrs`, `ptoff`, `timer`, `Origin` and two helpers (`oldplay`, `voc0`).
All offsets are image offsets; `w(x)` / `l(x)` read a word / long at offset `x`; `s16` sign-extends.

Scan: position `o` starts at 0 and steps by 2. At most 2048 positions are examined. After the step
that leaves a position, if the long at the **new** position is `0x1AC01940` (the first two entries
of the driver's own period table) the scan stops. At each position, the first of these tests that
matches is applied, and the rest are skipped:

| order | long at `o` | effect |
|---|---|---|
| 1 | `0xD04149FA` | `arps = o + 4 + s16(w(o+4))` |
| 2 | `0x48E7F8FC` or `0x48E7FFFE` | if `oldplay != 0` and `Origin` is still 0: `Origin = oldplay - o`. Then look at the 21 words at `o, o+2, ..., o+40` for the first one equal to `0x41FA`. If one is found at `q`: `songs = (q+2) + s16(w(q+2))`, and the scan carries on from `q+2` (its next step goes to `q+4`). If none is found, the scan carries on from `o+42` (next position `o+44`). |
| 3 | `0xE94847F0` | `ptoff = s16(w(o+4))` |
| 4 | `0x00BFD500` | `timer = (byte at o-1) * 256 + (byte at o+7)`. Then `oldplay` (an absolute address): if `w(o+20) == 0x4E71`, leave it unchanged; else if `w(o+28) == 0x21FC`, `oldplay = l(o+30)`; else if `w(o+32) == 0xC000`, `oldplay = l(o+22)`; else `oldplay = l(o+32)`. |
| 5 | `0x42A8001C` | `instrs = o + 6 + s16(w(o+6))` |
| 6 | `0xE44843FA` or `0xE448207B` | if `voc0` is known: `P = l(o + 4 + s16(w(o+4)))` (the stored absolute address of voice record 0); if `Origin` is still 0: `Origin = P - voc0`. |
| 7 | `0x48E700F0` | `voc0 = o + 6 + s16(w(o+6))` |

After the scan: `songs += ptoff`. Every corpus file has all markers. A file without the `ptoff`
marker would get an undefined value from the reference, so treat such a file as not playable.
`Origin` is whichever of the two methods (order 2 or order 6) fires first with a non-zero result.
In the corpus both kinds occur: Origin values are 0x20, 0x13A74, 0x23000, 0x246F0, 0x3D800 and
0x485C4.

### 1.4 Song table and subsongs

`songs` points to 16-byte entries, one per subsong. Each entry holds four longs, the absolute
addresses of the track lists of voices 0 to 3. The table ends at the first entry whose first long,
masked with `0x00FFFFFF`, equals `0x00DFF0A0`. In the dump the driver's voice records follow the
song table, and each starts with that hardware address. **Number of subsongs** = number of entries
before that one. Subsong `n` (1-based) uses entry `(n & 15) - 1`.

### 1.5 Track list (12-byte entries)

| offset | size | meaning |
|---|---|---|
| 0 | long | absolute address of the track data |
| 6 | word | transposition (added to note numbers, 16-bit) |
| 10 | word | play count (the track plays this many times; see 4.5) |

The list ends at an entry whose first byte has bit 7 set. Its first long is `0xFFFFFFFF` in all
files. A track list entry that starts with a byte < 0x80 is a normal entry.

### 1.6 Instrument record (32 bytes, at `instrs + 32*i`)

| offset | size | meaning |
|---|---|---|
| 0 | long | absolute address of the sample descriptor |
| 4 | long | absolute address of the vibrato descriptor |
| 8 | byte | one-shot flag (non-zero: after the note's first tick the loop length becomes 1 word) |
| 9 | byte | phase (waveform-modulation) speed: one step every this many ticks; 0 = off |
| 10 | byte | phase length `L` |
| 11 | byte | vibrato speed; 0 = no vibrato |
| 12 | byte | vibrato depth (unsigned) |
| 13 | byte | vibrato delay, in rows of 4 ticks |
| 14..17 | 4 bytes | envelope target levels, stages 0..3 |
| 18..21 | 4 bytes | envelope step sizes, stages 0..3 |
| 22 | byte | phase value (written into the waveform; inverted on each direction change) |
| 23 | byte | phase direction (bit 7 clear: forwards, set: backwards; inverted on each change) |
| 24 | byte | phase position |
| 28 | long | zero for every instrument; the first record with a non-zero long here is past the end |

Bytes 22..24 are live state. The phase step (6.1) changes them in the image. So does it change the
waveform, which is part of the sample data. These changes are never undone: not at init, not when
the subsong changes.

Sample descriptor: `+0` long, absolute address of the sample data; `+4` word, length in words
(written to AUDxLEN); `+6` word, the phase base (see 6.1).
Vibrato descriptor: `+0` long, absolute address of a table of signed bytes; `+4` word, its
length `n`.

Metadata only: a sample descriptor whose length word is not 2 and is <= 16 is counted as a synth
sample, all others as samples.

### 1.7 Arpeggio table

12-byte entries at `arps + 12*x`, unsigned bytes added to the note number.

### 1.8 Track data

A byte stream. Commands (values 0x80..0x85) and notes (a byte < 0x80 followed by a duration
byte) are described in section 5.

### 1.9 Other values

- Tick rate: the host's CIA timer runs with `timer`. The tick period is `5 * (timer + 1)` cc.
  Measured: 48 010 cc (timer 0x2581), 49 290 (0x2681), 62 090 (0x3081), 63 370 (0x3181),
  68 490 (0x3581).
- Song length shown by the player: the largest number of entries (before the end marker, counted
  until a long equal to `0xFFFFFFFF`) among the four track lists of the subsong. Position = index
  of the current entry of the voice that has that longest list (the first such voice).
- Title: none. Author string "Ron Klaren".

## 2. Period table (player data)

The player has its own 84-entry table. It does not use the dump's. Index `i` (0-based) gives a
word:

```
1AC0 1940 17D0 1680 1530 1400 12E0 11D0 10D0 0FE0 0F00 0E20
0D60 0CA0 0BE8 0B40 0A98 0A00 0970 08E8 0868 07F0 0780 0710
06B0 0650 05F4 05A0 054C 0500 04B8 0474 0434 03F8 03C0 0388
0358 0328 02FA 02D0 02A6 0280 025C 023A 021A 01FC 01E0 01C4
01AC 0194 017D 0168 0153 0140 012E 011D 010D 00FE 00F0 00E2
00D6 00CA 00BE 00B4 00AA 00A0 0097 008F 0087 007F 0078 0071
006B 0065 005F 005A 0055 0050 004B 0047 0043 003F 003C 0038
```

The last 14 entries extend the usual 70-entry table. `period[i]` is entry `i`. "Period at `off`"
means the entry at byte offset `off` (a 16-bit value read as signed, so entry `off / 2`). Entries outside 0..83 read the player's
own memory (see README). Index 84 is the player's status word: `0x003F` while playing,
`0x013F` once stopped. Index 85 and up are its voice records. No corpus run reads outside 0..83.

## 3. Initialisation

### 3.1 Host sequence seen in the logs

1. After loading, the host sets volume and balance twice. Each time the player writes 0 to
   AUD0VOL, AUD1VOL, AUD2VOL, AUD3VOL, in that order. That gives 8 writes. The value is the last
   volume it wrote for that voice (0 at start), scaled as in 7.2.
2. InitSound (3.2).
3. To start subsong N (N > 1), the reference host goes through subsongs 1..N. Before each
   subsong after the first it calls stop (DMACON = 0x000F, then AUD0..3VOL = 0), sets volume and
   balance again (8 more zero writes), and calls InitSound. No tick runs in between.
4. Ticks start one tick period plus about 312 cc after the last init write (8.4).

### 3.2 InitSound(n)

- Status: stopped. Maximum envelope level: 0x3F.
- All four song-end flags set (4.6).
- Per voice k = 0..3, in order:
  - track list start `TL = rel(long at songs + 16*((n&15)-1) + 4k)`; current entry = TL;
  - if the word at TL+10 is 0x270F, it is replaced in the image by 0x003E. This is permanent; it
    happens in Battle Squadron's intro, subsong 1;
  - cleared to 0: note, row counter, portamento note, envelope speed, envelope counter, volume,
    vibrato delay counter, phase delay counter, portamento speed, envelope stage, vibrato
    position, arpeggio index, the 12-byte arpeggio;
  - one-shot pending = 1;
  - instrument = instrument 0;
  - write AUDxLC = image address of `rel(sample data)` of instrument 0's sample descriptor (32-bit
    write), then AUDxLEN = that descriptor's length word;
  - track pointer = rel(long at TL+0); transposition = word at TL+6; repeat counter = (word at
    TL+10) - 1 (16-bit).
  - **not** reset: the current period and the portamento target. They keep their values from the
    previous subsong; both are 0 after loading.
- Status: playing.

The image itself is not restored between subsongs (phase state, the 0x270F patch).

## 4. The tick

When the status is stopped, a tick does nothing. Otherwise voices 0, 1, 2, 3 are processed in
order, with the voice's DMA bit `b = 1 << k`:

1. write DMACON = `0x8000 | b`;
2. if one-shot pending: clear it, write AUDxLEN = 1;
3. if the row counter is 0: parse a row (section 5);
   otherwise: row counter -= 1, then effects (section 6).

A stop (5.4) during a tick does not end the tick. The remaining voices are still processed (in
the corpus they all reach a stop too).

### 4.1 State per voice

note (byte), transposition (word), current period, portamento target, portamento speed, row
counter, vibrato delay counter, vibrato position (word), phase delay counter, envelope stage (0..3),
envelope counter and speed, volume (byte), arpeggio (12 bytes) and arpeggio index (0..11),
instrument, track pointer, track list start/current entry, repeat counter (word), one-shot pending.

## 5. Row parse

At the start of a row: portamento speed = 0. If the **current** instrument's vibrato delay
(byte 13) is non-zero, vibrato delay counter = `(delay * 4 - 1) & 0xFF`.

Then the parser reads at the track pointer `p`. It uses a 16-bit scratch value `S`, whose high
byte matters only for zero-duration notes (5.6). `S` starts with high byte 0x80. The parser
repeats the following **pass**, in this fixed order, until an item ends the row:

1. **0x80 x**: arpeggio = the 12 bytes at `arps + 12*x`; arpeggio index = 0; `S = 12*x`;
   `p += 2`.
2. **0x81 n s d**: portamento. The envelope stage becomes 0, portamento note = n, target =
   period[n + transposition], portamento speed = s, row counter = `(d*4 - 1) & 0xFF`,
   `p += 4`. Store `p` and write AUDxPER = the **current** period (not the target). The row ends.
3. **0x82 i**: instrument. Volume = 0, envelope stage = 0, instrument = `instrs + 32*i`,
   `S = 32*i`. Write DMACON = `b`, then AUDxLC = sample data address, then AUDxLEN = length word of
   that instrument's sample descriptor. `p += 2`.
4. **0x83**: stop (5.4). The row ends and the track pointer is not stored.
5. **0x84 v**: envelope speed = v; `p += 2`.
6. **0x85**: same as 0x83.
7. any other byte with bit 7 set: end of track (5.5); then a new pass starts at step 1.
8. a byte `n` < 0x80 followed by a duration byte `d`: a note (5.6).

Each step looks at the byte at `p` as it is after the previous steps of the same pass. A command
can therefore only follow a command that comes earlier in this order within one pass. For example
`0x84 v 0x80 x` makes the 0x80 count as "end of track" at step 7. Stopping at step 4 or 6 does not
look at the following items.

### 5.4 Stop

Status = stopped, and the host is told that the song ended. No further tick does anything. The
host ends playback there.

### 5.5 End of track

- If the repeat counter is non-zero: decrement it, `p = rel(long at current entry + 0)`,
  transposition = word at entry+6. Restart the pass.
- Otherwise: current entry += 12. If the first byte of the new entry has bit 7 set (end of list):
  current entry = track list start, this voice's song-end flag is cleared, and if all four flags
  are now clear the host is told "song end" and all four flags are set again. Then (in both
  cases) `p = rel(long at entry+0)`, transposition = word at entry+6, repeat counter =
  (word at entry+10) - 1 (16-bit), and `S = repeat counter` (its high byte is what counts).
  Restart the pass.

Playback goes on after a song-end report from 5.5. The reference host stops a few ticks later
(3 to 5 ticks in the logs).

### 5.6 Note

- `d == 0`: no retrigger. Note = n, `off = (((S & 0xFF00) | n) + transposition) * 2` (16-bit),
  current period = period at `off`, then `S = off`. `p += 2`. Restart the pass (no write).
  With `S`'s high byte 0x80 or 0 this is just `period[n + transposition]`. Other high bytes
  (after 0x80 x with x >= 22, after 0x82 i with i >= 8, or after a track change whose play count
  is 0 or > 256) read far outside the table. That never happens in the corpus.
- `d != 0`: row counter = `(d*4 - 1) & 0xFF`; envelope stage = 0; note = n; current period =
  period[n + transposition] (clean index). Write DMACON = `b`, AUDxLC = sample data address,
  AUDxLEN = sample length (of the current instrument, after any 0x82 of this row). `p += 2`.
  If the instrument's one-shot flag is set, one-shot pending = 1. Store `p`. Write
  AUDxPER = current period. The row ends.

A note row therefore lasts `4*d` ticks (the parse tick plus `4*d - 1` effect ticks).
Durations of 64 or more wrap in the byte arithmetic.

## 6. Effects (ticks with a non-zero row counter)

The instrument is the voice's current instrument `I`. Steps in this order:

### 6.1 Phase (waveform modulation)

Only if `I.speed` (byte 9) != 0:
- if the phase delay counter is non-zero: decrement it;
- else: phase delay counter = `I.speed - 1`; take the sample descriptor `D`; `base = rel(D.data)`
  plus a signed 16-bit offset `B`: `B` is `D+6` with only its **low byte** reduced by `L`
  (byte 10), with no borrow into the high byte. Write the byte `I.value` (byte 22) at
  `base + I.pos` (pos = byte 24, 0..255). Then:
  - forwards (byte 23 bit 7 clear): `pos = (pos + 1) & 0xFF`; if `pos == (2*L) & 0xFF`, invert
    bytes 23 and 22 (bitwise NOT);
  - backwards: `pos = (pos - 1) & 0xFF`; if `pos == 0`, invert bytes 23 and 22.
  - store pos.

### 6.2 Pitch

- If portamento speed `s` != 0: with `c` = current period and `t` = target (16-bit), and "sign"
  meaning bit 15 of the 16-bit difference:
  - if `t - c` has sign clear: `c += s`; if `t - c` now has sign set, `c = t`;
  - else: `c -= s`; if `t - c` now has sign clear, `c = t`.
  - current period = c; write AUDxPER = c.
- Else (arpeggio): `a` = arpeggio[index]; `off = (((a + note) & 0xFF) + transposition) * 2`
  (16-bit); current period = period at `off`; write AUDxPER. Then index -= 1, and below 0 it
  becomes 11. The arpeggio is read 0, 11, 10, ..., 1, 0, 11, ... The order matters only for
  non-symmetric arpeggios.

### 6.3 Vibrato

Only if `I.vibspeed` (byte 11) != 0:
- if the vibrato delay counter is non-zero: decrement it (no write);
- else: with the vibrato descriptor `V`: `x` = signed byte at `rel(V.table) + position`;
  write AUDxPER = `(x * I.depth + current period) & 0xFFFF` (the current period is not changed);
  then position -= vibspeed; if it became negative (bit 15), position = `2 * V.length`.
  So position can be `2n`, which reads past an `n`-byte table into whatever follows in the
  image.

### 6.4 Envelope

Envelope counter -= 1 (byte). If it is still >= 0 (bit 7 clear), nothing else happens. Otherwise:
counter = envelope speed; with stage `st`, target `T = I.level[st]` (byte 14+st), step
`E = I.step[st]` (byte 18+st), volume `v`, max = 0x3F; "below" means bit 7 of the 8-bit
difference:
- if `T - v` has bit 7 clear (rising): `v = (v + E) & 0xFF`;
  - if `v - max` has bit 7 set (below max): if `T - v` now has bit 7 set, `v = T` and advance;
  - else `v = max` and advance;
- else (falling): `v = (v - E) & 0xFF`; if `T - v` now has bit 7 clear, `v = T` and advance;
- advance = stage + 1, unless the stage is already 3.
- store v (all 8 bits). Write AUDxVOL = `v & 0x3F` (scaled as in 7.2).

With envelope speed 0, the volume changes every tick. With speed `e`, it changes every `e+1`
ticks.

## 7. Other host-visible behaviour

### 7.1 Song end

- 0x83 / 0x85: stop and report the song end (5.4).
- All four track lists having wrapped (5.5): report the song end, keep playing.

### 7.2 Volume, balance, voice mute

Volume writes (envelope and the host's volume calls) are `(v * side) >> 6` with the left level
for voices 0 and 3 and the right level for voices 1 and 2. Each level is
`balance * master / 64`. At the defaults (64) this is `v`. A muted voice gets 0. The player
remembers the last unscaled volume per voice for the host's volume calls.

### 7.3 Stop (EndSound)

DMACON = 0x000F, then AUD0VOL..AUD3VOL = 0.

## 8. Timing

### 8.1 Frame

Ticks are `5 * (timer + 1)` cc apart, exactly (checked on every tick of every run).

### 8.2 Clock model

For a tick keep a cycle counter `T`, 0 at the interrupt call. Each step below adds its cost to
`T`. A register write is stamped `call_time + floor(T / 2)`, with `T` taken after the costs listed
up to that write. Both words of AUDxLC get the same stamp. The costs are fitted to the logs, and
only their sums along a path matter. A zero cost means that step's time is counted in a
neighbouring step. The first write of a tick (voice 0's DMACON) is at `floor(166/2) = 83` cc after
the call.

`k` = voice number. Per-voice helper costs:

| name | k=0 | k=1 | k=2 | k=3 |
|---|---|---|---|---|
| `per_k` (after a period write) | 110 | 141 | 172 | 180 |
| `smp_k` (after a sample's AUDxLEN write) | 186 | 217 | 248 | 256 |
| `vpre_k` (before an envelope volume write) | 0 | 23 | 46 | 36 |
| `vpost_k` (after an envelope volume write) | 90 | 121 | 152 | 0 (not observable) |
| `wrap_k` (track list wrap) | 0 | 33 | 62 | 81 |

### 8.3 Step costs (cycles), in execution order

Voice start (for every voice, including voice 0): **166** → DMACON write.
One-shot pending: **53** → AUDxLEN=1 write. Not pending: **21**.

Effects tick (row counter non-zero): **200**, then:
- phase: off 0 / delay counting 47 / step forwards 285 (with direction change 327) / step
  backwards 265 (with direction change 297);
- pitch: portamento before its write: up 0 (clamped 4), down 0 (clamped 14); arpeggio before its
  write: 14. → AUDxPER write. After it: portamento `per_k`; arpeggio 0 (8 if the index wrapped
  to 11);
- vibrato: off 16 / delay counting 63 / active 293 → AUDxPER write, then `per_k`, then 0 (80 if
  the position wrapped);
- envelope: counter not expired 34. Expired: 383 plus an extra cost for the case:
  falling without reaching 0; rising without reaching 29; falling and reaching the target
  35 (21 if the stage was 3); rising and reaching the target 74 (60); rising and clamped at the
  max 69 (55). Then `vpre_k` → AUDxVOL write → `vpost_k`.

Row tick (row counter 0): vibrato delay not set 133 / set 159; then per pass: **47**, then:
- 0x80 present 180 (absent 0);
- 0x81 present 124 → AUDxPER write → `per_k`, the row ends (absent 0);
- 0x82 present 96 → DMACON write → 108 → AUDxLC write → 20 → AUDxLEN write → `smp_k`
  (absent 0);
- 0x83 present 110, the row ends (absent 152);
- 0x84 present 28 (absent 0);
- 0x85 present 0, the row ends (absent 49);
- the first stop (0x83 or 0x85) of a tick costs as listed; each further stop in the same tick
  costs 1 more;
- end of track: repeat 42; next entry 139, plus on a wrap `wrap_k` and 198 (no song end) or 282
  (song end reported); then a new pass (47 ...);
- note with `d == 0`: 0, a new pass;
- note with `d != 0`: 0 → DMACON write → 108 → AUDxLC write → 20 → AUDxLEN write → `smp_k` →
  one-shot flag set 16 / clear 0 → AUDxPER write → `per_k`.

The next voice's 166 then follows directly.

Example (Plague ingame, first tick, voice 0: one-shot pending, row `0x82 i`, `0x84 v`, note with
d != 0, instrument without one-shot). Cumulative `T` and tick-relative stamps (`floor(T/2) - 83`):

| write | costs added | T | stamp |
|---|---|---|---|
| DMACON 0x8001 | 166 | 166 | 0 |
| AUD0LEN = 1 | 53 | 219 | 26 |
| DMACON 0x0001 | 133 + 47 + 0 + 0 + 96 | 495 | 164 |
| AUD0LC | 108 | 603 | 218 |
| AUD0LEN | 20 | 623 | 228 |
| DMACON 0x0001 | 186 + 152 + 28 + 49 + 0 | 1038 | 436 |
| AUD0LC | 108 | 1146 | 490 |
| AUD0LEN | 20 | 1166 | 500 |
| AUD0PER | 186 + 0 | 1352 | 593 |
| DMACON 0x8002 (voice 1) | 110 + 166 | 1628 | 731 |

These are the reference's values.

### 8.4 Absolute times (host business)

- The 8 host volume writes are 124, 136, 131, 241, 124, 136, 131 cc apart.
- From the last of them to the first init write (voice 0 AUDxLC):
  `(2411 + 49*E + 15*U) / 2` cc (rounded), where `E` is the total number of track list entries
  of the subsong's four voices (each counted up to the end marker) and `U` is the number of voices
  whose count is greater than all the counts before it (counting from voice 0, start 0). This
  matches all 37 runs within 1 cc.
- Init writes: AUDxLC → AUDxLEN 10 cc; AUDxLEN → next voice's AUDxLC 377 or 378 cc (755 cycles).
- First tick: the first DMACON is one tick period plus 311..313 cc after the last init write.
- The first host volume write after loading depends on the length of the init scan. It is about
  `10 + 154 * positions + 77 * instruments + 38 * subsongs + 26 * w + 17 * q` cc, where
  `positions` is the number of scan positions examined (1.3), `instruments` the number of instrument
  records, `w = (o - 8) / 2` for the offset `o` of the detection sequence (rule 1.2.3), and `q` the
  number of words looked at in the order-2 searches. That fit is within 11 cc on the corpus;
  measured values are in LOGS.md.
