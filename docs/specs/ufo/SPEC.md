# UFO replayer (MicroProse, 1994): behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the music format of the PC/Amiga game "UFO: Enemy Unknown" (AGA version, MicroProse 1994) as
played by the reference (UADE Eagleplayer "UFO": the 68k wrapper plus the game's sequencer routines).
Names: Eagleplayer "UFO", file name prefixes `mus.` / `ufo.` (see 2.3, the wrapper itself only
understands the suffix `.mus`). Everything here is observed behaviour (write logs of the reference,
module data) described in my own words; it contains no instructions or addresses of the original.
Timing numbers are CPU cycles of the 68000 (1 colour clock "cc" = 2 cycles) or cc, as stated.

The player is a small MIDI-like sequencer: up to 16 tracks of timed note/controller events, four
Paula voices, PCM instruments taken from a separate sample bank file or embedded in the song, a linear
volume envelope with a fast release, a pitch-bend control and a tempo accumulator. There are no
subsongs, titles or authors. The module format was only seen through the player; no genuine game
module was available (the archive holds none), so the format below is derived from the player's
parsing code and confirmed by black-box runs of synthetic modules (section 12).

## 1. Files

| file | name | content |
|---|---|---|
| song | `NAME.mus` (or `mus.NAME`) | an IFF-like container "FORM"/"DDAT"/"BODY" holding chunks: CHAN (track data), BLCK (shared event blocks), INST (instrument embedded in the song), BPM, DRUM (ignored) |
| sample bank | `NAME.BANK`, else `SMP.set` in the same directory | an IFF-like container "FORM"/"ADAT"/"BODY": a table of 40 instrument records followed by 8-bit signed PCM data |

All integers are big-endian. The bank is loaded by the wrapper as an "additional file" (2.2).

## 2. Detection, loading, info

### 2.1 Detection

The song file is accepted when, from offset 0: the long `FORM`; one long (the container size, not
checked here); the long `DDAT`; the long `BODY`; one long (body size, not checked); and the long `CHAN` at
offset 20. In other words the first chunk of the body must be a CHAN chunk. There is no other check.

### 2.2 Loading

1. The wrapper loads the additional file named as follows. If the song's file name ends in `.mus`
   (the three letters in either case, preceded by a dot) the bank name is the same name with those three
   letters replaced by the upper-case string `BANK` (so `x.mus` gives `x.BANK`). If the song name does not
   end that way (for example `mus.x`), or the first load fails, the file `SMP.set` in the song's directory is
   loaded instead. If both fail the player does not start ("could not load additional files"). The
   directory is the song's directory.
2. The song must be at least (container size at offset 4) + 8 bytes long, else the player refuses it
   ("module too short"). The song data starts at offset 20 (the first chunk).
3. The bank must start with `FORM`, and its size field (offset 4) + 8 must not exceed the file size; it
   must contain `ADAT` at offset 8 and `BODY` at offset 12, else "error in file". Its first long (the text
   `FORM`) is overwritten with zero by the player: the first four bytes of the bank serve as the silent
   sample (section 8.6). The record table starts at offset 20.
4. The loaded sizes shown by the reference are informative only.

### 2.3 Subsongs, title, length, sample info

- One song per file: the subsong range is 1..1 (no selection).
- No title, author or length is stored. The reference reports the creator text
  "(c) 1994 by MicroProse, adapted by meynaf & Wanted Team".
- The number of "samples" shown is the count of the 40 instrument records whose length is non-zero after
  the song's INST chunks are loaded (section 4.3).
- Song end: signalled to the host at the end-of-song event and at the loop-restart event (section 6.4).
  The reference harness stops playing at the first end signal.

## 3. Sample bank

Offset 20 (after `FORM`, size, `ADAT`, `BODY`, body size): 40 records of 10 bytes
(long length, long loop offset, byte A, byte volume), then 84 (0x54) bytes that are skipped, then the
PCM data of all instruments, back to back, in record order (only records with a non-zero length occupy
space).

For record number i (0..39), the player derives: length = the long; if the length is non-zero then
start = the running data pointer (data offset 20 + 400 + 84 = 504 from the file start for the first
sample), end = start + length, and the pointer advances by the length; loop start = start + loop offset;
the loop flag is set when the loop offset is non-zero (also for records with length 0); volume = the
volume byte (byte A is not used). A record with length 0 has start 0, which means "no sample": a note on
that instrument does nothing audible (8.2). The bank records are loaded when the player starts and
before the song is parsed, so INST chunks in the song replace records.

Sample data are signed 8-bit. The loop region runs from the loop start to the end of the sample.

## 4. Song file

### 4.1 Chunk walk

The body is a sequence of chunks, each beginning with a 4-character id. Parsing starts at offset 20 and
stops at the first chunk whose id is not one of the five below (for example a terminating `END `
chunk, or the end of file; chunks after that are ignored). Chunk layout (offsets from the chunk start):

| id | layout | next chunk starts at |
|---|---|---|
| `CHAN` | +4 long S; +8 byte (unused); +9 byte track number (0..15); +10 first byte of the track stream | chunk start + 8 + S |
| `BLCK` | +4 long S; +8 the block's event stream | chunk start + 4 + S (S counts the size field itself) |
| `INST` | +4 long S; +8 byte (unused); +9 byte instrument number (0..39); +10 long length; +14 long loop offset; +18 byte (unused); +19 byte volume; +20 sample data (signed 8-bit) | chunk start + 4 + S (S counts the size field itself) |
| `BPM ` | +4 byte (unused); +5 byte tempo value B | chunk start + 6 (no size field) |
| `DRUM` | 84 bytes of other data follow the id | chunk start + 88 (no size field; content ignored) |

(The `CHAN` size excludes the size field; `BLCK` and `INST` sizes include it. This follows the
reference's chunk walking; both kinds were exercised by the synthetic modules.)

Effects of each chunk when walked:
- `CHAN`: track `c` (byte at +9) is defined; its stream pointer is +10. The stream begins with a
  variable-length number (4.2), the initial delay in row ticks; the track's pointer is set to the
  byte after it, the "initial pointer" for restarts, and the initial wait counter to that number. A
  later `CHAN` with the same number replaces the earlier one. Tracks not mentioned are inactive.
- `BLCK`: block number n = the number of BLCK chunks seen so far (0 for the first). Its stream pointer
  is chunk start + 8. Blocks are addressed by this order. Up to 80 blocks.
- `INST`: instrument record `i` is set: length = the long at +10, loop offset = long at +14, volume =
  byte at +19, start = chunk start + 20, end = start + length, and the loop flag is set when the loop
  offset is non-zero (loop start = start + loop offset), cleared otherwise. These records replace the
  bank's.
- `BPM `: the tempo value is B. If several BPM chunks occur the last wins. If none occurs B is 0.
- `DRUM`: skipped.

After the walk the stored tempo is computed once: `T0 = floor(B * 24 / 60)` (low 8 bits). The row
counter (6.1) starts at T0.

### 4.2 Variable-length number ("varint")

Bytes are consumed while bit 7 is set: value = (value << 7) + (byte & 0x7F), starting from 0, in 16-bit
arithmetic; the last byte has bit 7 clear. One byte covers 0..127, two bytes 128..16383, three bytes up to
65535 (16-bit overflow beyond). A delay of 0 means "next event in the same row".

### 4.3 Track event stream

Each event is: a command part (below), then a varint delay. If the delay is 0 the next event of the same
track is executed immediately in the same row; otherwise the track sleeps for that many row ticks
(6.2). Command part, first byte `b`:

| b | meaning |
|---|---|
| 0xFF | end of song: signal song end, stop the player (section 6.4) |
| 0xFE | call: next byte = block number n. Remember the position after that byte as the return point (one level only: a call inside a block overwrites it). Continue at block n's stream, which begins with its own varint delay (the delay after the call event itself is read only when the block returns) |
| 0xFD | return: continue at the remembered return point, whose next bytes are the call event's varint delay. No delay follows the 0xFD byte itself |
| >= 0x80 (other) | a status byte; the byte that follows is the first data byte. The status is remembered per track ("running status") |
| < 0x80 | a first data byte; the track's remembered status applies (initially 0, which is a no-op status) |

The upper nibble of the status selects the event; `n` below is the first data byte:

| status | data | effect |
|---|---|---|
| 0x80 | n = note, one more byte (ignored) | note off (8.3) |
| 0x90 | n = note, one byte velocity v | note on (8.1); effective volume `(v * scale) >> 8` where `scale` is the track's volume scale (default 0x7F, set by controller 7). v is not limited to 127. A resulting volume of 0 acts as a note off |
| 0xB0 | n = controller, one byte d | controller 0: tempo, stored tempo = floor(d * 48 / 60) (low 8 bits). Controller 7: volume scale = d. Controller 20 and all others: ignored |
| 0xC0 | n = program | n = 0x7E: song restart (6.4). Otherwise the track's instrument = n (0..39 defined; n >= 40 is outside the tables) |
| 0xE0 | n = bend byte | pitch bend: bend = (n - 0x10) as a signed byte; the voice's period is rewritten at once (8.5) |
| 0xA0, 0xD0, 0xF0..0xFC and the initial status | one data byte consumed | ignored |

Note: the status nibbles 0xA0 and 0xD0 consume only the one data byte (not two as in MIDI), as does every
ignored status. The next byte after the data byte(s) is always the varint delay.

The track state: instrument (initially 0), volume scale (0x7F), bend (0), the base period of the last
note started on that track (initially 0), the saved return point, and the status. None is reset by a
restart.

## 5. Voices, tracks and records

Track `t` (0..15) plays on Paula voice `t mod 4`. Several tracks share a voice; a later note on the
voice takes over.

For each voice j (0..3) the player keeps a record: the track that last started a note (init 0), the note
index of that note (init 0), the two pitch-bend slopes (8.4), the sample's volume (the "release
rate"), the target volume, the current volume and the envelope rate. Initial values: all zero.

## 6. Replay

### 6.1 Timer and structure of one interrupt

The host calls the player at 59 115 cc intervals (about 59.9 Hz, the PAL 50 Hz timer rescaled by 5/6;
see section 9). Each call does, in order: row-step (6.2) twice, then the volume stage (7). The two
row-steps are identical; the second one can run another row.

Row-step: if the player is stopped (after the song end event) nothing happens. Otherwise
`counter -= T` where T is the stored tempo; if `counter > 0` nothing more happens (this step is a no-op).
Otherwise `counter += 120` and one *row* is processed. So one row happens every 120/T row-steps
(2 steps per interrupt): with B = 150 (T = 60) exactly one row per interrupt, with B = 255 (T = 102)
mostly two. The tempo controller changes T at once (not the counter). With T = 0 one row is processed in
the first step and never again.

### 6.2 Row

For each track t in ascending order 0..15:
- if the track is inactive: nothing;
- if its wait counter is 0: run the track (6.3), which returns a new delay D (0 for "stopped"); set the
  wait counter to D; then decrement it (a track that returns D=1 will run again in the next row);
- otherwise decrement the wait counter.
If the restart event occurred the remaining tracks of this row are skipped and the song state is
re-initialised (6.4).

A track's delay D means "run again D rows later". The initial wait from the CHAN stream is not
decremented before the first row (so an initial delay of 0 runs in the first row, 1 in the second).

### 6.3 Running a track

Execute events (4.3) in order until an event's delay is non-zero; that delay is returned. The track's
stream pointer is saved after the delay. Reading the end-of-song event returns 0 without saving.

### 6.4 Song end and restart

- Event 0xFF: signal song end; the player is stopped (no more rows; the other tracks of the row being
  processed still run to completion; from this tick on the volume stage writes volume 0 to all four
  voices, section 7).
- Program change with n = 0x7E: signal song end; then the whole song state is rewound: every track's
  pointer and wait counter go back to their initial values (4.1); the tracks after the current one in
  this row are not processed. The per-track state, the voice records and the counter are not reset.
  Playing continues with the next row-step. The harness stops at the first end signal, so the rewind was
  only observed for a few ticks.

## 7. Volume stage (each interrupt, after the row-steps)

For each voice j in 0..3, using the voice record's three bytes `rate` (signed), `cur` (signed) and
`max` (signed):
1. `cur = (2*cur + rate) >> 1` (arithmetic shift, result stored as a byte, so it wraps).
2. If `rate >= 0`: if the new value (as a signed byte) is greater than `max` then `cur = max` and
   `rate = 0`.
   If `rate < 0`: if the new value is negative then `cur = 0` and `rate = 0`.
3. Write AUDjVOL (register $DFF0A8 + 16 j): if the player is running, the value `cur & 0x7F`
   (the signed byte, low seven bits; values above 64 act as 64 in Paula); if the player is stopped,
   the value 0.

The four writes occur in voice order 0,1,2,3 on every interrupt (even for silent voices).
Initial record state at a note start: `max = cur = ` the note's volume, `rate = 0x63` (99), which clamps
at the first stage run (no audible rise). On note off (8.3): `rate = -(sample volume)` (as a byte) and
`max = 0`, so the volume falls by half the sample volume each interrupt until it passes below 0 (then 0).
A sample volume of 0 gives rate 0 and `max = 0` which clamps `cur` to 0 at once (a hard cut).

With the host's volume at its default and balance centred the hardware value equals the number above. The
host may scale it (volume/balance/muted voice give smaller values or 0).

## 8. Notes

### 8.1 Note on

Inputs: instrument i (the track's current instrument), note number `k`, volume `vol` (> 0), track `t`,
voice `j = t mod 4`.

1. Note index `n = k - 24`; while `n < 0` add 12. (Notes 0..23 therefore fold into the lowest octave of
   the table; notes above 24+83 index beyond the table, 8.4.)
2. Write DMACON = 1 << j (that channel's audio DMA off, no SET bit). Then the *beam wait* (section 9.2).
3. If instrument i has no sample (start 0): the event ends here (the DMA of the voice stays off, the record
   is not touched).
4. Voice record j: track = t, note index = n, `rate = 0x63`, `max = vol`, sample volume = the
   instrument's volume. The slopes for the pitch bend are computed (8.4). The track's base period is set to
   `TABLE[n]`.
5. Write, in this order, the hardware registers of voice j (base $DFF0A0 + 16 j):
   - AUDjLC = the sample start (the long as two word writes, high then low);
   - AUDjPER = the note period with the track's bend applied (8.5);
   - AUDjLEN = (end - start) / 2 words (if end < start add 1 before halving; low 16 bits);
   - AUDjVOL = `vol & 0x7F` (host-scaled);
   - set `cur = vol`.
6. Write DMACON = $8000 | (1 << j) (that channel's audio DMA on), then the beam wait again.
7. Write the "next buffer" for the hardware: if the instrument has a loop: AUDjLEN = (end - loop start)/2
   then AUDjLC = loop start. Otherwise AUDjLEN = 2 and AUDjLC = the silent sample (the first four bytes
   of the bank, all zero).

The register write ORDER of one note on is therefore (voice 0 shown):
`096=bit, 0A0=LChi, 0A2=LClo, 0A6=PER, 0A4=LEN, 0A8=VOL, 096=$8000|bit, 0A4=LEN2, 0A0=LC2hi, 0A2=LC2lo`.

### 8.2 Note on without a sample

Only `DMACON = 1 << j` and the beam wait happen. Nothing else is written or changed.

### 8.3 Note off, and velocity 0

No register is written. Compute `n` as in 8.1. For every voice record whose track equals `t` and whose
note index equals `n`: `rate = -(sample volume)` (signed byte negation), `max = 0`. (The record is looked up
by track and note, not by voice.) The second data byte is skipped.

### 8.4 Period table and pitch-bend slopes

TABLE index `n` (0..83) gives the period (decimal-readable list of hex words):

```
0FBE 0EEE 0DF7 0D09 0C8C 0BDC 0B1A 0A8F 09F4 0969 08D6 0853
07DF 0766 070A 069C 063A 05E4 058D 053F 04F2 04AE 046B 0429
03EF 03B7 0381 034E 0320 02F2 02C8 029F 027B 0257 0235 0216
01F7 01DB 01C0 01A7 0190 0179 0164 0150 013D 012B 011A 010A
00FB 00ED 00E0 00D3 00C7 00BC 00B2 00A8 009E 0095 008D 0086
00FB 00ED 00E0 00D3 00C7 00BC 00B2 00A8 009E 0095 008D 0086
00FB 00ED 00E0 00D3 00C7 00BC 00B2 00A8 009E 0095 008D 0086
```

(The top three octaves repeat the same twelve periods; the original lists them so.) Reads outside the
table happen for notes below index 0 (never, because of the folding) and for note numbers above 107
(index 84 and up), and the slopes of the lowest and highest entries read two entries before and after the
table. The reference reads whatever memory surrounds the table, which is not format data; a data file
holds the observed words for indices -2..103 (2 bytes each, big-endian, first word is index -2). Indices
84..87 read as 0. Sane songs stay within 24..107; the lead decides whether to embed the file or to
treat such notes as period 0.

Slopes at note start (word values, stored for the voice): `up = (TABLE[n] - TABLE[n+2]) >> 4` and
`down = (TABLE[n-2] - TABLE[n]) >> 4` (arithmetic shift of the 32-bit difference, stored as 16-bit signed).
Measured values for the two entries whose `n-2` lies before the table: `down(0) = 400`, `down(1) = 1016`;
for `n = 82, 83` the `n+2` entries are the zero words above.

### 8.5 Pitch bend

The bend of a track is a signed byte (initially 0). The period written for note index `n` on track `t`
and voice `j` is: `P` = TABLE[n] (the track's base period) when bend = 0; for bend > 0
`P - up_j * bend`, for bend < 0 `P + down_j * (-bend)` (32-bit arithmetic with the 16-bit signed slopes
of voice record j, result truncated to 16 bits when written; no clamping).

Bend event (status 0xE0): `bend = n - 0x10` (signed byte), then AUD(t mod 4)PER is written at once with the
formula above using the track's current base period and the *voice's* current slopes (which may belong to
another track's note; with no note ever started the base period is 0 and the slopes are 0, giving 0).
A note on writes its period with the bend of its track in force.

## 9. Host timing in brief

### 9.1 Clock

All timing is in colour clocks (1 cc = 2 CPU cycles, 3 546 895 cc per second). The reference counts
fixed 68000 cycles per instruction and the clock of a write is the clock at the start of the call plus
floor(cycles/2) after charging the writing instruction. The beam position at clock c is line = floor(c/227)
(227 cc per line, frame wrap on a multiple of 227), h = c mod 227.

Interrupts occur on a grid of 59 115 cc. An interrupt that arrives while the previous call is still
running is held and taken immediately when that call returns; the grid is not shifted. (Heavy ticks
with many note-ons take longer than one period: 16 simultaneous note starts take about 99 000 cc.)

### 9.2 Beam wait

Each DMACON write of a note start is followed by a wait for the beam: the code reads the beam line byte
repeatedly and waits for 9 line changes. Reads happen (in cc, relative to the DMACON write):
the first read 31 cc after the write; the second 8 cc later; then every 12.5 cc (alternating 12/13) until a
read shows a different line than the first read of that iteration ("exit read"); then the next iteration's
first read comes 17.5 cc after the exit read (17/18 alternating), its second read 8 cc later, etc. After
the 9th exit read the code continues. The first wait of a run was observed 12 cc shorter than this rule.

The line byte changes at clock multiples of 227, so a wait ends between 8 and 9 lines after it began; the
next write follows the 9th exit read by a fixed delay (11.2).

## 10. Register write timing (colour clocks)

Measured on the reference run; all figures are cc. A 32-bit register write (the sample pointers) is
logged as two word writes (high word first) at the same clock.

Idle interrupt (no row processed, no note events): the four volume writes of the volume stage come in
voice order with fixed spacings of 266 cc (voice 0 to 1), 286 cc (1 to 2) and 314 cc (2 to 3). The
first of them follows the interrupt entry by roughly 505 cc plus the cost of the row-steps (a row-step
that processes no row is cheap; a processed row costs about 250 cc for the walk over the 16 tracks plus
the cost of every executed event, below).

Typical note start on an otherwise idle voice (clock relative to the DMA-off write at t = 0):

| write | t (cc) |
|---|---|
| DMA off (DMACON, 1<<j) | 0 |
| sample pointer (2 words, same clock) | about 2265 (the beam wait, 9.2, plus fixed overhead) |
| period | +242 |
| length | +79 |
| volume | +164 |
| DMA on (DMACON, $8000 + bit) | +101 |
| length (next buffer) | about +1945 (second beam wait) |
| loop/silent pointer (2 words, same clock) | +12 |

The two beam waits dominate: a note start costs about 4200 cc, and 16 notes in one row about 99 000 cc
(more than one interrupt period, so the next interrupt is taken immediately after; the volume stage of
that interrupt is delayed accordingly).

Other event costs (approximate, in 68000 cycles, from fitting): a plain event dispatch 170 to 250;
note off about 500 plus about 16 per voice record belonging to the track (22 more on a note match);
controller 500 to 560; program change about 500; call about 445; return about 366; the volume stage
about 530 per voice. These are fitted constants, not exact.

## 11. Cycle-level accuracy and what is not exact

The value sequence (addresses and values) of every write is exactly reproduced by the behaviour above on
all validation modules. The clocks are reproduced by a cycle model only approximately: after fitting the
constants of section 10, a little over half of all writes land on the reference clock exactly, most of the
rest within 1 to 3 cc (cycle parity of the clock = entry + floor(cycles/2)), and the remainder within one
wait-loop iteration (about 12 cc) because the beam wait quantises: a small shift of the wait start moves the
exit read by one iteration. A cycle-exact port needs the original instruction timings, which this spec
deliberately does not describe; the implementer is advised to reproduce values and write order exactly and
the wait rule of 9.2 and the 59 115 cc grid for timing, and to accept small clock differences.

## 12. Validation

A private model of this behaviour (value level) reproduced the reference write sequence exactly (address
and value of every write, in order) on 221 of 222 synthetic modules (about 145 000 writes), covering:
all event kinds, running status, calls and returns, tempo values 0..255 and tempo changes, volume
scale, program changes, bend up/down on all voices, tracks 0..15 sharing voices, looped and unlooped
instruments, zero-length and zero-volume instruments, embedded INST chunks, repeated CHAN/BPM chunks,
overlapping notes on one voice, notes 0..125, end of song and restart. The remaining module has tracks
that run off the end of the data, behaviour that is outside the format. No real UFO module was available:
all modules are synthetic (generated, both hand-built and random). The tick-level clock model is as
described in section 11.
