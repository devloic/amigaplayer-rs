# SynTracker (Eagleplayer "SynTracker", UADE player name SynTracker) - behaviour spec

*Sanitised for publication: behaviour only.*

Written from observed behaviour (register-write logs of the 68k reference, DMA word logs) and from
reading the reference. Everything below was checked against the reference with a model of this
text (see LOGS.md): all Paula register writes (address, value, order) and all audio DMA words of
17 test modules were reproduced exactly; timings (section 10) are approximate.

SynTracker is a 4-channel synthesis tracker: each channel plays a 32-byte looping waveform that
is rebuilt every tick (wavetable "scanning" and low-pass "morphing"), or a plain 8-bit sample.
The player is called once per frame (50 Hz). There are no subsongs and no title.

## 1. Identification

- A module is a SynTracker module when its first 16 bytes are the ASCII text `SYNTRACKER-SONG:`
  (no terminator, case sensitive). Nothing else is checked by the player.
- File name prefixes the Eagleplayer lists: `st.` and `synmod.` (the content check decides).
- Subsongs: exactly one. Title: none. Author: none. Minimum useful size: 0xA14 + 128 * number of
  patterns (more when instruments point beyond).
- Song length (positions): big-endian u16 at 0x10 (`nPos`). Highest pattern number: u16 at 0x12
  (`nPat - 1`; patterns are numbered 0..hdr12). Duration is `nPos * 32 * speed` ticks (speed
  changes with effect S, jumps with B change it); 50 ticks per second.
- Bytes 0x14..0x413 are 32 text fields of 32 bytes. The player hands the address 0x14 to the host
  as "sample/instrument info text", so they are most likely the 32 instrument names (not verified,
  the host output was not inspected). They do not influence sound.

## 2. File layout

All numbers big-endian. `D` is the start of the data area:
`D = 0xA14 + (hdr12 + 1) * 128` (right behind the last pattern).

| offset | size | content |
|---|---|---|
| 0x000 | 16 | `SYNTRACKER-SONG:` |
| 0x010 | 2 | nPos, number of positions |
| 0x012 | 2 | highest pattern number |
| 0x014 | 1024 | 32 text fields of 32 bytes (see above) |
| 0x414 | 512 | 32 instrument records of 16 bytes (4.1) |
| 0x614 | 512 | pattern-number lists: channel c uses 128 bytes at 0x614 + 0x80*c, entry p = pattern number at position p |
| 0x814 | 512 | transposition lists, same shape: channel c, position p at 0x814 + 0x80*c + p; signed byte, semitones |
| 0xA14 | 128 * (hdr12 + 1) | patterns |
| D | rest | instrument data, vibrato tables, scan windows ("pages") |

Only the first nPos entries of the lists are used. All four channels use the same nPos.

### 2.1 Pattern

32 rows of 4 bytes: `note, instrument, effect, parameter`. A pattern is 128 bytes, row r at
offset 4*r. Row 0 is played first. The same pattern may be used by several channels and positions.

- note: 0 = none. 1.. = semitone index + 1 (note n sounds period table entry n - 1 + transposition).
  Valid indices are 0..71 (6 octaves).
- instrument byte, see 4.2.
- effect: an ASCII letter (section 7); 0 = none; unknown letters are ignored.

### 2.2 Instrument record (16 bytes at 0x414 + 16*i, i = 0..31)

| +0 | u32 | offset of the instrument data from D |
| +4 | u16 | length in words (value written to AUDxLEN). Synth instruments: 16 (32 bytes). A value 0 on a sample instrument means "silent" (4.4) |
| +6 | u16 | volume 0..64 |
| +8 | u16 | sample instruments: offset of the first sample byte inside the instrument data |
| +10 | u16 | sample instruments: offset of the loop start inside the instrument data |
| +12 | u16 | sample instruments: loop length in words; 0 = no loop |
| +14 | u16 | unused |

Instrument numbers in patterns are 1-based: 1..16 = records 0..15 are SYNTH instruments,
17..32 = records 16..31 are SAMPLE instruments. A first long of 0xFFFFFFFF marks the end of the
used records in files written by the editor; the player does not look at it for playing.

