# Anders 0land replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Amiga music format of Anders "Zonix" 0land (1993; used in the Psygnosis game Prime Mover), file
name prefix `hot.`, as played by the reference (the UADE player "Anders_0land", Eagleplayer name "Anders 0land",
"DeliTracker compatible" variant). It is a description of observed behaviour (write logs, module data) in my own
words; it contains no instructions or addresses of the original. "cc" means Amiga colour clocks (1 cc = 2 CPU
cycles); costs are given in 68000 CPU cycles.

No real module of this format was available (the module corpus holds none; see README.md). Everything below was
checked on 14 synthetic modules built from this description (14 runs, 10 506 ticks, 98 502 register writes). A value
and timing model written from this text alone reproduces the register, value and order of every write, and the
colour clock of every write, exactly in all of them (details in section 10 and LOGS.md).

## 1. File format

A module is three chunks one after the other. Each chunk starts with a 4-byte tag and a 32-bit big-endian size that
counts the 8 header bytes and the body; the next chunk begins `size` bytes after the start of this one.

| chunk | tag | content |
|---|---|---|
| 1 | `mpl1` | the game's own driver code plus a text (the replay below does not use the code, see 1.3) |
| 2 | `mdt1` | song data (section 1.2) |
| 3 | `msm1` | sample data: raw signed 8-bit samples |

### 1.1 Detection

The reference accepts a file when, starting at offset 0 with `remaining` = file length, for each of the three chunks in
turn: the size is read from bytes 4..7 of the chunk, `remaining` is reduced by it and must not become negative; the
first three bytes of the tag must be `mpl`, `mdt`, `msm` respectively (the fourth tag byte is not looked at; the known
tag is a digit one); for the first two chunks the size must be even. The next chunk starts at the current chunk start plus its size.
Nothing else is checked (the file name is not looked at by this test; the prefix `hot.` is only the player's registered
name prefix). A file shorter than the sum of the three sizes is rejected (module too short).

### 1.2 Song data (body of chunk 2)

All offsets below are relative to the start of the chunk-2 body (call it SONG), all words and longs are big-endian.
The body starts with 13 words:

| word at | name | meaning |
|---|---|---|
| 0 | voice table | offset of 4 longs; long n is the offset (relative to the voice table itself) of voice n's order list |
| 2 | restart table | 4 bytes per subsong: for voice 0..3 the order index to which the voice's order list jumps back when it reaches its end marker |
| 4 | subsong table | 4 bytes per subsong: byte 0 = active-voice mask (bit n set = voice n is played), byte 1 = unused, byte 2 = song mask (voices that get hardware output; bit n = voice n), byte 3 = initial speed (ticks per row) |
| 6 | end | offset of the end of the order data (used only for the length shown by the host, see 2) |
| 8 | - | unused |
| 10 | pattern table | longs, relative to the pattern table itself: offset of pattern p |
| 12 | instrument table | 8 bytes per instrument (1.2.2) |
| 14 | envelope table | 4 bytes per envelope entry (1.2.3) |
| 16 | command table | 16-bit words (1.2.4) |
| 18 | sample address table | longs: offset of sample s inside the chunk-3 body |
| 20 | sample length table | words: length of sample s in 16-bit words (= the AUDxLEN value) |
| 22 | loop address table | longs: offset of the loop start of sample s inside the chunk-3 body |
| 24 | loop length table | words: loop length in 16-bit words |

The host derives the subsong count from the distance between the restart-table offset and the subsong-table offset, and
the sample count from the distance between the sample-address-table offset and the sample-length-table offset
(section 2); so the restart table must fill exactly the space up to the subsong table, and the sample address table exactly
the space up to the sample length table. The loop tables have the same number of entries as the sample tables.

Index limits. The reference does its table index arithmetic in 8-bit quantities, so the following numbers must stay in
range or the reference reads other data (not specified here): pattern number < 64 (70 also worked in tests, 200 did not),
instrument number < 32, sample number < 64, envelope index < 64, note index (note + transpose, 8-bit) <= 69. Command
numbers use their low 7 bits. Tests with instrument numbers up to 39, envelope indices up to 69 and sample numbers up to 69
did give different results from the model below.

#### 1.2.1 Order lists and patterns

