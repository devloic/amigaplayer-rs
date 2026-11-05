# Professional Sound Artists (PSA) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Amiga music format of Dave "Sinbad" Hasler (Professional Sound Artists, 1990; game music such as
The Punisher), file name prefix `psa.`, as played by the reference (the Eagleplayer "Professional Sound
Artists" by Wanted Team, player module V1.1, which carries the replay routine of the game; module files are
plain data and contain no code). This is a description of observed behaviour (write logs of the reference
run through the host, module data) in my own words. It contains no instructions or addresses of the
original. "cc" is the Amiga colour clock (1 cc = 2 CPU cycles); costs are given in 68000 CPU cycles as the
host charges them.

A value model written from this description (private, with the cost constants of section 7) reproduces
the reference register writes (address, value, order and colour clock of every write) exactly for all 4 real
runs (3 modules, 4 subsongs; 5 261 ticks) and for 117 synthetic (module, subsong) runs (about 103 650
ticks); see LOGS.md. The state machine is small (four voices, one counter, a handful of effects); the
timing is fully determined by the instruction counts, except for two raster waits (7.5).

## 1. Module file

### 1.1 Detection

The file starts with the four bytes `50 53 41 00` ("PSA" and a zero byte). Nothing else is checked. The
prefix of the file name is `psa.` (and `.psa` as suffix is seen in the corpus).

### 1.2 Header and tables (all values big-endian)

| offset | size | meaning |
|---|---|---|
| 0 | 4 | magic |
| 4 | 32 | free text, padded with spaces ("Punisher Game Over By Steve/PSA "); the Eagleplayer shows it as the module's special info; it is the only title |
| 36 | long | total size (equals the file size in all modules seen) |
| 40 | long | offset of the instrument table |
| 44 | long | offset of the position list |
| 48 | long | offset of the pattern data |
| 52 | long | not used by the replay (0x00010000 in two modules, 0x00020009 in another) |
| 56 + 8 s | 4 words | subsong record s: first position, end position, restart position, unused (0) |

- Subsong count = (instrument table offset - 56) / 8 (1 or 2 seen). Subsongs are numbered from 0.
- The three position values of a subsong are byte offsets into the position list (steps of 2). The end
  value is the offset just after the last position played. The restart value is 0xFFFF for "stop at the end"
  or an offset to continue from.
- Length shown for a subsong = (end - first) / 2 positions.
- Instrument table: 64-byte records, count = (position list offset - instrument table offset) / 64 (rounded
  down). Instrument numbers in the patterns start at 0.
- Position list: big-endian words, each the number of a pattern.
- Pattern n starts at (pattern data offset + 1024 n). A pattern has 64 rows; a row is 16 bytes, four
  4-byte entries for voices 0 to 3. Entry: note (0 = none, else 1 to 37), instrument number, effect,
  parameter.
- Sample data are anywhere in the file (after the patterns in all modules seen); instruments refer to them by
  offsets from the start of the file.
- For its info display the Eagleplayer also counts instruments with a non-zero sample offset ("samples", sum
  of 2 x length for those whose offset is larger than every earlier one), the others as "synth", and
  patterns = (total size - that sample sum - pattern data offset) / 1024; none of this affects the replay.

### 1.3 Instrument record (64 bytes)

| offset | size | meaning |
|---|---|---|
| 0 | long | sample start (offset from the start of the file; used by sample instruments) |
| 4 | word | length in words (sample length, or the synth waveform window length) |
| 6 | long | loop start (offset from the start of the file), 0 = no loop |
| 10 | word | loop length in words |
| 12 | word | volume |
| 14 | byte | 0 = sample instrument, any other value = synth instrument |
| 15 | byte | synth: initial offset of the window |
| 16 | byte | synth sweep: step while sweeping forward |
| 17 | byte | synth sweep: delay (ticks) between forward steps, also the first delay |
| 18 | byte | synth sweep: count of forward steps before turning (also the first count), and reload value of the backward count |
| 19 | byte | synth sweep: step while sweeping backward |
| 20 | byte | synth sweep: delay between backward steps |
| 21 | byte | synth sweep: reload value of the forward count |
| 22 | byte | envelope: initial phase (0 = no envelope; 1 = attack is the normal value) |
| 23 | byte | envelope attack: volume step per interval |
| 24 | byte | envelope attack: interval (ticks) |
| 25 | byte | envelope attack: number of steps |
| 26 | byte | envelope decay: volume step |
| 27 | byte | envelope decay: interval |
| 28 | byte | envelope decay: number of steps |
| 29 | byte | envelope sustain: number of ticks to hold (0 = hold forever) |
| 30 | byte | envelope release: volume step |
| 31 | byte | envelope release: interval |
| 32 | byte | envelope release: number of steps |
| 33 | 31 | instrument name (up to 30 characters, zero padded) |