### 2.3 Synth instrument data (at D + offset)

32 bytes of base waveform (signed 8-bit), then a 32-byte parameter block ("blk"; offsets below are
from the start of the block, i.e. +0x20 from the start of the data):

| blk | meaning |
|---|---|
| 0..7 | envelope: 4 pairs (count, amount) = phases attack, decay, sustain-rise, release (6.3) |
| 8..15 | morph: 4 pairs (count, coefficient) (6.5) |
| 0x10 | vibrato: high nibble = speed, low nibble = depth (6.2) |
| 0x11 | scan period in ticks (0 = scanning off) (6.6) |
| 0x12 | vibrato table page: the 32 signed bytes at D + 64 * value are this instrument's vibrato wave |
| 0x14 | scan start position and "lower turn" position (0..31) |
| 0x15 | "upper turn" position (0..31) |
| 0x16 | turn action: bit 7 set = reflect, clear = jump forward by the value |
| 0x18 | scan window page: the 32 bytes at D + 64 * value are the scan window |
| others | unused |

Pages are 64-byte units counted from D; only the first 32 bytes of a page are read. Scan window
bytes with bit 7 set (0x80..0xFF) mean "transparent".

### 2.4 Sample instrument data

Signed 8-bit samples at D + offset(+0) + (+8) (first part) and D + offset + (+10) (loop part).

## 3. Constants

Period table PT[0..71] (hex, descending; one entry per semitone, lowest note first):

```
1ab0 1940 17d0 1670 1530 1400 12e0 11d0 10d0 0fe0 0f00 0e20
0d58 0ca0 0be8 0b38 0a98 0a00 0970 08e8 0868 07f0 0780 0710
06ac 0650 05f4 059c 054c 0500 04b8 0474 0434 03f8 03c0 0388
0356 0328 02fa 02ce 02a6 0280 025c 023a 021a 01fc 01e0 01c4
01ab 0194 017d 0167 0153 0140 012e 011d 010d 00fe 00f0 00e2
00d6 00ca 00be 00b4 00a9 00a0 0097 008f 0086 007f 0078 0071
```

Built-in vibrato wave SINE[0..31] (signed bytes; used when no page is selected, i.e. after effect V
and for the first note after an instrument number is set until the instrument loads its own):

```
00 18 2f 45 58 68 74 7b 7e 7d 77 6c 5e 4c 37 20 08 f1 d9 c3 af 9e 90 87 82 82 86 8f 9c ad c1 d7
```

Out-of-range index behaviour (not producing music in sane modules, listed for completeness):
PT[-1] is the loop-length word still pending for channel 3 (0 except in the tick when channel 3
starts a looped sample, see 8.2); PT[72 + j] (j = 0..15) is the big-endian word j of SINE.

## 4. Playback state and notes

### 4.1 Global state

speed (initial 6), tick counter (initial 0), position (0), row (0), jump target (initial 0,
meaning "go to position 0, row 0 at the first row tick"), master volume per channel (64 under
UADE; the written volume is `(value * master) >> 6`), 128-byte working waveform memory, 128-byte
playing waveform memory, a 32-byte zero area (6.7).

### 4.2 Row instrument byte

| value | meaning |
|---|---|
| 0 | with a note: restart the previously set instrument of this channel (see 4.3) |
| 1..0x20 | with a note: set instrument and start it |
| 0x21..0x3F | with a note: only the period changes (as legato, 5.2), nothing starts |
| 0x40..0x4F | wave switch to synth instrument 0..15, note ignored (5.4) |
| 0x50..0x7F | ignored entirely (note too) |
| 0xFF | with a note: period changes, nothing starts (legato) |
| 0x80..0xFE | undefined (reference reads outside the tables); do not rely |

Starting instrument 0 before any instrument was ever set on the channel is undefined.

## 5. Row processing

### 5.1 When rows are processed

Every call (tick) increments the tick counter. When it equals speed the counter is cleared and the
row is processed (a "row tick"); otherwise nothing of 5 happens. The first row tick is the 6th
call (speed 6, counter starting at 0). Order inside a row tick: 5.1.1 jump, 5.2 for channels
0,1,2,3 in this order, 5.5 advance, then the common per-tick part (section 6).

