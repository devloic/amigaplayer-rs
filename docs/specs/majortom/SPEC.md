# Major Tom (ARP.) - behaviour specification

*Sanitised for publication: behaviour only.*

Format played by the Eagleplayer/UADE player "MajorTom" ("Major Tom's Player 2.01", by Major Tom of
Classified, adapted by marley/INFECT and Eagleeye/DEFECT). UADE lists the prefixes `hn`, `mtp2`, `thn`, `arp`;
all known files are `*.arp` (suffix style). Slug: `majortom`.

A four-voice synthesiser format: every voice plays 32-byte single-cycle waveforms (or a noise block that
lives in the player), driven by four 64-byte sequences per instrument (volume, waveform, arpeggio,
pitch). One subsong, no title. Paula only; one tick per 50 Hz interrupt.

Numbers: `0x` = hexadecimal; all multi-byte values big-endian; "byte"/"word"/"long" = 8/16/32 bits.
"cc" = colour clocks (the unit of the write logs), "cycles" = CPU cycles (2 cycles = 1 cc).

---

## 1. Detection

The file starts with the four bytes `41 52 50 2E` ("ARP."). That is the player's only check. Bytes 4-5 are
`01 00` in every known file (not checked, not used).

A sensible port should also check that the 13 offsets of 2.1 lie inside the file (the player does not).

## 2. Module format

### 2.1 Header

| offset | size | meaning |
|---|---|---|
| 0 | 4 | "ARP." |
| 4 | 2 | `01 00` (unused) |
| 6 | 13 x 4 | offsets O0..O12, each counted from the start of the file |

What each offset points at (sizes are those of the known files; the player uses only what it indexes):

| offset | contents |
|---|---|
| O0 | byte: song length L (number of positions) |
| O1 | byte: restart position R |
| O2 | word: speed S (ticks per row); must be at an even offset |
| O3 | position list: byte per position = pattern number |
| O4 | transpose list: byte per position, added to every note of that position |
| O5 | instrument table: 4 bytes per instrument (entry 0 is never used: instrument 0 means "none") |
| O6 | volume sequences, 64 bytes each |
| O7 | waveform sequences, 64 bytes each |
| O8 | arpeggio sequences, 64 bytes each |
| O9 | pitch sequences, 64 bytes each |
| O10 | waveforms, 32 bytes each (signed 8-bit samples, one cycle) |
| O11 | patterns, 1024 bytes each |
| O12 | not used (in the known files it points past the end of the file) |

In all nine known files the layout is identical: O0..O12 = 0x13A, 0x13C, 0x13E, 0x140, 0x240, 0x340,
0x440, 0x1440, 0x2440, 0x3440, 0x4440, 0x6240, 0x16240 (so 64 sequences per bank, 240 waveforms, and
as many patterns as the file holds).

### 2.2 Instruments

Instrument i (1..255) is the 4 bytes at O5 + 4 i: volume-sequence number, waveform-sequence number,
arpeggio-sequence number, pitch-sequence number. Sequence number k of a bank B is the 64 bytes at
B + 64 k.

### 2.3 Sequences

Byte 0 of every sequence is its speed (ticks per step); bytes 1..63 are steps. In every sequence the
step value 0xF0 is a jump: the byte after it is the new position (see 4.6). Step meaning per bank:

- volume: the volume (written to AUDxVOL as is, values above 64 included);
- waveform: 0 = no change, 0xFF = noise, any other value = waveform number;
- arpeggio: high nibble = octaves up, low nibble = semitones up (4.7);
- pitch: values 0x00..0x0F are added to the period, values 0x10..0xFF subtract their low nibble.

### 2.4 Patterns

Pattern p is 64 rows of 16 bytes; a row is 4 voice entries of 4 bytes: note, instrument, effect, parameter.
The pattern's offset is O11 + (p x 1024 taken as a signed 16-bit number): patterns 0..31 are where one
expects them, numbers 32..63 land 32768 bytes *before* their expected place (not used by known files).

Note byte: 0 = no note. Otherwise high nibble = octave (1..), low nibble = semitone (0..11). The position's
transpose byte is added (8-bit wrap). Effects: 1 slide up, 2 slide down, 3 portamento, D pattern break,
F set speed; other values do nothing except that the parameter byte always serves as portamento step (4.8).