Only byte 14 decides sample or synth; bytes 0 and 6 are ignored for a synth instrument. Sample instruments
ignore bytes 15 to 21.

## 2. Host side

### 2.1 Tick rate and calls

The host calls the replay routine ("tick") once per 70 940 cc (50 Hz) after the player has been started with
the chosen subsong. The first tick of a run starts a fixed time after the end of the init (section 8); every
later tick starts exactly 70 940 cc after the previous one. All times below are relative to the start of the
interrupt call of the tick, "tick start", unless said otherwise.

A tick has three parts, always in this order: the music step (section 4.1 to 4.8), a wait for nine raster
line changes, the DMA step (4.9, which starts with a write of DMACON and contains a second raster wait).

### 2.2 Volume and balance of the host

The host scales every audio volume write by a factor (left voices 0 and 3, right voices 1 and 2, each
factor 64 for full volume and centre balance, as in all reference runs): the value written is
(volume AND 0x7F) x factor / 64, i.e. unchanged. Muting a voice in the host makes its volume writes 0. Values
written can therefore be as large as 127 (Paula treats 64 and above as 64); they are not clamped by the player.

### 2.3 Song end notification

The report "song end" is a call into the host (section 4.10); in the reference run it stops the run: the tick
in which the first report is made is the last tick of the log.

## 3. Player state