5.1.1 If the jump target is not 0xFFFF: position = target, row = 0, target = 0xFFFF.

5.5 Advance: row = row + 1; at 32: row = 0, position = position + 1; when position == nPos:
position = 0 and the host song-end callback is called (the playing continues if the host lets it).
Note: a jump (effect B) takes effect at the NEXT row tick, after the row counter was advanced once
(so the row after a B row is not played, the target position starts at row 0).

### 5.2 Channel row (note n, instrument i, effect e, parameter p)

Let tr = transposition byte of (channel, position).

1. If i is in 0x40..0x7F: wave switch (5.4) if i <= 0x4F; then go to step 7 (effect still runs).
2. Else if n != 0: idx = (n + tr - 1) as 8-bit; if bit 7 set, idx = 0. Remember idx as the
   channel's note index. Base period = PT[idx]; arpeggio slots 0,1,2 and the current period all
   = base; glide rate = 0; slide offset = 0 (both reset on every note, with or without start).
3. If i == 0xFF: skip the start (step 7).
4. If i == 0: start the channel's remembered instrument (its number stays unchanged).
5. If 1 <= i <= 0x20: vibrato table = SINE, vibrato index = 0, vibrato depth = 0 (so a following
   start overwrites them for synth instruments and leaves them zero for sample instruments);
   remember i as the channel's instrument; start it.
6. If 0x21 <= i < 0x80: no start.
7. Run the effect (section 7).

Arpeggio state and counter are NOT reset by a note.

### 5.3 Instrument start

Common: write DMACON with the value `1 << channel` (this stops that channel's DMA; no 0x8000 bit).
Let j = instrument number - 1 (0..31).

Synth (j < 16):
- Pending Paula load for this tick: LC = playing buffer of the channel (6.7), LEN = record +4.
- instrument volume = record +6; channel volume = (63 * volume) >> 6.
- Envelope: phase 0, tick counter 0, level 0. Morph: phase 0, counter 0. Fade (effect G) rate 0.
- Vibrato: speed = high nibble of blk[0x10], depth = low nibble; vibrato wave = page blk[0x12];
  vibrato index is NOT reset (it was zeroed in step 5 when an instrument number was given; a
  restart with i == 0 keeps the index).
- Scan: direction +1; position = blk[0x14]; period = blk[0x11]; counter = period - 1 (8-bit, only
  if period != 0, otherwise 0); the "first copy" flag is set.
- Current synth block = this instrument's blk.

Sample (j >= 16), record length word (+4) != 0:
- Pending load: LC = module address + D + data offset + record(+8), LEN = record(+4).
- Pending second load (made after DMA is switched on, 6.8): LEN = loop length (+12), LC = module
  address + D + data offset + record(+10); if the loop length is 0, LC = start of the 32-byte zero
  area (6.7) and LEN = 1 (a one-word silent loop).
- volume as above. No synth block; envelope and morph are off (finished) and the scan is off.
  The vibrato depth and speed are cleared.

Sample, record length word == 0: write AUDxVOL = 0 immediately (this write comes right after
the DMACON write of this channel); pending load LC = start of the zero area, LEN = 1; the channel
volume variable is NOT changed (the per-tick volume code below will write the old value again);
synth block off as above.

(All starts: fade rate reset to 0, envelope and morph counters 0, level 0.)

### 5.4 Wave switch (instrument byte 0x40..0x4F)

The synth block of the channel becomes the one of synth instrument (i - 0x40) and the "first copy"
flag is set (6.6). Nothing else changes: no DMACON write, no LC/LEN, envelope, vibrato, morph and
scan state continue. The scan period, bounds and window are read from the new block at once; the
morph/envelope pairs likewise.

## 6. Per-tick processing (every tick, after the row part if any)

Order (this is also the Paula write order): periods of channels 0..3 -> for each channel 0..3:
fade, envelope, volume write -> morph for channels 0..3 -> scan for channels 0..3 -> pending
LC/LEN loads -> DMACON 0x800F -> copy of waveform memory -> second loads.

### 6.1 Period (channel k = 0..3; one AUDxPER write each)

Arpeggio: the 16-bit arp counter is incremented. When its low byte equals the arp rate (initial 1
on every channel; effect O sets it): the low byte of the counter is cleared, the current period
becomes slot[state] and state = (state + 1) mod 3. Otherwise the current period is unchanged.
(So by default the arpeggio steps every tick; rate n steps every n ticks; rate 0 effectively
never.)

p = current period. If vibrato depth d != 0 (6.2) p is modified. Then
`slide = slide + glide; AUDxPER = (p + slide) & 0xFFFF` (16-bit wrap).

### 6.2 Vibrato

For depth d != 0: v = wave[idx] (signed). a = (|v| * d) >> 7 (|-128| = 128). Let i = the index in
PT of the current period p, searched upward from the channel's note index (first entry equal to p;
the arpeggio slots are always entries at or above the note index).
- v >= 0: p = p - (((((p - PT[i+1]) & 0xFFFF) * a) & 0xFFFF) >> 5)
- v < 0: p = p + (((((PT[i-1] - p) & 0xFFFF) * a) & 0xFFFF) >> 5)
Then idx = (idx + speed) & 31. When d == 0 idx does not advance. The vibrato amount is a fraction
of the distance to the neighbouring semitone: at most 14/32 of it.