An order list is a byte string, one per voice:
- a byte < 0x80 is a pattern number;
- a byte >= 0x80 and not 0xFE/0xFF: low 7 bits are a transpose value (stored for the voice), and the next byte is the
  pattern number;
- 0xFF: end marker, the voice restarts at the order index given by the restart table;
- 0xFE: stop marker, the voice is switched off.

A pattern is a byte string of rows, terminated by 0xFF. A row starts with a flag byte F; its low 5 bits (F & 0x1F) are
the duration: the row holds for that many further rows after the row it starts in (duration 0 = one row).

| F | row length and content |
|---|---|
| bit 7 set (instrument row) | F, X, [N]. Instrument := X & 0x7F. If X bit 7 is clear a note byte N follows (3 bytes), else no note (2 bytes) |
| bit 7 clear, bit 6 set (command row) | F, X, [N]. Command := command table word number (X & 0x7F). X bit 7 / N as above |
| bits 7,6 clear, bit 5 set | F only (1 byte, no note) |
| bits 7,6,5 clear | F, N (2 bytes, with note) |

Bit 5 of F in an instrument or command row has the same extra meaning as in the other rows (alternate instrument, 4.4).
The note byte N: bit 7 = tie (no restart of the sample, 4.4), low 7 bits + the voice's transpose, wrapped to 8 bits, =
index into the period table (section 7).

#### 1.2.2 Instruments (8 bytes)

| byte | meaning |
|---|---|
| 0 | primary envelope index (bit 7 ignored) |
| 1 | alternate envelope index (bit 7 ignored) |
| 2 | primary sample number |
| 3 | alternate sample number |
| 4 | vibrato delay in ticks (0xFF = vibrato off) |
| 5 | vibrato step (signed byte) |
| 6 | vibrato half period in ticks |
| 7 | not used by the replay |

#### 1.2.3 Envelope entries (4 bytes)

volume byte (0xFF = leave the volume as it is), step (added to the volume each tick, 8-bit wrap), count (number of ticks
the step is applied), next entry index.

#### 1.2.4 Command words

A command word is looked at in this order: bit 14 set = start a fade (the low byte is not used); else bit 13 set =
speed := low byte; else bit 12 clear = pitch slide of minus (word & 0xFFF) per tick; else (bit 12 set) pitch slide of plus
(word & 0xFFF) per tick.

### 1.3 The driver in chunk 1

Chunk 1 contains the game's version of the driver and a text. The reference (the Eagleplayer) does not run that code; it
has its own copy of the replay routine and reads only the tables of chunks 2 and 3. All behaviour below is that of the
reference. (The text is shown by the host as "special info"; see 2.)

## 2. Information shown by the host

- Subsong count = (word at 4 - word at 2) / 4 of the song data. Subsongs are numbered from 0.
- Sample count = (word at 20 - word at 18) / 4.
- Length shown = (word at 6 - word at 0) - (the long at offset 12 of the voice table), i.e. the number of bytes of the
  fourth voice's order data up to the end offset (display only; there is no time length).
- Sizes: the calculated total size is the sum of the three chunk sizes; the file must be at least that long.
- Title: none (the host shows the file name). Author: the player name. Special info: the bytes of chunk 1 that follow its
  first 18 bytes, up to the target of the branch at chunk 1 offset 8 (a 0x6000 word followed by a 16-bit
  displacement counted from offset 10, or a short branch whose byte displacement is the low byte of the word at offset 8;
  target = offset 10 + displacement); the byte at the target is replaced by 0. In the text a ':' is followed by a new
  line and a '-' followed by a space is replaced by a new line. Display only.

## 3. Host interface and frame

The reference calls the replay once per frame from a 50 Hz timer interrupt: exactly 70940 cc between calls. A call
writes nothing but the Paula registers of the four audio channels (DMACON, AUDxLC/LEN/PER/VOL; the only other write in a log is the single CIA line at the start of a subsong, see below).

Start of a subsong (host side, in this order, each pair separated by the given cc; T = the CIA write):
- a CIA port write at T (value 0x02; it is the read-back of a bit set in CIA-A port A, in the log as `bfe001 0002`);
- DMACON = 0x000F at T+10;
- AUD0VOL, AUD1VOL, AUD2VOL, AUD3VOL = 0 at T+20, T+30, T+40, T+50.
The module's initialisation (4.1) has no register writes of its own. The first call of the tick routine starts
74267 cc after the AUD3VOL write of the start (that is, call number k, k = 0, 1, 2 ..., starts at
C(k) = (clock of the AUD3VOL start write) + 74267 + 70940 k).