Global: running flag (1 after init), speed (6 after init; ticks per row = speed + 1), step counter byte (0
after init), row (0..63), current position offset (starts at the subsong's first position), end and restart
position of the subsong, the DMA bit set (starts as 0x8000 = "set" bit only), the phase counter (0..2, starts
at 0), a pending-loop flag per voice, pointers to the pattern row being played and to the next row.

Per voice (all start as zero): base period; note index (0-based); portamento speed (signed word) and target
period; volume (word); vibrato step (signed word), accumulator (word), counter and reload (bytes); the eight
synth bytes copied from instrument bytes 14 to 21 (enable, window offset, forward step, forward delay,
backward reload count... in that order), the two sweep counters, the sweep direction flag (0 forward, 0xFF
backward); the envelope counter and the 12 envelope bytes copied from instrument bytes 22 to 33; the loop
start address and loop length written after a sample note starts.

Period table (37 words, note n uses entry n - 1; the first entry is the lowest note):

`358 328 2FA 2D0 2A6 280 25C 23A 21A 1FC 1E0 1C5 1AC 194 17D 168 153 140 12E 11D 10D 0FE 0F0 0E2 0D6 0CA 0BE 0B4 0AA 0A0 097 08F 087 07F 078 071 06B` (hex).

Synth waveform memory (player data, section 6): 256 bytes of 0x80 immediately followed by 256 bytes of 0x7F,
then 4 zero bytes. A synth instrument plays a window of 2 x length bytes whose end can sit anywhere in the
0x7F part: the base of the window is (boundary between the two parts) - 2 x length; the playing start
address is that base plus the current window offset (0..255). The 4 zero bytes are the "silent loop" used by
sample instruments without a loop. Addresses written for these are addresses inside the player's own data;
a port may use any constants that keep the same relations (the checking tool accepts one constant offset).

## 4. What a tick does

### 4.1 Music step

If the running flag is 0 (after the end of the module, 4.10) nothing happens in this part. Otherwise:

1. The step counter is decremented as a byte. If the result has bit 7 set (it starts at 0 so this is true on
   the first tick, and after the reload by `speed` it is true again `speed + 1` ticks later), a new row is
   processed (4.2), which includes reloading the step counter with `speed` at its end.
2. The phase counter is incremented; when it reaches 3 it becomes 0. So the phases used are 1, 2, 0, 1, 2, 0, ...
   starting with 1 on the first tick. It is global (not reset by rows).
3. Voices 0, 1, 2, 3 are processed in this order (4.5 to 4.8) with the phase and the row being played (the one
   read by the latest new-row step; on a new-row tick that is the row just read).

### 4.2 New row

The row pointer is taken from the "next row" pointer, remembered as the row being played, and the four voice
entries of that row are decoded in the order 0, 1, 2, 3 (4.3 and 4.4). Each voice yields a "loop pending" flag
(1 only when a sample note was started, else 0), stored per voice. Then the song position is advanced (4.10):
the row number is incremented modulo 64; when it wraps to 0 a new position is entered; the step counter is
set to `speed`; the "next row" pointer is the pointer after the four entries just read (or the first row of
the new pattern after a position change).

### 4.3 Decoding a row entry (voice n)

Fields: note, instrument, effect, parameter. Let "effect has bit 7" mean effect >= 0x80.

- Note = 0: only the effect checks of 4.4.3 are made (no period change, no sound start).
- Note != 0 and effect has bit 7: period only: note index = note - 1, the period of the table is written to
  AUDxPER and kept as the voice's base period (steps 4.4.2); no DMA stop, no sound start. The effect checks of 4.4.3 are executed (they cost time, see 7.4) but cannot match, because the effect byte is 0x80 or more.
- Note != 0 and effect = 3 (tone portamento): the target period becomes table[note - 1]; the portamento speed
  becomes `-parameter` (as a 16-bit word) if the target is greater than or equal to the current base period
  (difference taken as a signed 16-bit word, zero or positive), else `+parameter`. Nothing is written, the note
  index and the base period do not change, and the effect checks of 4.4.3 are NOT made.
- Any other note (effect 0..2, 4..127 except 3): a new sound starts (4.4.1), then the period is set (4.4.2),
  then the effect checks (4.4.3).

### 4.4 Starting a note

#### 4.4.1 New sound

Let I be the instrument record number `instrument` (no bounds check). In this order:

1. DMACON is written with the single bit of this voice (1, 2, 4 or 8): the channel is switched off. The bit
   is also remembered to be set in the DMACON write of the DMA step (4.9).
2. Envelope bytes 22 to 33 and synth bytes 14 to 21 of I are copied into the voice; the envelope counter is
   set to 1.
3. Synth instrument (byte 14 != 0): sweep direction = forward, first delay counter = byte 17, first count =
   byte 18. AUDxLEN is written with the word at byte 4. The window base is
   boundary - 2 x length (word arithmetic), remembered as the voice's waveform base; the start address is that
   base + byte 15, and AUDxLC (high word, then low word, same clock) is written with it. The pending flag is 0.
   Then the volume (below).
4. Sample instrument: the pending flag becomes 1. AUDxLC (two words) is written with (module load address +
   long at byte 0); then AUDxLEN with the word at byte 4. Then the volume (below). Then the loop start is
   remembered: module load address + long at byte 6, or the silent loop if that long is 0; the loop length is
   the word at byte 10.
5. Volume: with effect = 12 the volume is the parameter, else the word at byte 12. It is remembered as the
   voice's volume and AUDxVOL is written (4.8). With effect 12 this is a second volume write: the effect check
   of 4.4.3 writes it again.
6. The loop length (word at byte 10) is stored for both kinds of instrument (it is only used for sample
   instruments).

Order of the writes for a sample note: DMACON, AUDxLC hi, AUDxLC lo, AUDxLEN, AUDxVOL, then (4.4.2) AUDxPER.
For a synth note: DMACON, AUDxLEN, AUDxLC hi, AUDxLC lo, AUDxVOL, AUDxPER.

#### 4.4.2 Setting the period

note index = note - 1; base period = table[note index]; AUDxPER is written with it. This is the last write of
the note start (for a bit-7 entry it is the only write).

#### 4.4.3 Effect checks after a row entry

Made for note = 0 (any effect) and for new-sound notes (effect without bit 7), in this order:

- effect 5: the envelope's sustain counter is set to 1 (so that a running sustain ends in the next envelope
  step; no write);