### 6.3 Envelope and volume (channel k)

Fade: volume = max(0, volume - fadeRate) (16-bit signed compare; rate set by effect G).
Level: byte 0..63, starts at 0 at an instrument start. Phase = 0,2,4,6 select the pairs
(count, amount) of blk[0..7]; phase 8 = finished. Each tick (only for synth channels; sample
channels use the plain volume):
1. while phase != 8: if (counter & 0xFF) == count of the current pair: counter = 0, phase += 2,
   continue with the next pair in the same tick; otherwise counter += 1, adjust the level and stop.
2. Adjust: phases 0 and 4 (attack, sustain-rise): level = level + amount (8-bit signed); if the
   result is > 0x3F as a signed byte it becomes 0x3F (an addition that wraps to a negative byte
   stays negative and is stored). Phases 2 and 6: level = level - amount; if the signed byte result
   is negative it becomes 0.
3. Output: `v = ((level & 0x3F) * volume) >> 6`. A channel without synth block outputs `volume`.
AUDxVOL = `(v * master[k]) >> 6`, written right after this channel's envelope step. The four
VOL writes follow the four PER writes and are separated only by the envelope work (in the reference
the fades of all four channels run first, then envelope+write of channel 0, 1, 2, 3; fade only
touches its own channel, so the values do not depend on that).

### 6.4 Summary of the order of work in a tick

row part (row ticks only) -> period of channels 0..3 -> fade of 0..3 -> envelope+volume write of
0, 1, 2, 3 -> morph of 0..3 -> scan of 0..3 -> pending LC/LEN of 0..3 -> DMACON 0x800F -> copy
working to playing waveform memory -> second LC/LEN of 0..3.

### 6.5 Morph (low-pass filter), channel k, synth block present

Wave memory: working waveform of the channel (32 bytes, 6.7). While morph phase != 8: pair =
blk[8 + phase], blk[9 + phase] = (count, coefficient c). If (mcounter & 0xFF) == count: mcounter =
0, phase += 2, continue. Otherwise mcounter += 1 and apply the filter ONCE and stop:

```
y = working[31] as UNSIGNED 0..255        (previous value for sample 0: last byte, zero-extended)
for i in 0..31:
    x = signed(working[i])
    y = y - x                              (16-bit signed)
    y = ((y * c) as 16-bit signed) >> 7    (arithmetic shift of the low 16 bits of the product, c = 0..255)
    y = y + x                              (16-bit)
    working[i] = low 8 bits of y           (y itself stays 16-bit for the next i)
```