If the subsong number is not 0, the host first runs the start (and an end-of-sound, which writes DMACON 0x000F and the four volumes 0)
for each lower subsong; see LOGS.md for the exact host sequence. The audio DMA is "off" before the first tick of every subsong.
Before the first start the host also writes the four volume registers = 0 eight times (its volume/balance set-up); times in LOGS.md.

Host volume and balance. Voices 0 and 3 use the left volume, voices 1 and 2 the right volume (each 0..64, default 64).
The volume written is (v & 0x7F) * scale / 64 (integer, truncated). With the defaults the written value is v & 0x7F.
A muted voice writes 0 (host feature, not exercised in the logs).

## 4. State and the replay

Notation: `voice v` is 0..3. All counters are 8-bit unless stated; "16-bit" values wrap at 65536. S = song data.

### 4.1 Start of a subsong n

All working state is zeroed, then:
- speed := byte 3 of subsong table entry n; active mask := byte 0; song mask := byte 2;
- restart index of voice v := byte v of restart table entry n;
- envelope enable mask := 15 (all voices);
- for each voice: order list pointer := voice table entry; order index := 0; "current pattern" := an empty pattern
  (a lone 0xFF row), pattern position := 0;
- the DMA-on word := 0x8000 | song mask;
- tick counter t := 0; fade counter := 0.
Everything else (periods, volumes, envelope state, vibrato, slides, transposes, flags) is zero.

### 4.2 Per-voice state

active (bit of the active mask), order index, pattern position, current pattern, instrument number, wave (sample number),
transpose, duration counter, target period (16-bit), current period (16-bit), slide (16-bit, set by a command),
applied slide (16-bit), vibrato: delay counter, step, half period, counter, accumulator (16-bit), output byte;
envelope: index, step, count, volume (8-bit); flags: new-note flag (this row started an event), tie flag, alternate-instrument
flag, previous-row-note flag; sample snapshot (wave number copied at phase 0). Global: speed, tick counter t,
fade flag, fade counter (16-bit), note-flag (used while decoding a row), song-end flags (4).

### 4.3 Tick phases

Let t be the tick counter at the start of the call. The tick is made of the following steps, in this order.
Step costs are in section 9 (item names in `code` style).

1. Entry. Cost `entry` + `play_prologue`.
2. Phase select, by t, with t read as a signed byte only for the "later" test:
   - t = 1: phase 1; cost `disp_phase1`; the voices stepped are 2 and 3.
   - t = 2..127: no voice stepping; cost `disp_late`.
   - t = 0 or t >= 128: phase 0; cost `disp_phase0`; first the previous-row-note flags := the new-note flags, the sample
     snapshot := the current wave numbers, then the new-note flags and the tie flags are cleared; the voices stepped are
     0 and 1.
3. Voice steps (4.4), for the voices of the pair, lower first.
4. DMA step (4.5).
5. Tick counter: t := t + 1 (8-bit); cost `rc`. If t equals speed, the row ends: t := 0 and the row-end step (4.6) runs;
   otherwise it is skipped.
6. Envelope step (4.7), cost `en_head` + per-voice items.
7. Output step (4.8).

### 4.4 Voice step (voice v)

Cost `vs_head`. If voice v is not active: cost `vs_tail`, done.
Cost `vs_busy`. If the duration counter is not zero: decrement it, cost `vs_wait` + `vs_tail`, done.
Otherwise a new event starts for the voice:
- slide := 0; the note-present flag is cleared; the new-note flag is set; the tie flag is cleared. Cost `ev_start`
  (this covers reading the first row byte F from the current pattern at the pattern position).
