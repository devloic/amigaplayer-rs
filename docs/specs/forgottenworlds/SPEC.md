# Forgotten Worlds game music ("FWMP" modules): behaviour specification

*Sanitised for publication: behaviour only.*

Format: module files whose first four bytes are the ASCII text `FWMP`. Files named `fw.<tune>` in the
archives (prefix `fw`). The tunes belong to the Amiga game Forgotten Worlds (the replay routine is tiny and the module holds only data). One module = one tune, no
subsongs, no title text, no length information.

All numbers are decimal unless prefixed `0x`. "Tick" = one call of the player's interrupt routine,
one per 50 Hz period. "cc" = Amiga colour clock (3.546895 MHz, 227 per raster line), "cycle" = 68000
bus cycle of the reference run, 2 cycles = 1 cc.

## 1. Detection and structure of a module

Detection: the first four bytes equal `FWMP`. Nothing else is checked. Everything below is big-endian.

Header, 16 bytes:

| offset | size | meaning |
|---|---|---|
| 0 | 4 | `FWMP` |
| 4 | 4 | A: offset of the pattern table, relative to byte 16 |
| 8 | 4 | B: offset of the instrument table, relative to byte 16 |
| 12 | 4 | C: offset of the waveform directory, relative to byte 16 |

Define `base = 16`, `PT = base + A`, `IT = base + B`, `WD = base + C` (offsets from the module start).

* At `base` there are four 16-bit words: the start offsets (relative to `base`) of the event streams
  of audio channels 0, 1, 2, 3. Stream start = `base + word`. The rest of the file up to `PT` holds the
  streams themselves.
* Pattern table at `PT`: a list of 16-bit offsets (relative to `PT`, treated as signed 16-bit). Pattern `p`
  starts at `PT + signed(word[p])`. The first word is also the size of the table in bytes (the first
  pattern follows the table directly), which tells how many patterns there are. Patterns hold the same
  kind of events as streams (section 3).
* Instrument table at `IT`: 8-byte entries, number of entries = `(WD - IT) / 8`. Entry `i` is at
  `IT + 8*i`:

  | byte | meaning |
  |---|---|
  | 0 | offset (in bytes, a multiple of 8) of this instrument's waveform entry inside the waveform directory |
  | 1 | sustain level S (the volume the decay stage stops at) |
  | 2 | attack step / start level (see 4.2) |
  | 3 | decay step (signed byte, normally negative) |
  | 4 | hold time in ticks (0 = this instrument has no volume envelope at all, see 4.2) |
  | 5 | release step (signed byte, normally negative) |
  | 6,7 | unused (zero in all samples) |

* Waveform directory at `WD`: 8-byte entries, referenced by instrument byte 0:

  | bytes | meaning |
  |---|---|
  | 0-1 | offset of the sample data, relative to `WD` |
  | 2-3 | length of the first pass in 16-bit words (AUDxLEN) |
  | 4-5 | offset of the loop start inside the sample, in bytes |
  | 6-7 | loop length in words |

  The sample data (signed 8-bit PCM) follows the directory (the first entry's data offset equals the
  directory size). Sample start address = module start + `WD` + offset; loop start address = sample
  start address + loop offset. The first-pass part and the loop part overlap: the loop is the tail of the
  sample.

Quirk of instrument selection (not exercised by any sample module): the directory entry used is at
byte offset `instr_byte0 + 256 * (((index*8) >> 8) & 255)` where `index` is the signed 8-bit instrument
number from the event; for numbers 0..31 this is simply `instr_byte0`.

### Period table

Notes 0..45, in colour clocks (AUDxPER):
`1712 1616 1520 1440 1360 1280 1208 1144 1080 1016 960 904 856 808 760 720 680 640 604 572 540 508 480 452 428 404 380 360 340 320 302 286 270 254 240 226 214 202 190 180 170 160 151 143 135 127`.
A note byte >= 46 and < 128 yields period 0.

## 2. Player state

Four independent channels, each with:

* event pointer, duration counter (8 bit), "loop start" pointer (the channel's stream start), a single
  return pointer for pattern calls;
* current instrument (none at the start), current sample start/length/loop start/loop length from the last
  instrument selection;
* envelope: a stage, a level (8 bit, kept in a 16 bit word whose high byte stays 0), a hold counter (8 bit).

Initial state when the song is started (the song starts in the same state every time the module is played
for the first time; the envelope fields are not reset by a restart):

* duration counter = 1 (so every channel reads its stream on the first tick), event pointer = stream start.
* no instrument selected.
* envelope level / stage / hold counter: channel 0: 0x30 / hold stage / 45; channel 1: 0x40 / invalid
  stage / 50; channels 2 and 3: 0x28 / attack stage / 0. (These matter only if an instrument is selected and
  has a non-zero hold time while no note has been played yet; observed for channel 3 in one tune, the
  invalid stage of channel 1 was never exercised: behaviour undefined.)
* volume scale per channel: 64 (see section 6).

Global: play flag (set when the song is initialised).

## 3. Event streams

A stream is a sequence of bytes read from the event pointer. A byte below 0x80 is a note; a byte of
0x80 or more is a command. Commands are even numbers.

| byte | arguments | action |
|---|---|---|
| `n` < 0x80 | duration `d` (1 byte) | note: key `n`, lasts `d` ticks. Starts a note (section 5) and ends this channel's turn for the tick. |
| 0x80 | none | end: stops the tick routine at once, later channels are not served this tick (details below) |
| 0x82 | none | restart: event pointer = the channel's stream start; continue reading |
| 0x84 | instrument `i` (signed byte) | select instrument `i` (see 3.1); continue reading |
| 0x86 | duration `d` | rest: duration counter = `d`; ends the channel's turn. Envelope keeps running. |
| 0x88 | pattern `p` (signed byte) | call: return pointer = pointer after the argument; event pointer = start of pattern `p` (`PT + signed(word at PT + 2*signed(p))`); continue reading |
| 0x8a | none | return: event pointer = the stored return pointer; continue reading |

Commands above 0x8a are not used (undefined). Only one level of call exists (the return pointer is a
single slot, a second call overwrites it). There is no loop counter; songs loop by 0x82 at the end of the
channel streams.

0x80 (observed in the "jingle" tune, which ends that way, at the end of each of its streams): the
current tick ends immediately; channels after the current one are neither decremented nor read. The
event pointer stays after the 0x80 and the duration counter is 0, so on the next tick the counter wraps to
255 and the channel runs the envelope for 255 ticks before reading the following byte (which is not part
of the stream: undefined data). The routine also zeroes a 16-bit word in the host's player data area; the
effect on output was not observed. The reference does not stop the replay; the tune simply becomes silent
(all envelopes reach 0) apart from garbage after the streams. Treat 0x80 as "end of song for this channel";
a port should stop reading events after it. Verified against the reference up to the first 0x80 of each
channel; the behaviour after the data end was not matched (see README).

### 3.1 Instrument selection (0x84)

Set the channel's current instrument pointer to `IT + 8*signed(i)`. Take instrument byte 0 as directory
offset into `WD` (quirk: section 1). Latch from that directory entry: sample start (= `WD` + word 0),
first-pass length (word 1), loop start (= sample start + word 2), loop length (word 3). Does not touch
the envelope, level or period, and does not touch the hardware.

## 4. Per-tick algorithm

Each tick, in order:

1. If the play flag is clear, do nothing at all (no writes).
2. Write DMACON = 0x800F (set bits for all four audio channels). This happens on every tick, including
   ticks in which nothing else is written.
3. For channels 0, 1, 2, 3 in this order:
   a. Decrement the 8-bit duration counter (wraps 0 -> 255).
   b. If the result is not zero ("running"): if the channel has no instrument or the instrument's hold
      time (byte 4) is zero, nothing happens (no write). Otherwise run one envelope step (4.2) and write
      AUDxVOL (4.1).
   c. If the result is zero ("expiry"): read events (section 3) until a note or a rest ends the turn. A
      note starts the hardware sequence of section 5.
4. End of tick.

### 4.1 Volume written

`vol_out = (scale[ch] * level) >> 6`, as a 16-bit value, `scale` = 64 by default so `vol_out = level`. A level
above 64 is written as is (Paula limits it to 64). The envelope step writes AUDxVOL only; the period and the
sample pointers are never rewritten by the envelope.

### 4.2 Envelope

The level is an 8-bit value, stages advance as follows. A note start sets level := instrument byte 2 and
stage := attack (with the volume written as part of the note sequence). At every later "running" tick with
a non-zero hold byte, one step of the current stage is executed, then AUDxVOL is written:

* attack: level += byte 2 (8 bit, wrap). If level >= 0x40 (unsigned): level := 0x40, stage := decay.
* decay: level += byte 3 (8 bit, wrap). If level (unsigned) is not greater than the sustain byte (byte 1):
  level := byte 1, hold counter := byte 4, stage := hold. Otherwise stay in decay.
* hold: hold counter -= 1 (8 bit). If it becomes 0, stage := release; level unchanged.
* release: level += byte 5 (8 bit). If the sum has bit 7 set (negative): level := 0, stage := stopped.
* stopped: nothing changes; AUDxVOL is written with 0 on every tick.

A rest does not change the envelope. A note start with an instrument whose hold byte is 0 never writes
AUDxVOL again (the volume of the note start stays).

## 5. Hardware sequence of a note start (exactly this order)

Let `ch` be the channel number, `R` its audio register block (AUDxLC at 0xDFF0A0 + 0x10*ch), `B = 1<<ch`,
`I = 0x80 << ch` (the channel's audio interrupt bit). The period comes from the period table, volume,
sample start, first-pass length from the current instrument/sample.

1. INTREQ <- I (clear)
2. DMACON <- B (clear this channel's DMA)
3. AUDxPER <- 0x0001
4. AUDxVOL <- 0
5. wait until INTREQR shows I (poll W1, section 7.3)
6. INTREQ <- I
7. AUDxDAT <- 0
8. wait for I (W2)
9. AUDxDAT <- 0
10. wait for I (W3)
11. AUDxLC <- sample start (high, then low word, same clock), AUDxLEN <- first-pass length, AUDxPER <- period,
    AUDxVOL <- vol_out for the start level
12. DMACON <- 0x8000 | B
13. INTREQ <- I
14. wait for I (W4)
15. INTREQ <- I
16. AUDxLC <- loop start (high, low), AUDxLEN <- loop length

The channel's Paula DMA is therefore off for the register setup, on for the first pass, and its pointers
are re-aimed at the loop before the first pass ends. The envelope does not run in the note-start tick.
(The wait steps are short polls except in rare cases, see 7.3.)

The writes in step 11 are: LC hi, LC lo (same clock), LEN, PER, VOL.

## 6. Volume and balance

The host may change the global volume (0..64) and left/right balance (0..64). Each channel scale is
recomputed on such a change: `scale = (balance * volume) >> 6`, where channels 0 and 3 use the left
balance and channels 1 and 2 the right balance. Both are 64 by default, so scale = 64 always in the
reference logs. (Read from the module player; not exercised in the logs.)

## 7. Start-up, stop and timing

### 7.1 Song initialisation

On starting a song the player sets every channel's event pointer and counter as in section 2 and sets the
play flag, then writes the following (the first 24 writes of every log):

* for channel 0, 1, 2, 3: AUDxLC <- the address of a 4-byte silence buffer in the player's own data
  (the same for all four), AUDxLEN <- 2, AUDxPER <- 0x007C, AUDxVOL <- 0. Per channel the write of LC hi and lo has
  the same clock; the gaps are LC->LEN 8 cc, LEN->PER 8, PER->VOL 8, VOL -> next channel's LC 15 cc.
* then four stray writes, 17 cc after the last VOL: 0xDFF17E <- 0x00FF, then (8 cc apart each) 0xDFF176 <-
  0x000F, 0xDFF17C <- 0x0780, 0xDFF17A <- 0x0780. These are writes to sprite data registers made by
  accident (wrong place); they have no audible effect.

Measured clocks (the reference start phase): first init write at clock `S`; first tick's DMACON write at
`S + 71249`. Channels pass 15 cc between the last VOL write of one channel and the next channel's LC;
the whole init sequence takes 182 cc (from the first to the last write).

Stop (the host stops the song): the play flag is cleared and the same 24 hardware writes of section 7.1 are
done again (silence buffer, LEN 2, PER 0x7C, VOL 0, stray writes). Not exercised in the logs.

### 7.2 Tick schedule

The interrupt comes every 70940 cc. The k-th interrupt is raised 185 cc before the clock
`G + 70940*k` where `G` is the first tick's DMACON write; if the routine is still running when the next
interrupt arrives, the interrupt is held and the tick starts as soon as the routine has finished. No
interrupt is ever dropped: after a long tick (see the long waits below, up to about 91 frames) the held
ticks run back to back, about 780-900 cc apart, until the schedule is caught up.
Precisely: let `E` be the time, in cycles (half colour clocks), at the end of the previous tick's channel 3
turn (7.4, including its epilogue); tick k's DMACON write happens at the clock
`max(G + 70940*k, floor((E + 370)/2))`, i.e. 370 cycles (185 cc) after `E` when the tick is late. This matched
every tick start of the logs (about 6300 ticks, several hundred of them late).

### 7.3 Waiting for the audio interrupt flag

Steps 5, 8, 10, 14 of section 5 poll INTREQR. One poll iteration takes 25 cycles; the k-th read samples the
flag 13 + 25*k cycles after the previous write finished (k = 0, 1, ...). The flag is seen if the audio hardware
raised it strictly before that sample time. After the iteration that sees it, the next write follows
after: W1 12 cycles, W2 16, W3 28, W4 12 (these include the following write's own cost). So the write after
a wait comes `25*(k+1) + 12` (W1, W4), `25*(k+1) + 16` (W2) or `25*(k+1) + 28` (W3) cycles after the
write that preceded the wait, where k = number of unsuccessful iterations.

When the flag is raised (all by Paula's audio state machine, hence a port should let its Paula decide; the
following was measured in the reference):

* W1 (after DMA off, PER=1, VOL=0): Paula stops a playing channel at the end of its current period/word;
  the wait is 0..(old period) cc long plus the polling granularity (measured: always <= old AUDxPER + 16 cc,
  measured 18..431 cc).
* W2 (after the first AUDxDAT write): found at the first iteration (k = 0) in about 98.5% of cases (gap 41 cycles);
  otherwise it waits for about the old period (measured gaps 108..495 cc for old periods 127..508).
* W3 (after the second AUDxDAT write): always the first iteration (53 cycles).
* W4 (after DMA on): the flag rises when the channel fetches its first word. The fetch happens on the
  first raster-line boundary (clock multiple of 227) after the DMACON write (`L = 227*(floor(T/227)+1)` with
  `T` the clock of the DMA-on write). The INTREQ clear that follows the DMACON write
  (13 cc later) comes first if `L - T > 13`; then the poll finds the flag at the first iteration with sample time
  after `L`. If the line boundary is 13 cc or less after the DMACON write (`L - T <= 13`, about 5% of the notes),
  the flag is raised before or at the clear and so the clear removes it: the wait then lasts until the
  next flag, which is the end of the first pass of the sample: about `L + 2*period*(length - 1) + 227`
  (measured as 239..250 cc between L + 2*period*(length-1) and the write after the wait). Such notes stall
  the routine for as long as the first pass takes: 0.6 to 6.4 million cc (up to 91 frames; the longest is a
  6347-word sample at period 508), which is why late ticks occur.

### 7.4 Cost of the tick in cycles (reference run)

Time within a tick is counted in cycles (2 per cc) from the clock of the tick's DMACON write (cycle 0).
The clock (in cc) of a write is `floor(cycle/2)` added to that clock. All figures were fitted to the logs; they
reproduce every write gap of the four complete tunes except the corrections in README. "Gap" lists are
additive: costs of channels that produce no write are simply part of the gap before the next write.

Per tick:

| step | cycles |
|---|---|
| after the DMACON write until channel 0 is examined | 33 |
| channel turn ends (after anything the channel did, including its last write): channels 0..2 | 42 |
| same for channel 3 | 46 |
| after channel 3's turn: until the next tick's DMACON write (when the next tick is already due) | 370 |

Per channel turn, depending on the case (the epilogue above is added afterwards):

| case | cycles |
|---|---|
| running, no envelope (no instrument, or hold byte 0) | 74 |
| running with envelope, up to the end of the stage step (4.2) | 105 + stage cost |
| ... stage cost: attack not yet clamped 46; attack reaches 0x40 78; decay stays 50; decay settles at sustain 106; hold stays 42; hold ends 66; release stays 38; release goes negative 66; stopped 17 | |
| ... then until the AUDxVOL write | 144 |
| ... then until the end of the turn (before the epilogue) | 10 |
| expiry: from the start of the turn until the first event byte has been classified | 48 |
| each event after that adds (to the clock where the previous one ended): note 134, then its first write (step 1 of section 5) completes 12 cycles later; rest 92; call 130; return 86; restart 83; instrument select 271 | |
| after the last write of a note start (step 16 LEN) until the end of the turn | 40 |

Writes inside the note start (step n to step n+1; the write itself is included in the figure for the step
that precedes it):

| from -> to | cycles |
|---|---|
| step 1 -> 2 INTREQ -> DMACON (clear) | 12 |
| 2 -> 3 DMACON -> AUDxPER | 16 |
| 3 -> 4 AUDxPER -> AUDxVOL | 16 |
| 4 -> 6 (W1) VOL -> INTREQ | 25(k+1)+12 |
| 6 -> 7 INTREQ -> AUDxDAT | 16 |
| 7 -> 9 (W2) AUDxDAT -> AUDxDAT | 25(k+1)+16 (k = 0: 41) |
| 9 -> 11 (W3) AUDxDAT -> LC | 25(k+1)+28 (k = 0: 53) |
| LC -> LEN, LEN -> PER | 20 each |
| PER -> VOL (volume computation) | 168 |
| VOL -> DMACON (start) | 24 |
| DMACON(start) -> INTREQ | 26 |
| 13 -> 15 (W4) INTREQ -> INTREQ | 25(k+1)+12 |
| INTREQ -> LC (loop) | 28 |
| LC (loop) -> LEN | 20 |

A write of a 32-bit address is two 16-bit writes with the same clock (high word first). The pair
LC hi/lo of step 11 and of step 16 both use that.

Rounding: all of this is exact when the cycle counts are summed from the tick start before halving. Summing
rounded gaps gives errors of a colour clock.

## 8. Song end, length, restart

There is no end detection and no length: the streams loop (0x82) or run out into undefined data (0x80). A
host should play for its default time. A restart (host calls song initialisation again) resets pointers,
counters, flag and the hardware only; the envelope stages and levels, the current instrument and the
return pointers survive from the previous run (they are only set up at program load).