So a phase with count N filters for N consecutive ticks. Each output sample is the input moved
toward the previous output by c/128: c = 0 leaves the buffer unchanged, c = 64 halves the
differences, c = 128 makes the whole buffer constant (every output equals the previous one, the
first equals the old last byte), c > 128 overshoots. When the scan rewrites the whole waveform in a
tick (6.6) the filtered content is discarded; the filter then continues on the new content in
later ticks.

### 6.6 Scan (waveform rebuild), channel k, synth block present

Window source: 32 bytes at D + 64 * blk[0x18]; base wave = the instrument's first 32 data bytes
(of the CURRENT block, so a wave switch changes it).

- Scan period P == 0: if the first-copy flag is set: copy the 32 base wave bytes into the working
  waveform and clear the flag; nothing else. (Otherwise nothing.)
- P != 0: clear the first-copy flag (no initial copy!). counter += 1 (8-bit). If P (signed byte)
  >= counter (signed byte): done for this tick (buffer unchanged). Else counter = 0 and:
  - pos = (pos + dir) & 31 (dir is +1 or -1)
  - if pos == blk[0x14] and blk[0x16] has bit 7: dir = +1;
    else if pos == blk[0x15]: if blk[0x16] bit 7: dir = -1, else pos = (pos + blk[0x16]) & 31.
  - rebuild: for i = 0..31: w = window[(pos + i) & 31]; out[i] = w if w < 0x80 else base[i].
  The rebuild is written to the working waveform with the destination quirk of 6.7.
  Period change by effect M changes P only (not the counter). The update happens in the (P+1)-th
  tick after the start (counter starts at P - 1, step 1 reaches P: no update; step 2 updates).

### 6.7 Waveform memory (chip buffer) and the destination quirk

A 0x120-byte buffer B in chip memory (address chosen by the host; the reference's is
0x006861F8 and this appears in the written LC values):

- B + 0x00 .. 0x1F: 32 zero bytes (the "zero area", never written).
- B + 0x20 + 32*k: working waveform of channel k (k = 0..3).
- B + 0xA0 + 32*k: playing waveform of channel k; this is the address given to Paula (LC).

At the end of every tick (after DMA is switched on) the 128 bytes at B+0x20 are copied to B+0xA0.

Quirk (reproduced bit exactly): during the scan pass, a destination pointer starts at B + 0x20
and, for channel k, is B + 0x20 + 32*k + 32*w, where w is the number of channels with lower index
that WROTE (first copy or rebuild) earlier in this very pass. A channel that writes thus puts its
32 bytes in another channel's working slot (and past B + 0x9F for the upper channels, where the data
is lost at the end-of-tick copy). The morph filter always works on the channel's own slot
B + 0x20 + 32*k. Consequence: with channels 0 and 1 both scanning in the same tick, channel 1's data
lands in the slot of channel 2.

### 6.8 Paula writes of a tick, in order

1. (row tick only, during row processing, channels in order 0..3) DMACON `1<<k` for each
   instrument start; a zero-length sample adds AUDxVOL = 0 right after it.
2. AUD0PER, AUD1PER, AUD2PER, AUD3PER.
3. AUD0VOL .. AUD3VOL: `(env_out * master) >> 6` (fade/envelope work of each channel is done
   between the writes).
4. For k in 0..3 with a pending load: AUDxLC high word, AUDxLC low word (same time), AUDxLEN.
5. DMACON = 0x800F (every tick, all four channels on).
6. The waveform copy (6.7).
7. For k in 0..3 with a pending second load (samples with loop part): LC high, LC low, LEN.

Pending loads are cleared when written. Samples: LC of the first part, DMA on, then LC/LEN of the
loop part; Paula takes the latter at the end of the first pass.

## 7. Effects (letters; executed in step 7 of 5.2 after note/instrument handling, every row)

Parameters are bytes.

- `A` (0x41) arpeggio: ignored when the channel's note index is 0. slot1 = PT[idx + (p & 15)],
  slot2 = PT[idx + (p >> 4)] (slot0 stays the base period).