- If F = 0xFF (end of pattern; this is also the state right after start-up): cost `ev_patend`; read the order byte B at the
  order index.
  - B = 0xFF: order index := restart index; B := the order byte there; song-end bookkeeping for voice v (4.9); cost
    `ev_restart` + the bookkeeping cost + `ev_restart_end`.
  - otherwise cost `ev_stopchk`; if B = 0xFE: the voice is stopped: active := 0, envelope index := 0, new-note flag set,
    tie flag cleared; song-end bookkeeping for v; cost `ev_stop` + bookkeeping + `ev_stop_end`; then cost `vs_tail`;
    the voice step is finished (no row is decoded).
  - Cost `ev_transchk`. If B >= 0x80: transpose := B & 0x7F, B := the next order byte, order index += 1; cost `ev_trans`.
  - Order index += 1; pattern position := 0; the current pattern is pattern B (pattern table); F := its first byte; cost `ev_newpat`.
- Row decode (the position is P, the position before this row):
  - cost `dc_flag7`. F bit 7 set: X := byte P+1; instrument := X & 0x7F; cost `dc_instr`; then the length rule below.
  - else cost `dc_flag6`. F bit 6 set: X := byte P+1; C := command word number (X & 0x7F); cost `dc_cmd`; then:
    C bit 14: fade flag := 1, cost `dc_cmd_fade`; else cost `dc_cmd_t`; C bit 13: speed := C & 0xFF, cost `dc_cmd_tempo`;
    else cost `dc_cmd_s`; C bit 12 clear: slide := -(C & 0xFFF), cost `dc_cmd_down`; C bit 12 set: slide := C & 0xFFF,
    cost `dc_cmd_up`. Then the length rule.
  - else cost `dc_flag5`. F bit 5 set: note-present := 0, position += 1, cost `dc_short`. Else: note-present := 1, position += 2,
    cost `dc_note2`. (No length rule.)
  - Length rule: cost `dc_len_chk`; if X bit 7 is clear: position += 3, note-present := 1, cost `dc_len_note`; else
    position += 2, note-present := 0, cost `dc_len_nonote`.
- Instrument load, cost `in_chk`:
  - F bit 5 set (alternate): cost `in_alt_chk`; if the voice's alternate flag is clear: set it; envelope index :=
    byte 1 of the instrument & 0x7F; wave := byte 3; cost `in_alt`. If the flag is already set nothing changes.
  - F bit 5 clear: alternate flag := 0; envelope index := byte 0 & 0x7F; wave := byte 2; cost `in_pri`.
- Finish: duration counter := F & 0x1F; cost `fin`. If note-present: cost `fin_c0`; the note byte is at P+1, or at P+2
  if F has bit 7 or bit 6 set (in that case cost `fin_c0_inc` too); N := that byte; cost `fin_note`; if N bit 7: tie flag :=
  1, cost `fin_tie`; index := ((N & 0x7F) + transpose) & 0xFF; target period := period table[index]; cost `fin_period`.
- Cost `vs_tail`.

The instrument number used is the one set by the most recent instrument row of the voice (initially 0).

### 4.5 DMA step

Cost `dm_head`. Let last := ((speed - 1) & 0xFF) equals t (t before the increment of step 5).
- last: write DMACON := (new-note flags XOR (tie flags & 15)), a "clear" write (bit 15 clear), i.e. it switches
  off the voices that start a new sample this row. Cost `dm_off` (write completes `w_dm_off` cycles into it).
- otherwise: write DMACON := 0x8000 | song mask. Cost `dm_on` (write completes `w_dm_on` cycles into it). Then, if
  t = 1: cost `dm_setup`; for voice v = 0..3: cost `dm_v_c1`; if v is in the song mask: cost `dm_v_c2`; if also the
  previous-row-note flag of v is set: write AUDxLEN := loop length table[sample snapshot of v] then AUDxLC (hi, lo, same clock) := chunk-3 start + loop
  address table[snapshot]; cost `dm_v_wr` (length completes at `w_dm_v_len`, address at `w_dm_v_adr`); then cost `dm_v_next`.
  This installs the loop part after the DMA has taken the start address.

### 4.6 Row end

Cost `re_start`. For voice v = 0..3, in order:
- cost `re_c_mask`; if v is not in the song mask: cost `re_next`, next voice.
- cost `re_c_s7`; if the tie flag of v is set: go to the copy below (no pointer writes).
- else cost `re_c_s8`; if the new-note flag is clear: cost `re_next`, next voice.
- else cost `re_c_s6`; if the alternate flag of v is clear: vibrato initialisation: output byte := 0, delay := instrument
  byte 4, step := byte 5, half period := byte 6, counter := byte 6 / 2, accumulator := 0; cost `re_vib`.