- effect 12 (volume): the volume becomes the parameter, remembered and written (4.8); the checks end here;
- effect 13 (pattern break): the row number is set to 63, so that the position advance of this row (4.10)
  enters the next position; the checks end here;
- effect 15 (speed): `speed` becomes the parameter (it is used by the step counter reload of this very row);
- other effects: nothing.

Note: an entry with bit 7 in the effect field and no note falls under "note = 0" with an effect that cannot be
5, 12, 13 or 15: nothing happens.

### 4.5 Per-tick voice processing

For voice n, with effect e = (effect field of the row being played) AND 0x7F, parameter p, and the phase f of
the tick (4.1):

- e = 0 and p != 0 (arpeggio): no plain period write. f = 0: AUDxPER = base period; f = 1: AUDxPER =
  table[note index + (p >> 4)]; f = 2: AUDxPER = table[note index + (p AND 15)]. (An index above 36 reads
  beyond the table, section 9.)
- e = 4 (vibrato): no plain period write. If the step counter equals `speed` (the first tick after the row
  was read) and p != 0: step = p >> 4 (as a word), counter = p AND 15, reload = 2 x (p AND 15) (byte
  arithmetic), accumulator = 0. Then in every tick: accumulator += step (word); AUDxPER = base period +
  accumulator (word); counter -= 1 (byte); when the counter is 0: counter = reload and step = -step.
- every other case: first a plain write: AUDxPER = base period (written every tick even when the voice never
  played, in which case 0). Then, by effect:
  - e = 10 (volume slide): if p has bit 7 clear: volume = volume + p (word); if the low byte of the volume
    minus 0x40 has bit 7 clear (low byte in 0x40..0xBF), the low byte is replaced by 0x40 (the upper byte is
    kept); if p has bit 7 set: volume = volume - (p AND 0x7F) (word), and 0 if the result is negative as a
    word. Then AUDxVOL is written (4.8) and the volume remembered.
  - e = 1 (slide up): period = base period - p (word), AUDxPER written with it, and kept as base period.
  - e = 2 (slide down): the same with +p.
  - e = 3 with a non-zero portamento speed s (word): new = base period - s (word). If s is positive the target
    is reached when (new - target) as a 16-bit word has bit 15 set; if s is negative it is reached when that
    difference has bit 15 clear. When not reached: AUDxPER = new, base period = new. When reached: AUDxPER =
    target, base period = target, speed = 0.
  - other effects (including 5, 12, 13, 15 and unknown): nothing more.

### 4.6 Synth sweep

Done for each voice after 4.5 whenever the voice's synth enable byte is non-zero. The enable byte is the copy of
instrument byte 14 made by the last new sound on that voice (so a sample note switches the sweep off; a
portamento or bit-7 entry does not change it). The first delay counter is decremented (byte); when it reaches 0:

- forward (direction 0): delay counter = forward delay (byte 17); offset += forward step (byte 16, byte
  arithmetic); the second counter is decremented and when it reaches 0 it is reloaded with byte 21 and the
  direction flips;
- backward: delay counter = backward delay (byte 20); offset -= backward step (byte 19); the second counter
  is decremented and when it reaches 0 it is reloaded with byte 18 and the direction flips.

Then AUDxLC (two words) is written with the waveform base + offset (offset 0..255). The sweep writes only
the address, never the length. (All counters are bytes; a counter that starts at 0 wraps and takes 256 steps.)

### 4.7 Envelope

Done for each voice after 4.6 whenever the envelope phase byte is non-zero. The envelope counter is
decremented (byte); when it reaches 0, by phase:

- phase 1 (attack): counter = attack interval (byte 24); the low byte of the volume word += attack step (byte
  23, byte arithmetic, the upper byte of the word unchanged); the attack count (byte 25) is decremented
  and at 0 the phase becomes 2; then the volume is written (below).
