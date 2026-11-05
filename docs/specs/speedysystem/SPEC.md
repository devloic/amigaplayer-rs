# Speedy System (Michael Winterberg, 1988) - behaviour specification

*Sanitised for publication: behaviour only.*

Format family: "Speedy System" modules, file prefix `ss.`. A four-voice Paula
tracker with a 256-position order list, 16-row tracks, per-note attack/release
volume envelopes, 8SVX-style samples with a one-time loop reload, and a period
slide effect. One subsong per file.

Scope: this document covers Speedy System only. The related "Speedy A1 System"
(prefix `sas.`) was not analysed in depth and no module for it exists in the
corpus; see README.md.

All timing numbers below are in 68000 cycles counted from the start of the
tick (the moment the CIA timer interrupt enters the player), unless stated as
colour clocks. A register write whose cycle offset from the tick start is `c`
is stamped at `tick_start + floor(c/2)` colour clocks. The cycle numbers were
measured with the reference 68k player run through the host with
DELI_WRITE_LOG (the host charges a fixed static cycle count per instruction),
so they reproduce the host logs exactly; they are not claims about real
hardware.

## 1. Module file

All multi-byte values are big endian.

### 1.1 Header and detection

* Bytes 0..12: the ASCII text `SPEEDY-SYSTEM`, followed by one more byte
  (zero in every file seen).
* A 32-bit marker value `0x00001190` at an even offset. The player's
  detection searches from offset 14 upward, two bytes at a time, for up to 128
  steps (offsets 14 .. 268), and accepts the first match (its information
  routine searches from offset 0x12 without a limit). In every file seen (and in all
  synthetic files) it is at offset 0x14. Call that offset `Mk`; all other
  offsets in the format are relative to `Mk`.
* Detection succeeds if the 13-byte magic matches and a marker is found in
  that window. (The marker is in fact voice 0's pointer for position 0, see
  below, so it also fixes where the first track lives: at `Mk + 0x1190`.)
* Header bytes: offset 14..17 holds a 32-bit value equal to (file length - Mk).
  It is not used for playback. The byte at `Mk-1` is the **speed** (see 3.1).
  The byte at `Mk-2` has no playback meaning; it only perturbs the player's
  start-up timing (section 6.1).

### 1.2 Position tables

At `Mk` there are four tables, one per voice, each 0x400 bytes: 256 big-endian
32-bit values. Entry `p` of voice `v` (at `Mk + v*0x400 + 4*p`) is the offset,
relative to `Mk`, of the track data that voice plays during order position `p`.
Position 0 of voice 0 is the marker value 0x1190.

### 1.3 Sample tables

At `Mk + 0x1000` there are 50 signed 32-bit sample offsets, followed (at
`Mk + 0x10C8`) by 50 sample lengths (unsigned 32-bit). A negative offset
means "no sample in this slot" (seen: `0xFFFCBE40`). The number of samples is
the count of non-negative offsets. The sample's start address in memory is
`Mk + offset + 7` (call this X; X is usually odd, offsets are authored that
way). The sample's region base B is X with the lowest bit cleared. The stored
length is the byte length of the whole sample object (including any IFF
header).

Sample objects, as seen in the corpus, are 8-bit signed data in one of three
shapes, distinguished by looking at the long word at B+0x60 and at B+0x28:

* Shape H60: an IFF 8SVX file (`FORM`...`8SVX`, VHDR, NAME, ANNO, BODY) whose
  BODY chunk id (`BODY`) sits at B+0x60. Then: one-shot length = long at
  B+0x14, repeat length = long at B+0x18 (as in the VHDR chunk), BODY byte
  length = long at B+0x64, sample data starts at B+0x68.
* Shape H28: a shorter IFF header with `BODY` at B+0x28; sample data starts
  at B+0x30; the one-shot length is still read from B+0x14.
* Raw: neither id present; the object is plain sample data. Playback starts
  at (X + 1) with bit 0 cleared, which is B+2 when X is odd (see 4.3).

### 1.4 Track data

Each track describes exactly 16 rows. A voice's track is a stream of:

* literal rows: 3 bytes `[b0, b1, b2]`, where b0 is not 0x90 or 0x92;
* run codes: `0x90 k` (k rows of `FF FF FF`) or `0x92 k` (k rows of
  `00 00 00`), two bytes. Runs are clamped so the track never exceeds 16
  rows. A run of k = 0 adds no rows and decoding continues with the next item.

Decoding stops once 16 rows are produced; any bytes after that (an authoring
tool appends `0x91`) are never read. Because tracks are decoded from the
position tables, several positions or voices may share one track.