- Pointers. With i := wave of v: AUDxLEN := sample length table[i]; cost `re_ptr1` plus `lenstore_vN` (N = voice number);
  the write completes `w_re_len` cycles into `re_ptr1`. AUDxLC := chunk-3 start + sample address table[i]; cost `re_ptr2`
  plus `adrstore_vN`; the write (hi and lo with the same clock) completes `w_re_adr` cycles into `re_ptr2`.
  (The reference computes the table offsets as 2 i and 4 i in 8-bit arithmetic, which is plain indexing for i < 64.)
- Envelope start: envelope count := 0; E := envelope entry [envelope index]; cost `re_env`; if E's volume byte is not 0xFF:
  volume := it, cost `re_env_set`.
- Copy (also for tied voices): current period := target period; applied slide := slide; cost `re_copy`.
- cost `re_next`.

### 4.7 Envelope step

Cost `en_head`. For voice v = 0..3:
- cost `en_c1`; if v is not active: cost `en_next`, next voice. cost `en_c2`; if v is not in the song mask: `en_next`.
  cost `en_c3`; if the new-note flag of v is set: `en_next`.
- Vibrato: cost `vb_chk`; if the vibrato delay is 0xFF: nothing. Else cost `vb_dchk`; if the delay is not 0: delay -= 1, cost `vb_dec`.
  If the delay is 0: accumulator += (step as a signed byte) * 16 (16-bit); output byte := high byte of the accumulator;
  counter -= 1; cost `vb_run`; if counter = 0: step := -step (8-bit), counter := half period; cost `vb_flip`.
- Envelope: cost `en_chk` (the enable mask is always set). Cost `en_cchk`; if the envelope count is not 0: volume := volume + step
  (8-bit), count -= 1, cost `en_step`. Else: entry E := envelope [envelope index]; cost `en_fetch`; if E.volume != 0xFF:
  volume := E.volume, cost `en_fetch_set`; step := E.step, count := E.count, envelope index := E.next; cost `en_fetch_rest`.
- Fade: cost `fd_chk`; if the fade flag is set: q := volume / 64 (integer; the volume is an 8-bit value),
  volume := (volume - q * fade counter) & 0xFF, fade counter += 1 (16-bit); cost `fd_run`; if the fade counter now equals 0x40: fade flag
  := 0, cost `fd_end`. The fade counter is not reset by a new fade command: after one fade it stays at 0x40, so the next
  fade runs for 65536 further steps before it can end again; and the fade acts on a voice only for voices that reach this step.
- cost `en_next`.

### 4.8 Output step

Cost `fl_head`. For voice v = 0..3: cost `fl_chk`; if v is not in the song mask: cost `fl_next`, next voice. Else:
current period := current period + applied slide (16-bit); P := current period + (output byte as a signed byte) (16-bit).
Write AUDxPER := P (completes `w_fl_period` cycles into `fl_period`). Then the volume: V := (volume & 0x7F) * scale / 64, written to
AUDxVOL by the volume routine, which starts at the end of `fl_period`; its total cost is `volstore_vN` and the write completes
`volstore_vN_write` cycles after its start. Then cost `fl_next`.
All four voices of the song mask get a period and a volume write every tick, also when stopped or silent.

### 4.9 Song-end bookkeeping

Called at an order-list restart or stop of voice v. Clear voice v's song-end flag (all four set at start). Cost
`songend_vN` (N = v). If now all four flags are clear: cost `songend_raise`, the host is notified that the song ended,
which advances time as follows: the cycle count so far in this call is converted to colour clocks (rounded down), 8 cc
are added, and counting continues from that (so the cycle count is even afterwards); then all four flags are set again.
A voice that never restarts or stops (for example one that is not in the active mask) keeps the notification from
ever happening. The host in the logs stops the run a few ticks (2..5) after the notification.

### 4.10 Write time

Within a call, cycles are counted from the start of the call (the first item of step 1). The time of a register write is
C(k) + floor(cycles / 2), where `cycles` is the count at the end of the writing instruction; the tables give the write
position inside an item as an offset from the start of the item, already including the write's own cycles. A long
write (a pointer) gives two lines (high word at the register, low word at register + 2) with the same clock.