- phase 2 (decay): counter = byte 27; low byte -= byte 26; byte 28 is decremented, at 0 phase 3; write.
- phase 3 (sustain): counter = 1; no volume change and no write; if the sustain counter (byte 29, or 1 after
  effect 5) is non-zero it is decremented and at 0 the phase becomes 4; a counter of 0 holds forever.
- any other phase (4 normally): counter = byte 31; low byte -= byte 30; byte 32 is decremented, at 0 the phase
  becomes 0 (envelope off); write.

Volume write after a phase 1, 2 or release step: if bit 7 of the new low byte is set the whole volume
becomes 0; the volume word is remembered and AUDxVOL is written (4.8). Note that a volume that goes above
127 in the low byte becomes 0 (no clamp at 64).

### 4.8 Volume write

AUDxVOL = ((volume AND 0x7F) x factor) / 64 with the factor of the voice (2.2): an unchanged value in
the reference runs. This applies to every volume write above.

### 4.9 DMA step

1. DMACON is written with 0x8000 OR the bits of the voices whose note was started since the last DMA step
   (set bits start the channels); then the remembered bits are cleared to 0x8000. This write is made in
   every tick, also when nothing was started (value 0x8000).
2. Raster wait (7.5).
3. For each voice in the order 0..3 whose pending-loop flag is set: the flag is cleared and AUDxLC (two
   words, same clock) is written with the voice's loop start address, then AUDxLEN with the loop length
   (the loop start is the silent loop when the instrument has none).

### 4.10 Position advance and song end

Ends every new-row step, after the four entries have been decoded:

- row = (row + 1) mod 64. If the row is not 0 now, the next row is the one after the four entries just read.
- If the row is 0: p = position + 2. If p equals the end position of the subsong, the song end is reported to
  the host and p becomes the restart position; if that is 0xFFFF the report is made a second time and the
  module stops (below). Otherwise (p different from the end position, or a real restart position) the position
  becomes p, and the next row is row 0 of the pattern given by the position list word at p.
- The step counter is set to `speed` (in the stop case too; `speed` may have been changed by an effect 15 of
  this very row).

The stop (restart 0xFFFF):

- the running flag becomes 0;
- DMACON = 0x000F is written (all four channels off), then AUDxVOL = 0 for voices 0, 1, 2, 3 in that order;
- the music step goes on to process the four voices of that last row as usual (their writes follow), and from
  the next tick on the music step does nothing (only the DMA step runs: DMACON 0x8000 each tick). In the
  reference run the run ends with this tick (2.3).

The report is a host call: cost and clock effect in 7.6. The position test is an equality: an end position that
the position never equals (for example an odd offset) never ends the song.

## 5. Order of the writes of a tick

1. Music step: for each new-row voice entry in the order 0..3 the writes of 4.4; then for each voice 0..3 the
   writes of 4.5, 4.6 and 4.7 in this order (period or vibrato/arpeggio write, volume write for effects 10,
   second period write for slides and portamento, sweep address, envelope volume).
2. DMACON (0x8000 + bits).
3. For each pending voice, LC then LEN of the loop.

AUDxLC is always written high word first, both words with the same clock.

## 6. Data that lives in the player

Period table: section 3 (37 words). Synth waveform memory: 512 bytes (256 x 0x80, 256 x 0x7F) followed by 4
zero bytes; the exact bytes are in `data/psa_synth_wave.bin` (the lead decides on embedding). In the
reference run this memory sits at 0x6BE2A8 (boundary at +0x100, silent loop at +0x200); the log addresses of
synth notes and loops are those. The module load address in the reference runs is 0x45E0.

## 7. Timing in cycles

### 7.1 Rule

Every instruction of the original has a fixed cost in CPU cycles (the host's table: the conditional branches
cost the same whether taken or not, a multiplication always 62). The clock of a write is

    tick start + floor(C / 2)

where C is the number of cycles from the start of the interrupt call up to and including the cycles of the
instruction that performs the write. A write of a 32-bit value (AUDxLC) is two word writes with the same
clock. "Segment" below means the cycles of a straight run of the original between two points; the
tables list the segments in the order in which they are charged and mark the writes.