- `L` (0x4C): volume = (p * instrument volume) >> 6 (instrument volume = record +6; no clamp).
- `M` (0x4D): scan period = p.
- `V` (0x56): vibrato speed = p >> 4, depth = p & 15, vibrato wave = SINE; index unchanged.
- `F` (0x46): p == 0: set CIA-A PRA bit 1 (LED filter off), else clear it (filter on). Not Paula.
- `S` (0x53): p != 0: speed = p (1..255). p == 0 ignored.
- `O` (0x4F): p != 0: arpeggio rate = p, arp counter = 0. p == 0 ignored.
- `B` (0x42) jump: p == 0: target = position + 1, or 0 if that is >= nPos; p != 0: target = p if
  p < nPos, otherwise target = 0xFFFF (no jump; this also cancels a jump set by an earlier channel
  of the same row). The jump runs at the next row tick (5.1.1). A jump never calls the song-end
  routine; only the normal position wrap in 5.5 does. With several B in one row the last channel
  wins.
- `U` (0x55) fine slide up: if PERIOD_SLOT2 - p >= 0x39 then slide = slide - p, else ignored (slot2 =
  the third arpeggio period, equal to the base period unless effect A was used).
- `D` (0x44) fine slide down: if base period + p <= 0x1AB0 then slide = slide + p, else ignored.
  (U and D change the slide once per row; slide persists until the next note.)
- `G` (0x47): if the instrument byte of this row is 0: volume = (63 * instrument volume) >> 6.
  Always: fade rate = (p * instrument volume) >> 6 (subtracted from the volume every tick; cleared
  by the next instrument start).
- `E` (0x45) glide to the next note: only when the row has a note. Look for the next row (in the
  rest of the same pattern, never wrapping) with a non-zero note byte; if none, nothing happens.
  rowsAhead = number of rows to it; d = rowsAhead * speed; target T = PT[(nextNote + tr) - 1]
  (8-bit add, no clamping; tr as for the current row). With cur = base period of the current note:
  - cur <= T: glide = ceil((T - cur) / d).
  - cur > T:  glide = -(ceil((cur - T) / d)) computed by a 32-bit division quirk (8.1).
  The glide is added to the slide offset every tick (6.1); the parameter is ignored.
- anything else: ignored.

## 8. Quirks that matter for bit exactness

8.1 Glide down divides a 32-bit value whose upper half is leftover register contents: it starts at
0 at every tick and is updated by each downward glide computation of the same tick, in channel
order. For a computation with upper half U (initially 0): X = U*65536 + (cur - T). If U >= d the
division overflows and X is left unchanged; otherwise X = (X mod d)*65536 + (X div d). In both
cases, if X > 0xFFFF the low 16 bits are incremented by one (ceil), glide = -(X & 0xFFFF) as a
16-bit value, and U = X >> 16 for the next computation. Upward glides do not touch U. So the
glide of channel 1 in a row with two downward glides is wrong by a huge factor.

8.2 Vibrato on the lowest note (index 0) with a negative wave value reads the "period below"
PT[-1], which is the pending second-load length word of channel 3 (normally 0; it is non-zero in
the tick in which channel 3 starts a sample: the loop length, or 1); vibrato on the highest
note (index 71) with a non-negative wave value reads PT[72] = 0x0018.

8.3 First scan update delay and the destination quirk: 6.6, 6.7.

8.5 The envelope step already runs in the tick of the note start: after the first tick the level is
the first attack amount (volume register shows `(63*vol>>6)*amount>>6`).

8.6 The arp counter is not reset by notes; slot selection continues from where it was.

## 9. Start and stop

Start (before the first tick), observed writes in order (values; timing in LOGS.md): AUDxVOL =
0 for channels 0..3 twice (volume set, called twice by the host); INTENA = 0x4000; DMACON = 0x000F
twice; for each channel 0..3: AUDxLC = chip buffer B (zero area), AUDxLEN = 0x0010, AUDxPER =
0x03E8, AUDxVOL = 0; DMACON = 0x800F; ADKCON = 0x00FF; CIA-A PRA bit 1 set (byte write 0x02).
Stop: DMACON 0x000F and the same four-channel silent load as above (without the final ones).

## 10. Timing (colour clocks, cc; measured from DELI_WRITE_LOG of the reference)