A row's meaning is determined by b0 (section 3.3): `0x10..0x4F` note,
`0x50..0x8F` command, `0xFF` song end on voice 0, everything else ignored.

### 1.5 Song information

* Subsongs: 1.
* Title: none stored.
* Song length: the song ends at the first row processed on voice 0 whose first
  byte is `0xFF` (authoring tools make the last position's voice-0 track a run
  of 16 such rows), or after position 255 if none. Length in ticks is
  (rows played) x (speed + 1) with the tick period of section 2.
* The player's own information routine reports the number of samples
  (non-negative sample offsets), and treats trailing order positions whose
  four pointers all equal the last position's voice-3 pointer as unused when
  computing the number of positions.

## 2. Timing

The player is driven by one CIA timer interrupt with a period of 70940 colour
clocks (about 50 Hz PAL). Every interrupt is one "tick". Tick k (k = 0, 1,
...) starts at `E0 + k*70940`, where E0 is the first-tick epoch of 6.1.

## 3. Replay

### 3.1 State

Per voice: active flag, attack-phase flag, current volume (0..0x40), attack
period, release period, countdown timer, current period, stored command byte
and 16-bit argument, and a pending loop-reload (address and length) record.
Global: speed, tick counter, row (0..15), position (0..255), decoded rows of
the current position.

### 3.2 Tick types

* The tick counter starts at 0. When it is 0 at a tick, that is a **full
  tick**: the counter is set to `speed`, and the row is processed (3.3). Then
  the voices are updated (3.5) and pending loop reloads are performed (3.6).
* Otherwise it is a **light tick**: the counter is decremented and only the
  voice update (3.5) runs. No reload and no DMACON mask write.

So a new row starts every speed+1 ticks; speed 0 means every tick is full.

A full tick first expands the four tracks of the current position into 16 rows
each (every full tick re-expands, there is no cache), then processes the
current row for voices 0, 1, 2, 3 in order.

### 3.3 Row processing (per voice, in voice order)

Let b0, b1, b2 be the row bytes, o = b0 >> 4, n = b0 & 15.

1. `b0 = 0xFF`: on voice 0, the **song end**: position and row become 0, the
   tick counter becomes 0 (the next tick is full and the song restarts), the
   host's song-end report fires, and the remaining voices of this row are not
   processed. The rest of the tick (3.4 onward) still runs. On voices 1..3 the
   row is ignored.
2. `b0 >= 0x90` or `b0 < 0x10`: ignored.
3. `0x50 <= b0 <= 0x8F`: command. Store b0 as the voice's command byte and
   (b1 << 8 | b2) as its argument. The command stays in force every tick until
   the voice's next accepted note clears it. Only `0x50` (add) and `0x51`
   (subtract) do anything (3.5); other command bytes are stored but inert.
4. `0x10 <= b0 <= 0x4F`: note, octave o (1..4), note n. If n = 0 or n >= 14
   the row is ignored (no state change, no writes). Otherwise:
   * The voice's active flag is cleared; the command byte is cleared.
   * DMACON is written with the voice's bit (DMA off), then DMACON is written
     with 0x8200 twenty cycles later.
   * Period = table[n] (n = 1..13: 0x194, 0x17D, 0x167, 0x153, 0x140, 0x12E,
     0x11D, 0x10D, 0x0FE, 0x0F0, 0x0E2, 0x0D6, 0x000), shifted left by 3-o if
     o < 3, right by o-3 if o > 3 (o = 3: unchanged). Stored as the voice's
     current period (the AUDxPER write occurs below).
   * If b1 = 0xFF the note is silent: the voice stays inactive and nothing
     further is written (period is stored but not written).
   * Otherwise attack period = map[b1 >> 4], release period = map[b1 & 15],
     with map(i) = i for i < 14, map(14) = 16, map(15) = 24.
   * Sample index i = b2. If 0x31 <= i <= 0xB0 the note is rejected like the
     silent case (voice stays inactive). Indices must refer to a non-negative
     offset in the sample table; behaviour for a negative-offset slot or for
     i > 0xB0 is not defined here (see README).
   * Otherwise the note starts: attack flag set, volume 0, timer 0, and the
     writes of 4.3 are made. The voice becomes active.

After the last voice of the row (and not interrupted by a song end), the row
counter increments; after row 15 the position increments. If the position
wraps from 255 to 0 the song also ends (tick counter 0, report fired) with the
timing in 5.5.

### 3.4 DMACON mask