### 2.5 Metadata

- Subsongs: 1. Title/author: none in the file.
- Length: L positions of 64 rows (D breaks shorten); the song ends when the position counter wraps
  at the end of a pattern (4.5).

---

## 3. State

Global: speed S (16-bit, initial = the word at O2), tick counter (16-bit, initial 4), position (8-bit,
initial 0), row offset (0..1008 in steps of 16, initial 0), song length L, restart R.

Per voice v (0..3): note byte; effect and parameter bytes; four sequences (start offset, position,
counter, speed each), initially an all-zero 64-byte area with position 1, counter 0, speed 0; slide
(16-bit, 0); noise flag (0); portamento current, target (16-bit, 0) and direction (0); new-note flag (0);
arpeggio value (0).

The speed is also written back into the module image (word at O2) when effect F changes it: a port does
not need the module image for that, but must not re-read the original speed afterwards.

## 4. Behaviour

### 4.1 Start

Sound initialisation writes, for all four voices in register order (0..3 each): AUDxLC = the player's
4-byte zero area, AUDxLEN = 2, AUDxPER = 200, AUDxVOL = 0; then DMACON = 0x000F (all four voices off).
Exact clocks are in 5.4. Before it, the host's two volume calls each write AUDxVOL = 0 for the four
voices (5.4).

### 4.2 Tick

Each tick:

1. The tick counter is decremented (16-bit). If it reaches 0: it is reloaded with S, and a row is
   played (4.3), then the row is advanced (4.5).
2. For each voice 0..3 in order: if its new-note flag is set, nothing happens for it this tick;
   otherwise its update runs (4.6..4.12). Then its new-note flag is cleared.
3. DMACON = 0x0000, then DMACON = 0x800F.

So the first row is played on the 4th tick; a voice that gets a note with an instrument writes nothing on
that tick and starts sounding on the next one (DMA is re-enabled by the 0x800F write of every tick).

### 4.3 Row

The row is found from the position: pattern = byte at O3 + position, transpose = byte at O4 + position,
entry of voice v = the 4 bytes at (pattern start) + row offset + 4 v. For each voice 0..3: fetch (4.4),
then if its effect byte is 0x0F: the effect byte becomes 0 and S = the parameter (a parameter of 0 gives a
65536-tick row). The counter was reloaded before the fetch, so a new speed applies from the next row.

### 4.4 Fetch (one voice)