Segments that depend on the voice number n (0 to 3): the helper that sets one hardware register is reached
through a chain whose length depends on n, therefore:

| from the call of the helper to the write, inclusive | n = 0 | n = 1 | n = 2 | n = 3 |
|---|---|---|---|---|
| AUDxLC (both words) | 67 | 98 | 129 | 137 |
| AUDxLEN | 63 | 94 | 125 | 133 |
| AUDxPER | 63 | 94 | 125 | 133 |
| AUDxVOL (volume scaling, up to the write) | 174 | 197 | 220 | 210 |
| after the AUDxVOL write, until the caller continues (includes the second helper that records the volume) | 113 | 144 | 175 | 183 |

After the AUDxLC, AUDxLEN and AUDxPER writes the helper needs 28 cycles to return to the
caller. The caller's cost of calling the helper is part of the neighbouring segments below. In this section
"VOL(n)" is the volume sequence: the AUDxVOL row, the write, the "after" row.

### 7.2 Frame

| segment | cycles |
|---|---|
| interrupt entry up to the start of the music step test | 242 |
| music step, stopped (running flag 0): test, return | 41 + 28 |
| music step, running: test, entry, step-counter decrement | 41 + 162 + 9 |
| (if the counter went negative) call of the new-row step | 18, then 7.3 |
| phase counter update | 33 (+ 4 when the phase wraps 2 to 0) |
| set-up for voice 0 and its call | 66 |
| voice 0 processing | 7.4 |
| set-up for voice n = 1, 2, 3 and its call, before each | 34 |
| after voice 3 (restore, return from the music step) | 176 |
| call of the first raster wait | 18 |
| first raster wait | 7.5 |
| call of the DMA step | 18 |
| DMA step up to and including the DMACON write | 56 (write) |
| from the DMACON write to the call of the second wait | 34 |
| second raster wait | 7.5 |
| voice with no pending loop, per voice | 21 |
| voice with a pending loop: to the AUDxLC write (inclusive) | 69 (write LC) |
| ... then to the AUDxLEN write (inclusive) | 24 (write LEN) |
| after the last voice, return | 44 |
| interrupt exit | 168 |

### 7.3 New-row step (after the call)

| segment | cycles |
|---|---|
| set-up and call of voice 0's entry | 82 |
| between voices (store flag of the previous, set-up and call of the next) | 12 + 34 |
| after the last voice's entry | 12 |
| position advance: row increment and test | 45 |
| (row wrapped to 0) position test and next position computed | 53 |
| (song end reported, 4.10) call of the host report, then the report | 18, then 7.6 |
| ... then read of the restart position and test | 29 |
| (restart is 0xFFFF) second report | 18, then 7.6 |
| ... call of the stop sequence, the sequence, jump over the pattern look-up | 18, 7.7, 10 |
| (a position is entered) look up the new pattern | 88 |
| store the row pointer, reload the step counter, call the info update | 54 |
| info update (fixed) | 384 |
| return | 16 |

### 7.4 Voice entry decoding and note start

Entry decoding (voice n, called as above): read the four bytes 63. Then:

| case | cycles after the read |
|---|---|
| note 0 | straight to the effect checks |
| note != 0 | 15 (bit 7 test) |
| ... bit 7 set | straight to 4.4.2 (period) |
| ... bit 7 clear | + 17 (portamento test) |
| ... effect 3, target >= current | 55 + 26, then the end |
| ... effect 3, target < current | 55 + 22, then the end |
| ... other: new sound | note start sequence below |

Note start (sample instrument), in order (segments, then the write that ends them):

| segment | cycles | write |
|---|---|---|
| (before: 63 + 15 + 17) | | |
| DMACON stop | 16 | DMACON |
| parameter copy | 233 | |
| sample address set-up and call | 102 | |
| AUDxLC helper | table 7.1 | LC hi, LC lo |
| return, length set-up and call | 28 + 30 | |
| AUDxLEN helper | table 7.1 | LEN |
| return, effect-12 test | 28 + 29 | |
| volume set-up: effect 12 / other | 34 / 46 | |
| VOL(n) | 7.1 | VOL |
| restore: effect 12 / other | 34 / 24 | |
| loop test | 25 | |
| loop start: none / set | 30 / 64 | |
| loop length store | 20 | |