## 5. Register write order within a tick

DMACON; then (non-last ticks with t = 1) for each voice in the loop-pointer pass AUDxLEN, AUDxLC hi, AUDxLC lo; or, at the
row end, for each voice with a fresh sample: AUDxLEN, AUDxLC hi, AUDxLC lo (voices in order 0..3); then for each voice of the song
mask AUDxPER, AUDxVOL. Writes of one kind in one tick are never interleaved with another voice's pass.

## 6. Values written

AUDxLEN: words, from the tables as above. AUDxLC: address of the sample data in memory; in the logs the address of the
module is a host value (see LOGS.md). AUDxPER: the 16-bit value computed in 4.8 (it may exceed the Paula range; it is written as is).
AUDxVOL: 0..127 as computed (not clamped; Paula clamps).

## 7. Period table (index 0..69)

0xD60, 0xCA0, 0xBE8, 0xB40, 0xA98, 0xA00, 0x970, 0x8E8, 0x868, 0x7F0, 0x780, 0x714, 0x6B0, 0x650, 0x5F4, 0x5A0,
0x54C, 0x500, 0x4B8, 0x474, 0x434, 0x3F8, 0x3C0, 0x38A, 0x358, 0x328, 0x2FA, 0x2D0, 0x2A6, 0x280, 0x25C, 0x23A,
0x21A, 0x1FC, 0x1E0, 0x1C5, 0x1AC, 0x194, 0x17D, 0x168, 0x153, 0x140, 0x12E, 0x11D, 0x10D, 0x0FE, 0x0F0, 0x0E2,
0x0D6, 0x0CA, 0x0BE, 0x0B4, 0x0AA, 0x0A0, 0x097, 0x08F, 0x087, 0x07F, 0x078, 0x072, 0x06B, 0x065, 0x05F, 0x055,
0x050, 0x050, 0x050, 0x050, 0x050 (indices 0..68), index 69 = 0x8000 (a marker; it is written as a period like any other
value). Indices above 69 read other memory of the reference and are not specified.

## 8. Edge behaviour confirmed on the reference

- speed 1: only phase 0 runs, voices 2 and 3 are never stepped (their events never start).
- speed 0: 256 ticks per row; the tick counter wraps at 256.
- t >= 128 behaves as phase 0 (so voices 0 and 1 are stepped again on every tick from t = 128 up, after a speed
  change made the counter overrun the speed).
- A stopped voice (order marker 0xFE) keeps being output: its period and volume continue to be written, and its last
  sample keeps playing/looping.
- A tie note (note bit 7) neither restarts DMA nor rewrites pointers/envelope, but takes the new target period and slide.

## 9. Cost items

Costs in 68000 CPU cycles; they include the control-flow tests and the call/return of the subroutines, and are static
(the same on every execution). `N` in `_vN` is the voice number 0..3.

#### Entry and phase select

| item | cycles | covers |
|---|---|---|
| `entry` | 242 | interrupt wrapper: register save, flag set-up, call into the replay routine |
| `play_prologue` | 76 | replay routine start: base pointers, eight working values cleared, tick-counter test |
| `disp_phase0` | 130 | phase 0: both tests, then the flag copy/clear before the voice pair 0,1 |
| `disp_phase1` | 41 | phase 1: tick-counter tests, then the voice-pair set-up for voices 2,3 |
| `disp_late` | 20 | phase 2 and later: both tests, straight on to the DMA part |

#### Voice step

| item | cycles | covers |
|---|---|---|
| `vs_head` | 23 | voice step: "is this voice active" test |
| `vs_busy` | 25 | active voice: duration counter test |
| `vs_wait` | 18 | duration counter still running: decrement |
| `vs_tail` | 43 | after every processed voice: advance to the next voice of the pair |

#### Event start, order list and pattern change