- Note byte 0: effect and parameter = entry bytes 2, 3; new-note flag = 0. Nothing else.
- Note byte n != 0: note = n + transpose (8-bit); portamento current = portamento target; direction = 0.
  - instrument byte 0: effect and parameter = entry bytes 2, 3; new-note flag = 0 (the note changes
    pitch from the next update on, sequences continue).
  - instrument i != 0: effect and parameter = entry bytes 2, 3; the four sequences are set from the
    instrument (start = bank + 64 x number, speed = the sequence's byte 0, counter = 1, position = 1);
    slide = 0; new-note flag = 1; arpeggio value = 1 (overwritten by the first arpeggio step before use).

### 4.5 Row advance and song end

Row offset += 16. When it reaches 1024: row offset = 0, position += 1 (8-bit); if position == L: position =
R and the song end is notified (the host stops a few ticks later; a port reports the end here). Length 0
therefore ends after 256 positions.

### 4.6 Sequence stepping (common rule)

A sequence step happens when the sequence's counter, decremented (8-bit), reaches 0; the counter is then
reloaded with the speed (speed 0 = a step every 256 ticks). The step reads the byte at the current
position; if it is 0xF0, position = the byte after it, and the byte at the new position is read instead
(waveform sequences: see 4.7). After the step the position is incremented; when it becomes 63 it is set to
62 (waveform, arpeggio) or 60 (pitch, volume). A jump to a position of 63 or more is not caught: the
position then keeps counting up and steps read on into the following bytes of the module (positions are
32-bit; bytes outside the module read as 0).

The four sequences are stepped in the order waveform, arpeggio, (period computation), pitch, volume.

### 4.7 Waveform step

- Value 0: nothing (only the position advances).
- Jump (0xF0): position = next byte as in 4.6, but the value used is *not* the byte at the new position:
  it is the byte at module offset (sequence start + X) modulo 2^24, where X is the long at bytes 24..27 of
  the waveform sequence itself (a defect of the original). Offsets outside the module read as 0. This
  value is then handled as below even if it is 0 (waveform 0).
- Value 0xFF (noise): DMACON = voice bit (1 << v, voice off); AUDxLC = noise block + 2 x (volume
  sequence position); AUDxLEN = 700 (0x2BC); AUDxPER = 200; noise flag = 1.
- Other value w: if the noise flag is set: DMACON = voice bit, noise flag = 0. AUDxLC = start of waveform
  w (O10 + 32 w).

AUDxLEN for waveforms is written by the pitch step (4.11).

### 4.8 Arpeggio step and period

Arpeggio step: arpeggio value = the step byte (jumps per 4.6).

Period (every update): with note byte n and arpeggio byte a,
semitone s = (n & 15) + (a & 15), octave o = (((n - 16) & 255) >> 4) + (a >> 4), table index = s + 12 o.
The period table (word values, index 0..87):

```
0D40 0C80 0BD0 0B20 0A80 09F0 0960 08D0 0850 07E0 0770 0700
06A0 0640 05E8 0590 0540 04F8 04B0 0468 0428 03F0 03B8 0380
0350 0320 02F4 02C8 02A0 027C 0258 0234 0214 01F8 01DC 01C0
01A8 0190 017A 0164 0150 013E 012C 011A 010A 00FC 00EE 00E0
00D4 00C8 00BD 00B2 00A8 009F 0096 008D 0085 007E 0077 0070
00D4 00C8 00BD 00B2 00A8 009F 0096 008D 0085 007E 0077 0070
0070 0070 0070 0070 0070 0070 0070 0070 0070 0070 0070 0070
0070 0070 0070 0070
```

Indices past 87 read whatever follows the table in the player's memory: index 88 = 0x4246; indices
89..103 = the 15 words of `data/period-overrun.bin` (reference host); index 180 = 0x0076 (reference
host); any other index: unknown, use 0. Index 180 is what a voice computes before its first note (note
byte 0, arpeggio 0); it matters because the portamento target takes this value (4.9). Indices up to 390
are reachable.

The table value becomes the portamento target.

### 4.9 Portamento

The step is always the voice's current parameter byte (whatever effect the current row has).
- If the direction is 0 and the effect is 3: direction = 1 if (target - current) as a signed 16-bit
  number is >= 0, else 2.
- Direction 1: current += step; if (current - target) signed >= 0: current = target.
  Direction 2: current -= step; if (target - current) signed >= 0: current = target.
  The period used is then `current`.
- Direction 0 (and no effect 3): the period used is the target.
The direction stays set until the next note (4.4), even when later rows have no effect 3: the voice keeps
gliding towards (and then follows, with the same rule) the current target.

### 4.10 Effects in the update

In this order, after the portamento:
- D (pattern break): the effect byte becomes 0; row offset = 0; position += 1 (8-bit); if position >= L
  (unsigned), position = 0. No song end. If several voices break in the same tick, each one advances the
  position. A break on a voice that got a new instrument in this row takes effect one tick later (the
  update is skipped on that tick).
- 2: slide += parameter. 1: slide -= parameter (16-bit, every update tick while the effect stays).
Then period = period + slide (16-bit).

### 4.11 Pitch step

Only on a pitch-sequence step: value v < 0x10: period += v; else period -= (v & 15). Then AUDxPER =
period; then, if the noise flag is clear, AUDxLEN = 16. Between pitch steps no period is written: the
arpeggio, portamento and slides of those ticks are computed but only reach Paula at the next pitch step.

### 4.12 Volume step

Only on a volume-sequence step: AUDxVOL = (value x V) >> 6, 16-bit, where V is the host's voice volume
(64 by default, so the value itself; the reference never uses another V).

---

## 5. Timing

### 5.1 Clock model

Ticks are called every 70 940 cc. For a tick called at clock C keep a cycle count T starting at 0. A write
gets the stamp C + floor(T / 2), where T already includes the step that makes the write. The song-end
notification rounds T down to even (5.3). Costs are fixed (no data or branch-direction dependence).

Voice selection costs, used below: Sv = 23, 54, 85, 93 and Wv = 107, 130, 153, 143 for voices 0..3.

### 5.2 Tick ledger (cycles)

| step | cost |
|---|---|
| tick entry and counter decrement | 409 |
| counter reached 0 (reload) | 28 |
| per voice: locate the entry | 180 (voice 0) or 188 (voices 1..3) |
| fetch: read note | 19; note 0: + 56, fetch done |
| fetch: note | + 75; instrument 0: + 56, fetch done; instrument: + 594 |
| speed-effect test after each fetch | 25; effect F: + 52 |
| row advance | 73; pattern end: + 73; position == L: + 60, round T down to even, + 16 |
| per voice: new-note test | 37; update: + 30 + update cost (5.3); then + 16 |
| DMACON 0x0000 | 20 -> write |
| DMACON 0x800F | 20 -> write |

### 5.3 Voice update ledger (cycles)

| part | cost |
|---|---|
| waveform counter | 27; no step: go to arpeggio |
| waveform step | 93; value 0: go to "advance" |
| value test | 17; jump: + 62; then + 25 |
| noise | 12 -> DMACON; 60 -> AUDxLC; 32 -> AUDxLEN; 16 -> AUDxPER; + 134 + Sv |
| waveform | 25; noise flag set: 12 -> DMACON, + 16; then 52 -> AUDxLC; + 48 + Sv |
| advance | 57; became 63: + 24 |
| arpeggio counter | 25; step: + 83; jump: + 28; + 50; + 57; became 63: + 24 |
| period | 255 |
| portamento | direction 0: 25; effect 3: + 41, then + 26 (direction 1) or + 16 (direction 2). If the direction is (now) set: + 94, then + 12 (target not reached) or + 42 (reached) |
| pattern break | 25; effect D: + 101; position wrapped to 0: + 20 |
| slide down test | 25; effect 2: + 32 |
| slide up test | 25; effect 1: + 32 |
| apply slide | 12 |
| pitch counter | 39; step: + 83; jump: + 62; + 25; value < 0x10: + 14, else + 12; 44 + Sv -> AUDxPER; + 25; noise flag clear: 16 -> AUDxLEN, + 16; + 69; became 63: + 24 |
| volume counter | 25; step: + 83; jump: + 62; 104 + Wv -> AUDxVOL; + 101; became 63: + 24 |
| return | 16 |

(A "->" cost is added and then the write is stamped.)

### 5.4 Start-up

The host makes these calls in order; their clocks do not depend on the module up to sound init:
check (clock 100), initialisation (130), info (236), two volume calls (248 and 924), sound init (1600).
The volume calls write AUD0VOL..AUD3VOL = 0 at clocks 401, 553, 717, 875 and again at 1077, 1229, 1393,
1551.

Sound init, called at clock 1600, cycle count T from 0:
- 3852, then a scan of L + 1 bytes starting at O1 (the restart byte and what follows it): 31 per byte,
  + 4 when the byte is >= the largest byte seen so far (unsigned, starting from 0); then + 40;
- AUD0LC..AUD3LC: 16 -> write each (both words, same stamp); + 4;
- AUD0LEN..AUD3LEN = 2: 12 -> write each; + 8;
- AUD0PER..AUD3PER = 200: 12 -> write each; + 4;
- AUD0VOL..AUD3VOL = 0: 12 -> write each;
- + 576; DMACON = 0x000F: 16 -> write; + 140: sound init returns at 1600 + floor(T / 2).

The first tick is called 70 940 cc after sound init returns. Example (log r.e.a.l, L = 12, scanned bytes 00 00 00 03
00 01 00 04 02 03 02 07 05): AUD0LC written at 3767, first tick at 75 177.

## 6. Data not in the module

- Noise block: 3000 bytes in the player, `data/noise.bin`. Noise plays 1400 bytes from offset
  2 x (volume-sequence position); positions up to 62 stay inside the block.
- The 4-byte zero area used at start-up: in the reference it lies directly after the noise block
  (noise block + 3000). Keeping that relation makes the address comparison of the log tool line up.
- Period table index 88 (0x4246), indices 89..103 (`data/period-overrun.bin`) and 180 (0x0076): see 4.8.
  These are bytes of the reference host's memory layout, not of the format.