After the row has been processed, DMACON is written once with 0x8200 OR-ed
with one bit per voice whose active flag is set (bit 0 = voice 0 ...). This
write is made on every full tick (including the song-end tick).

### 3.5 Voice update (every tick, voices 0..3 in order)

For each voice:

* Inactive: nothing is written.
* Active:
  1. Effect: if the command byte is 0x50, period += argument; if 0x51,
     period -= argument (16-bit wrap). The new period is written to AUDxPER.
  2. Envelope.
     * Attack phase: if the timer is non-zero, decrement it (no change of
       volume). Otherwise: if attack period = 0 the attack completes at once;
       else timer = attack period and volume += 8, completing when volume
       reaches 0x40. On completion: attack flag cleared, volume = 0x40, timer
       = release period. AUDxVOL is written with the volume on every tick
       of the attack phase, including the waiting ticks.
     * Release phase: if release period = 0 the voice sustains forever; on
       every tick AUDxLC (a long write: high word then low word, same colour
       clock) is set to the player's silent-sample address and AUDxLEN to 4,
       and AUDxVOL is not written. Otherwise: if the timer is non-zero,
       decrement it and write AUDxVOL with the unchanged volume. Else timer =
       release period, volume -= 8; if the volume reaches 0 the note ends: the
       active flag is cleared, DMACON is written with the voice bit, then
       0x8200, then AUDxVOL = 0 is written. Otherwise AUDxVOL is written with
       the new volume.

### 3.6 Loop reload (full ticks only, after the update)

For each voice with a pending reload record (set when a note started with a
sample of shape H60 having both a one-shot and a repeat part): write AUDxLC
(long) with the loop start address and AUDxLEN with the loop length in words,
then clear the record. Voices without a record are skipped.

## 4. Paula writes of a note start

### 4.1 Order

For the accepted note: DMACON(voice bit), DMACON(0x8200), AUDxPER, AUDxLC
(long, written as two word writes, high word first, same colour clock), AUDxLEN,
then (for shapes H60 and H28 only) AUDxLC (long) and AUDxLEN again, then the
tick's DMACON mask (3.4), the update writes (3.5), the reload writes (3.6).

### 4.2 Registers

AUDxLC/LEN/PER/VOL of voice v live at 0xDFF0A0 + 16*v + (0, 4, 6, 8).
A long write to AUDxLC appears in the log as the high word at offset 0
and the low word at offset 2 with the same colour clock stamp.

### 4.3 Values

First write set (always): AUDxPER = period; AUDxLC = (X + 1) with bit 0 cleared;
AUDxLEN = ((stored length - 12) >> 1) & 0xFFFF.

Second write set, by sample shape (all AUDxLEN values are 16-bit):

| Case | Condition | AUDxLC | AUDxLEN | Reload record |
|------|-----------|--------|---------|---------------|
| A | H60, repeat length = 0 | B+0x68 | body length >> 1 | none |
| B | H60, repeat length != 0, one-shot length = 0 | B+0x68 | body length >> 1 | none |
| L | H60, both non-zero | B+0x68 | (one-shot - 4) >> 1 | LC = B + one-shot + 0x64, LEN = repeat >> 1 |
| M | H28 | B+0x30 | (one-shot - 4) >> 1 | none |
| R | raw | no second write | | none |

(body length = long at B+0x64; one-shot = long at B+0x14; repeat = long at
B+0x18.)

## 5. Timing ledger (cycles from tick start)

### 5.1 Light tick

The voice update (5.3) starts at cursor 315 + 20 = 335.

### 5.2 Full tick

Cursor c starts at 501. Expansion adds
`1756 + 123*L + 104*R + 47*N90 + 70*N92 + 31*C` where, summed over the four
voices, L = literal rows, R = rows produced by runs (after clamping), N90 /
N92 = number of 0x90 / 0x92 run codes, C = number of runs that ended with the
track still incomplete (including k = 0 runs). Then add 40. This is s0 for
voice 0.

Voice slots run in order. Each slot starts at s0 (the cursor when it begins)
and advances the cursor by its duration; voice v+1's s0 is voice v's end.

Slot durations by row kind, per voice [v0, v1, v2, v3]:

* Ignored row (also 0xFF on voices 1..3, which adds 8): [206, 206, 206, 257]
* Command row: [385, 385, 385, 436]
* Note with n = 0: [322, 322, 322, 373]; with n >= 14: [347, 347, 347, 398]
* Note with b1 = 0xFF: [836, 891, 946, 1052] + O
* Note with rejected sample index: [1077, 1132, 1187, 1293] + O
* Accepted note, by sample case: A [3055, 3265, 3475, 3621];
  B [3090, 3300, 3510, 3656]; L [3170, 3380, 3590, 3736];
  M [3063, 3273, 3483, 3629]; R [2763, 2911, 3059, 3189]; each + O