Ticks are triggered by the host once per frame (about 70940 cc apart, jittering between 69700 and
72200 cc); the time from the trigger to the first write of the tick depends on the work done
before it (in row ticks the row processing) and is not modelled. All values below are gaps from
the previous write of the same tick; they are reproducible to +-1 cc (the reference alternates
between two values one cc apart; half-cycle effect), "~" marks fitted values. Tables from 7
modules (LOGS.md); P/V gaps matched within 1-2 cc in all tested ticks, G8 within about +-15 cc.

### 10.1 Plain ticks (no row processing), the nine writes PER0..3, VOL0..3, DMACON

PER gap before AUDkPER (k = 1,2,3): base by arpeggio event of that channel in this tick:

| k | counter not at rate (no slot change) | slot 0 selected | slot 1 | slot 2 |
|---|---|---|---|---|
| 1 | 155.7 | 194 | 208 | 208 |
| 2 | 171 | 209 | 223 | 223 |
| 3 | 186.7 | 225 | 238 | 238 |

plus, if the vibrato depth != 0: +143, +9 if the wave value is negative, +8.5 per table step i
that the search in 6.2 moved above the note index (arpeggio offset in semitones).

VOL gap (AUDkVOL after the previous write), k = 1..3: cost of the envelope step of that channel:

| channel state | cc |
|---|---|
| no synth block | 99.5 |
| synth, envelope finished (phase 8) | 153 |
| attack / sustain-rise step (phases 0, 4) | 245 (+9 if the level hit 0x3F) |
| decay step, phase 2 | 228.5 (+2 if clamped to 0) |
| release step, phase 6 | 241 (+2 if clamped to 0) |
| each additional phase switch inside the same tick | +77.5 |

AUD0VOL gap (after AUD3PER) = 283.2 + the envelope value of channel 0 from the table (+0..+6 when a
fade clamps a volume at 0).

DMACON gap (after AUD3VOL) when nothing is pending, approximately:
`G = 496 + 21.3*nBlk + 12.3*nCount + 74.8*nAdv + 2227*nFilt + 364.5*nFirst + 1116*nUpd - 1.9*nTrans
+ 27 *nTurnHi + 8.7*nTurnLo` (error up to +-30 cc, mostly < 15)
where nBlk = channels with a synth block, nCount = scanning channels that do not rebuild in this
tick, nAdv = morph phase switches, nFilt = filter passes, nFirst = first copies, nUpd = rebuilds,
nTrans = window bytes with bit 7 (transparent) in rebuilds, nTurnHi/Lo = rebuilds that hit the
upper/lower turn position.

### 10.2 Pending loads and tails

Per pending channel: LC (both words at the same time) -> LEN: 24 cc. LEN -> next channel's LC:
57.5, plus 27.5 for each channel skipped in between. Last LEN -> DMACON: 45 + 27.5 * (3 - k) for
the last pending channel k. The gap from AUD3VOL to the first LC absorbs the morph/scan work: the total AUD3VOL -> DMACON
is the G of 10.1 (the fit covers ticks with pending loads, residual as stated there); the LC/LEN
chain of this paragraph is placed so that it ends 45 + 27.5*(3-k) cc before DMACON.
After DMACON, second loads: DMACON -> LC of the first loop channel k: 368.5 + 27.5*k; LC -> LEN 24;
LEN -> next LC 57.5 (+27.5 per skipped channel).

### 10.3 Row ticks

The DMACON `1<<k` writes of instrument starts and the AUD0PER write come after the row processing
of the channels; observed gaps (cc): single start of channel 0: first write then AUD0PER 1602..1651
(synth start), channel 1: ~1269 (1138 without scan), channel 2: ~924..1035, channel 3: 572..716.
Four synth starts: 717, 717, 717 and 655 (DMACON(ch0) -> DMACON(ch1) -> ... -> AUD0PER). The costs
depend on the instrument block (scan/morph/vibrato fields) and on the effect; they were not
modelled (see README.md).

## 11. Verification summary

Every tick of 17 modules (>9000 ticks) was reproduced: same writes, same order, same values; all
audio DMA words equal the playing waveform except words fetched while the 128-byte copy runs.