| item | cycles | covers |
|---|---|---|
| `ev_start` | 143 | event start: clear slide, flags, fetch the first row byte, end-of-pattern test |
| `ev_patend` | 67 | end of pattern: fetch the order-list byte, restart test |
| `ev_restart` | 60 | order list restart: load restart position, fetch (up to and including the song-end call) |
| `ev_restart_end` | 10 | after the song-end call of the restart |
| `ev_stopchk` | 17 | not a restart: stop-marker test |
| `ev_stop` | 100 | stop marker: voice switched off (up to and including the song-end call) |
| `ev_stop_end` | 10 | after the song-end call of the stop |
| `ev_transchk` | 19 | pattern number: transpose flag test |
| `ev_trans` | 86 | transpose entry: store transpose, fetch the next order byte |
| `ev_newpat` | 176 | new pattern: advance position, locate pattern, fetch its first row byte |

#### Row decode

| item | cycles | covers |
|---|---|---|
| `dc_flag7` | 19 | row byte: instrument-row test |
| `dc_instr` | 50 | instrument row: store the instrument number |
| `dc_flag6` | 19 | row byte: command-row test |
| `dc_cmd` | 95 | command row: fetch the command word, fade-bit test |
| `dc_cmd_fade` | 42 | command with the fade bit |
| `dc_cmd_t` | 19 | command: tempo-bit test |
| `dc_cmd_tempo` | 22 | command with the tempo bit |
| `dc_cmd_s` | 19 | command: slide-direction test |
| `dc_cmd_down` | 36 | slide down: negate and store |
| `dc_cmd_up` | 42 | slide up: mask and store |
| `dc_flag5` | 19 | row byte: bit 5 test |
| `dc_short` | 48 | one-byte row (bit 5): flags, advance by 1 |
| `dc_note2` | 48 | two-byte row: note flag, advance by 2 |
| `dc_len_chk` | 19 | second byte: bit 7 test |
| `dc_len_note` | 48 | advance by 3, note flag set |
| `dc_len_nonote` | 38 | advance by 2, note flag clear |
| `in_chk` | 19 | row byte bit 5 (alternate instrument) test |
| `in_alt_chk` | 21 | alternate instrument: already-active test |
| `in_alt` | 164 | alternate instrument load |
| `in_pri` | 182 | primary instrument load |
| `fin` | 51 | duration store, note-flag test |
| `fin_c0` | 17 | note: row-kind test |
| `fin_c0_inc` | 4 | note offset one further |
| `fin_note` | 41 | fetch note byte, tie-flag test |
| `fin_tie` | 16 | tie flag set |
| `fin_period` | 60 | transpose, period table lookup, store target period |

#### DMA step

| item | cycles | covers |
|---|---|---|
| `dm_head` | 37 | tempo compare: last tick of the row? |
| `dm_off` | 62 | last tick: DMA-off write |
| `dm_on` | 49 | other ticks: DMA-on write, tick-counter test |
| `dm_setup` | 60 | tick 1: loop-pointer pass set-up |
| `dm_v_c1` | 21 | loop-pointer pass: voice-in-song test |
| `dm_v_c2` | 21 | loop-pointer pass: previous-row-note test |
| `dm_v_wr` | 108 | loop-pointer pass: length and address writes |
| `dm_v_next` | 29 | loop-pointer pass: next voice |
| `rc` | 51 | tick counter increment and tempo compare |

#### Row end

| item | cycles | covers |
|---|---|---|
| `re_start` | 60 | row end: clear counter, pointers |
| `re_c_mask` | 23 | row end, per voice: in-song test |
| `re_c_s7` | 23 | tie-flag test |
| `re_c_s8` | 23 | new-event flag test |
| `re_c_s6` | 21 | alternate-mode test |
| `re_vib` | 184 | vibrato initialisation |
| `re_ptr1` | 156 | sample tables, length write, call to the length store |
| `re_ptr2` | 76 | address write, call to the address store |
| `re_env` | 105 | envelope start: fetch first byte, test |
| `re_env_set` | 14 | envelope first volume stored |
| `re_copy` | 48 | copy target period and slide |
| `re_next` | 35 | next voice |

#### Envelope step