* O (octave term) = {1: 53, 2: 8, 3: 0, 4: 45}[o]
* Voice 3 on row 15 adds 75 to its slot.

Write offsets inside a note slot (from the slot's s0), per voice:

* DMACON(voice bit): [368, 423, 478, 533]; DMACON(0x8200): +20 after it.
* AUDxPER: [1277, 1332, 1387, 1442] + O; AUDxLC (first set): 16 cycles after AUDxPER.
* AUDxLEN (first set): [1653, 1770, 1887, 1958] + O.
* Second set, AUDxLC offset by case (+O), per voice:
  A [2323, 2471, 2619, 2698]; B [2358, 2506, 2654, 2733];
  L [2484, 2632, 2780, 2859]; M [2331, 2479, 2627, 2706].
  Second AUDxLEN: 12 cycles after that second AUDxLC.

After the four slots, with D0 the cursor and nact the number of active voices
after the row: the DMACON mask write is at D0 + 622 + 8*nact. The
voice update then starts at D0 + 656 + 8*nact + 20.

### 5.3 Voice update chain

A running cursor t starts at the value given above. For each voice in order:

* Inactive: t += 156.
* Active: first any effect: for command 0x50 the AUDxPER write is at
  t + 310, and an extra eo = [228, 259, 290, 298][v] is added to
  every later offset and to the voice's duration; for 0x51 the write is at
  t + 333 and eo = [251, 282, 313, 321][v]. No effect: eo = 0.
  Then the envelope case gives (AUDxVOL write offset, duration) at voice 0:
  attack waiting (719, 903); attack step (835, 1019);
  attack done with attack period 0 (858, 1042);
  attack done by reaching 0x40 (941, 1125);
  release waiting (819, 1003); release step (949, 1133).
  The write happens at t + offset + VO[v] + eo and the cursor advances by
  duration + VD[v] + eo, where VO = [0, 23, 46, 36] and VD = [0, 54, 108, 106].
  Sustain (release period 0): AUDxLC at t + 433 + eo, AUDxLEN at
  t + 453 + eo, cursor += 524 + eo (same for all voices).
  Note end: DMACON(voice bit) at t + 742 + 55v + eo, DMACON(0x8200)
  at t + 762 + 55v + eo, AUDxVOL = 0 at t + 1168 + 55v +
  VO[v] + eo, cursor += 1352 + 55v + VD[v] + eo.

### 5.4 Reload pass (full ticks)

Starts at cursor (end of update) + 34, then voice 0 begins at +38.
Per voice: with a record, AUDxLC at t + 273, AUDxLEN at
t + [416, 447, 478, 486][v], then t += 278 + 320 + 62*v;
without a record t += 278.

### 5.5 Song-end timing quirk

The host's end report discards the half-cycle accumulated so far and adds 16
cycles. Model: when the end occurs, define the trap cursor ct; later writes in
the same tick are stamped tick_start + floor(ct/2) + floor((c - ct)/2).

* Voice-0 end row: ct = s0 + 258; the slot duration is 385.
* Position wrap (after row 15, voice 3): ct = (cursor after the slot incl. the
  row-15 extra) + 67; then the cursor advances by a further
  194.

## 6. Start-up

### 6.1 First tick and initialisation writes

Let E0 be the first tick. With the reference host, E0 = 87738 + floor(Q/2),
where Q (cycles consumed by the player's initialisation scan) is

`Q = 2162 + 136*k + a(j) + 39*lead + 8*np - (2 if speed != byte at Mk-2)`

* k: counting positions from 255 downward, the number of consecutive positions
  all four of whose pointers equal voice 3's pointer at position 255.
* j: at the first non-matching position, test voices 3, 2, 1, 0 in that order;
  j is the first voice whose pointer differs; a(3) = -31, a(2) = 0,
  a(1) = 31, a(0) = 62.
* lead: number of leading sample-table entries with the sign bit set (before
  the first non-negative one).
* np: number of non-negative sample offsets.

Eight AUDxVOL = 0 writes are produced during initialisation: voices 0..3 at
floor(Q/2) + 497, + 629, + 773, + 912 colour clocks (absolute), then the same
four again 664 colour clocks later. The first full tick follows at E0.

The stamps above are tied to the host's initialisation sequence; an
implementation that does not need cycle-exact first-tick position can use any
E0 and keep the 70940 spacing.