Note start (synth instrument):

| segment | cycles | write |
|---|---|---|
| ... up to the parameter copy: as above | | DMACON |
| synth set-up (counters, window length, call) | 114 | |
| AUDxLEN helper | table 7.1 | LEN |
| return, window base and offset, call | 28 + 98 | |
| AUDxLC helper | table 7.1 | LC hi, LC lo |
| return, effect-12 test | 28 + 29 | |
| volume set-up: effect 12 / other | 34 / 46 | |
| VOL(n) | 7.1 | VOL |
| restore: effect 12 / other | 34 / 34 | |
| loop length store | 20 | |

Period set (after either, and for bit-7 entries directly): 34 + 18, AUDxPER
helper (7.1, write), return 28, then 12.

Effect checks: 17 (+ 16 if effect 5), then 17; effect 12: 34,
VOL(n), 34, end. Otherwise 17 (+ 26 if 13, end), else 17
(+ 12 if 15). Entry end: 20.

### 7.5 Raster wait

A wait is: a fixed entry of 28 cycles, then up to nine repetitions of: read of the beam line
(16 cycles; the read happens at the end of that segment), then repeated re-reads of
(16 + 9 cycles each, the read at the end of the first part) until a read
returns a line different from the first read of this repetition, then 10 cycles to the next
repetition; after the ninth repetition: 44 cycles. The line value is the low 8 bits of
floor(clock / 227) where the clock is the tick start plus floor(C / 2) at the moment of the read (the beam
position is the clock, lines of 227 cc, no offset; only the changes matter). A wait therefore lasts about
nine lines (2 043 cc) plus the time to the first change.

### 7.6 Song-end report (host call)

The report costs the cycles of its call segments plus the host: first 72 cycles (including the
call itself), then the host does not charge any cycles but re-bases the clock: if C is odd at that moment it
is reduced by one cycle; then 16 cycles are added; then 44 cycles follow.

### 7.7 Stop sequence

56 cycles to the DMACON write (0x000F), then for voices 0 to 3 20 cycles each to
the AUDxVOL write (0), then 28 cycles.

### 7.8 Per-voice processing (music step, after the set-up of 7.2)

Start: 45 (read of effect and parameter, first test). When the parameter p is not 0 the arpeggio
test follows (13); it is charged for every case below with p != 0 (it is part of the first
three rows). Then by case:

| case | segments (cycles); writes marked |
|---|---|
| arpeggio, phase 0 | 13 + 13 + 42; PER helper (7.1); 28 + 22 |
| arpeggio, phase 1 | 13 + 13 + 37 + 66; PER helper; 28 + 22 |
| arpeggio, phase 2 | 13 + 13 + 37 + 60; PER helper; 28 + 22 |
| vibrato | (13 if p != 0) + 17 + 33 (+ 13 when the step counter equals `speed`) (+ 90 when initialised) + 48 + 34; PER helper; 28 + 39 (+ 46 when the counter reached 0) |
| all other effects: common prefix P | (13 if p != 0) + 17 + 20 (write PER) + 17 |
| volume slide up | P + 13 + 33 (+ 8 if clamped) + 34; VOL(n); 34 |
| volume slide down | P + 13 + 33 (+ 4 if negative) + 34; VOL(n); 34 |
| slide up (1) | P + 17 + 50; PER helper; 28 + 34 |
| slide down (2) | P + 17 + 17 + 50; PER helper; 28 + 34 |
| portamento (3), speed 0 | P + 17 + 17 + 17 + 21 |
| portamento, speed > 0, not reached | P + the three compares + 21 + 9 + 37 + 18; PER helper; 28 + 12 |
| portamento, speed < 0, not reached | P + the three compares + 21 + 9 + 37 + 18; PER helper; 28 + 12 |
| portamento, reached | P + the three compares + 21 + 9 + (37 for speed > 0, 37 for speed < 0) + 42; PER helper; 28 + 58 |
| any other effect (0 with p = 0, 5, 6..9, 11..15 ...) | P + the three compares |