| item | cycles | covers |
|---|---|---|
| `en_head` | 44 | envelope pass set-up |
| `en_c1` | 23 | envelope pass, per voice: active test |
| `en_c2` | 23 | in-song test |
| `en_c3` | 23 | new-event flag test |
| `vb_chk` | 44 | vibrato delay: off-value test |
| `vb_dchk` | 23 | delay counter zero test |
| `vb_dec` | 28 | delay counter decrement |
| `vb_run` | 105 | vibrato step |
| `vb_flip` | 42 | vibrato direction reversal |
| `en_chk` | 21 | envelope enable test |
| `en_cchk` | 23 | envelope count test |
| `en_step` | 64 | envelope step |
| `en_fetch` | 55 | envelope fetch: first byte test |
| `en_fetch_set` | 24 | volume byte stored |
| `en_fetch_rest` | 72 | step, count, next index stored |
| `fd_chk` | 25 | fade flag test |
| `fd_run` | 275 | fade step |
| `fd_end` | 20 | fade finished |
| `en_next` | 35 | next voice |

#### Output step

| item | cycles | covers |
|---|---|---|
| `fl_head` | 16 | output pass set-up |
| `fl_chk` | 21 | in-song test |
| `fl_period` | 110 | period computation and write, call to the volume store |
| `fl_next` | 37 | next voice |

#### Song-end bookkeeping

| item | cycles | covers |
|---|---|---|
| `songend_v0` | 132 | song-end bookkeeping call for voice 0 without notification, including call and return |
| `songend_v1` | 153 | song-end bookkeeping call for voice 1 without notification, including call and return |
| `songend_v2` | 170 | song-end bookkeeping call for voice 2 without notification, including call and return |
| `songend_v3` | 177 | song-end bookkeeping call for voice 3 without notification, including call and return |
| `songend_raise` | 68 | extra cost when the bookkeeping raises the notification (before the host time step of 4.9) |

#### Pointer bookkeeping routines

| item | cycles | covers |
|---|---|---|
| `lenstore_v0` | 79 | bookkeeping routine after the length write, voice 0 (cost only) |
| `lenstore_v1` | 104 | bookkeeping routine after the length write, voice 1 (cost only) |
| `lenstore_v2` | 129 | bookkeeping routine after the length write, voice 2 (cost only) |
| `lenstore_v3` | 137 | bookkeeping routine after the length write, voice 3 (cost only) |
| `adrstore_v0` | 95 | bookkeeping routine after the address write, voice 0 (cost only) |
| `adrstore_v1` | 120 | bookkeeping routine after the address write, voice 1 (cost only) |
| `adrstore_v2` | 145 | bookkeeping routine after the address write, voice 2 (cost only) |
| `adrstore_v3` | 153 | bookkeeping routine after the address write, voice 3 (cost only) |

#### Volume output routine

| item | cycles | covers |
|---|---|---|
| `volstore_v0` | 226 | volume output routine for voice 0: total cost |
| `volstore_v0_write` | 176 | volume output routine for voice 0: the volume register write completes this many cycles after its start |
| `volstore_v1` | 243 | volume output routine for voice 1: total cost |
| `volstore_v1_write` | 193 | volume output routine for voice 1: the volume register write completes this many cycles after its start |
| `volstore_v2` | 260 | volume output routine for voice 2: total cost |
| `volstore_v2_write` | 210 | volume output routine for voice 2: the volume register write completes this many cycles after its start |
| `volstore_v3` | 267 | volume output routine for voice 3: total cost |
| `volstore_v3_write` | 227 | volume output routine for voice 3: the volume register write completes this many cycles after its start |


### 9.1 Write offsets

Offset of the end of the writing instruction from the start of the item (cycles).

| item | cycles | write |
|---|---|---|
| `w_dm_off` | 52 | in `dm_off`: DMACON write of the last tick |
| `w_dm_on` | 24 | in `dm_on`: DMACON write of the other ticks |
| `w_dm_v_len` | 46 | in `dm_v_wr`: AUDxLEN write of the loop-pointer pass |
| `w_dm_v_adr` | 108 | in `dm_v_wr`: AUDxLC write (both halves) of the loop-pointer pass |
| `w_re_len` | 138 | in `re_ptr1`: AUDxLEN write of the row end |
| `w_re_adr` | 58 | in `re_ptr2`: AUDxLC write (both halves) of the row end |
| `w_fl_period` | 78 | in `fl_period`: AUDxPER write of the output step |

## 10. Validation

A value-and-timing model built from sections 1..9 only (the numbers of section 9 parsed from this file) was run
against the reference write logs of 14 runs (LOGS.md): registers, values and order of every write match, and every write
time matches the log to the colour clock (the log rounds down, as in 4.10). Notes and limits: README.md.