"The three compares" are 17 + 17 + 17. Then, for every voice, the synth sweep and the
envelope:

| part | segments |
|---|---|
| sweep, voice not synth | 21 |
| sweep, synth, delay not elapsed | 21 + 25 |
| sweep step forward | 21 + 25 + 37 + 57 (+ 46) + 70; LC helper; 28 + 12 |
| sweep step backward | 21 + 25 + 37 + 57 (+ 36) + 70; LC helper; 28 + 12 |
| envelope, phase 0 | 23 |
| envelope, counter not elapsed | 23 + 27 |
| envelope step, common part | 23 + 27 + 29 |
| ... attack | common + 57 (+ 26 at count 0) |
| ... decay | common + 17 + 57 (+ 26 at count 0) |
| ... sustain | common + 17 + 17 + 37 (+ 25 if the hold counter is non-zero, + 26 when it reaches 0); no write |
| ... release | common + 17 + 17 + 57 (+ 26 at count 0) |
| ... volume write (attack, decay, release) | + 13 (+ 4 if negative) + 30; VOL(n) |
| voice end | 24 |

Each "PER helper", "LC helper" and "VOL(n)" is the row of table 7.1 for the voice number, ending in the
write. The segment written just before a helper includes the call of the helper.

## 8. Host head and the start of the first tick

The log of a run starts with host and init writes before the first tick (all AUD registers are Paula
registers; times in cc from the start of the run, for subsong 0):

1. The host runs the player's init-player routine; its duration D (cc) depends on the module: classify the
   instruments of the table (count from 1.2) as S (sample offset 0), N (sample offset larger than all
   earlier ones, with 3 as the smallest bound) or O (other); then D = floor((598 + 47 S + 122 N + 62 O) / 2),
   starting at cc 134.
2. Two volume set-ups (the host's set volume and set balance) of 648 cc each; each writes AUDxVOL = 0 for
   voices 0 to 3 at 220, 348, 488 and 623 cc after its start.
3. The init of the subsong: DMACON = 0x000F at 703 cc after its start, then AUDxVOL = 0 for voices 0 to 3 at
   +10, +20, +30, +40 cc after the DMACON write. The call lasts 976 cc.
4. The first tick starts 70 940 cc after the end of that call: tick start 0 = 73 380 + D for subsong 0 (73 918
   with D = 538, as for two modules).
5. For subsong s > 0 the host first runs the sequence once for subsong 0 (items 1 to 3), then for each step up
   to s: a stop call (87 cc long: DMACON = 0x000F at +33 cc, then AUDxVOL = 0 for voices 0 to 3 at 10 cc
   steps), the two volume set-ups and the init of that subsong. Each such step adds 87 + 2 x 648 + 976 =
   2 359 cc, so the first tick of subsong s starts 2 359 x s cc later than the value of item 4.

Init of the player state (item 3): speed 6, step counter 0, running flag 1, the subsong's three position
fields taken from its record, position = first position, row 0, next-row pointer = start of the pattern of the
first position, DMA bits = 0x8000, all four channels' DMA switched off and volumes written 0 as above. The
voice states and the phase counter are not cleared by the init (zero in a fresh run).

## 9. Undefined and edge cases

- Note index above 36 reads beyond the period table: the reference run gave 0x5754 for index 37, 0x0004 for
  index 38 and 0x0000 for index 40 (memory after the player's data; unspecified; avoid).
- Instrument number beyond the table, pattern number beyond the file, a sweep window that leaves the 512-byte
  waveform memory, a sample outside the file, a sample without loop whose loop length is above 2 (the silent
  loop is only 4 bytes): the reference reads whatever lies in memory; unspecified.
- Position list entries and the end/restart offsets are not range-checked; an end position that is never
  reached means the song never ends.
- The first tick uses the initial step counter value 0 and so starts with a new row at row 0 of the first
  position.
