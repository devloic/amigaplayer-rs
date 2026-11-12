# EMS v6 (`*.ems`, "E.M.S. V6.0x"): behaviour spec

Scope: modules of Sean Connolly's Electronic Music System version 6 (the game music of Stormlord,
Tropical Islands, A Sporting Chance, Robocop 3, Galaxy Man and the test songs of the system), as
found on modland (`EMS v6`, 10 files). A software mixer: up to 32 tracks of sampled or 64-byte
synthetic voices mixed into two 8-bit rings that Paula plays.

Method: black box only. The reference is UADE's Eagleplayer `EMS-6` on this project's 68000 engine
(`DELI_CPU_SPEED=8`: on a 68000 its mixer misses timer interrupts). Observed: the Paula and CIA
register writes with their colour clocks (`DELI_WRITE_LOG`), the bytes it writes into its output
rings and when (a write watch), which module bytes it reads and when (a read watch), and the output
of built modules (the layout of section 2 with edited sequences, patterns, tables and instruments:
differential probing). The player was not disassembled or read. No code of it is in this text; the
one table of its data that the format needs (section 5.2) is kept beside the port in
`src/replay/ems6_hi.rs`, measured from the output, embedded by the owner's decision. The pitch values are ProTracker's period table (a public table, used from
`crate::protracker::PERIODS`) and output step values measured from long notes.

Times are colour clocks ("cc", 3 546 895 a second). All words and longs big-endian.

## 1. Reference

| item | finding |
|---|---|
| Eagleplayer `EMS-6` | takes all 10 modland files; plays one subsong; reports a song end (section 7.4) |
| CPU | on a 68000 (cycle-counted) its interrupt takes longer than the timer period with many voices: the reference runs are made at `DELI_CPU_SPEED=8`, where it keeps up |
| start-up | a time that grows with the voice count `a` (it builds tables): section 9 |

## 2. Module layout

| offset | size | meaning |
|---|---|---|
| 0 | 10 bytes | `E.M.S. V6.` (the version digits after it are ignored: `00`..`05` and others all play) |
| 0x0E | long | L0: length of block 0 |
| 0x12 | long | L1: length of block 1 |
| 0x16 | long | L2: length of block 2 |
| 0x1A | long | L3: offset of block 3 from 0x22 |
| 0x22 | L0 | block 0: arpeggio tables |
| 0x22 + L0 | L1 | block 1: word `a` (voice divisor), word `b` (track count), 32 longs: sequence offsets from block 1 |
| block 1 + L1 | L2 | block 2: patterns |
| 0x22 + L3 | .. | block 3: instruments |

Blocks 0 and 2 are record blocks: the block starts with a table of longs, offsets of the records
from the block's start; the first offset divided by 4 is the record count; a record runs to the next
record's offset (the last one to the block's end).

Block 3 is a table of longs (offsets from block 3) closed by `FFFFFFFF`; record `k` is instrument
`k`. An instrument starting `FORM....8SVX` is an IFF 8SVX sample: one-shot length at +0x14, repeat
length at +0x18, data at +0x68. One starting `SYN@` is a synthetic voice: 64-byte waves from +6. Any
other is raw signed bytes from its start, its length the next offset less its own, less 2 (one-shot).

Track `i`'s sequence is the words from block 1 + offset `i` to block 1 + offset `i+1` (the last
track's to block 1 + L1).

Detection (the reference, from 200+ built files): the magic, L0 not zero, `a` and `b` not both zero,
files of 64 bytes and more (a file of 56 bytes is refused). The port: the magic, a file of at least 64
bytes, 0 < L0 <= file length, 1 <= `a` <= 255, 1 <= `b` <= 32.

## 3. Sequences

Each track reads its sequence word by word when its pattern ends:

| word | effect |
|---|---|
| 0..7FFF | play pattern `w` (block 2 record `w`; a number past the block is an empty pattern) |
| FFFF | the track has looped (section 7.4); continue at the word index given by the next word |
| FFFE | the track stops for good (its voice keeps what it had, it is not ticked again) |
| FBxx | play the next pattern `xx` times in all |
| FDxx | transpose: `xx` signed, added to every note |
| FCxx | instrument offset: `xx` added to the instrument number of notes |
| FAxx | skips the following word |
| F8xx, F9xx | played as a pattern whose data are block 2 from its start (the offset table) to the file's end (seen with read watches in Galaxy Man) |
| other 8000.. | ignored |

## 4. Patterns

A pattern is a byte stream; a row is commands up to a note or a hold. Bytes below `EF` are notes
(`EA`..`EE` included). Command parameters past the pattern's end read 0.

| byte | params | effect |
|---|---|---|
| < EF | | note: starts the track's instrument (unless legato), waits `dur * speed` ticks |
| FF | | end of pattern: the next from the sequence |
| FE | i | instrument `i` (default 0) |
| FD | n | duration in rows (0 = 256) |
| F7 | v t s d | volume envelope: volume `v`, moving to `t` by `s` every `d + 1` ticks |
| F6 | | hold: the voice plays on (re-enabled if it had ended, from where it is) for a duration |
| F5 | n s | release: in the note's last `n` ticks the volume falls by `s` more each tick |
| F4 | k n | note `n`, `k` times (the command is read again `k` times) |
| F3 | e k | retrigger: the next note's sample restarts every `e` ticks, `k - 1` times |
| F2 | lo hi w d | synthetic wave sweep: start wave `w`, `d` waves a tick between `lo` and `hi` |
| F0 / F1 | | legato on / off: notes do not / do restart the sample |
| EF | x | fine tune `x & 0x0F` (ProTracker's fine tunes, 0..7, -8..-1) |
| FC | x | `x < 0x20`: speed (ticks per row, default 6); else the tick length in samples, 70095 / (2 `x`) truncated (default 280) |
| FB / FA | n c d m | slide from note `n` to note `m` (transposed) in `c` ticks after `d` ticks; FB starts the sample, FA does not |
| F9 | t | arpeggio table `t` (block 0 record) |
| F8 | a b d | vibrato: depth divisor `a`, amplitude `b`, delay `d` ticks; arpeggio off |

## 5. Pitch

### 5.1 Notes 0 to 0x5F

The note index is `(byte + transpose) & 0x7F`. The period comes from rows of 96 built from
ProTracker's fine-tune row (octaves 1 to 3, `PT1 PT2 PT3`): `16 PT1, 8 PT1, 4 PT1, 2 PT1, PT1, PT2,
PT3, PT3 / 2` (each 12 notes). The step (16.16 ring bytes a sample) is
`floor(floor(3579264 / period) * 65536 / 3504)`.

### 5.2 Notes 0x60 and up

No period: a step of `floor(F * 65536 / 3504)`, `F` from a table of rows of 97 entries, one row per
fine tune (0 to 15), contiguous: index `0x60 + j` of fine tune `k` is entry `k * 97 + j` (so j >= 97
reads the next row), row 0 starting 248, 263, 278, 295 ... (measured from the ring output of a ramp
instrument, 1455 values, `src/replay/ems6_hi.rs`). Row 15 and past it read as 0: a step of 0, the voice
is silent and does not advance. A plain note byte is masked to 0x7F; an arpeggio's `(raw + x) & 0xFF`
reaches up to 0xFF.

### 5.3 Effects

* Slide: delta `(target period - period) / c` (truncated) a tick after the delay; on the last step
  the note becomes the target. No vibrato while a slide runs.
* Arpeggio: each tick the next table byte `x`; `FF` restarts the table, `FE` repeats the last one.
  When `x` is not 0, or the note byte (with transpose) has bit 7 set, the note
  `(raw + x) & 0xFF` plays, keeping the slide's offset from the base period. Indexes 0x60 and up use
  the table of section 5.2 with the track's fine tune.
* Vibrato (not while an arpeggio steps or a slide runs): after the delay, a value going +1 a tick up
  to `b`, -2 a tick down to `-b`, +1 back to 0, one tick rest; the period gets
  `value * floor((period(n-1) - period(n)) / a)`.

## 6. Volume

Per tick: during the release (the track's remaining wait below `n`), `relsub += s` and the envelope
stands still (neither the volume nor its counter moves: Tropical Islands' fade-out under F6 at tick
3075 needs it); otherwise `relsub = 0` and the envelope moves the volume toward the target by its
step every `d + 1` ticks. The voice's volume is `max(0, volume - relsub)`. A volume over 64 is a
factor as it is here; the original reads its volume table past row 64 there (mostly 0x80 bytes,
depending on the module's layout), which is not reproduced and which no known module needs.

## 7. Timing and the sequencer

### 7.1 Interrupts

A CIA timer (latch 0x3781) interrupts every 71050 cc. Each interrupt mixes 280 samples a side, 282
when an accumulator (start 450) stepping by 97 reaches 512 (then less 512). The first two interrupts
mix nothing.

### 7.2 Ticks

The sequencer ticks every `tlen` samples of the mixing (at the start of the block when due): all
tracks in order, each: the wave sweep, the row if its wait is over (else the wait moves on), the
retrigger, the volume, the pitch. A wait of `dur` rows counts rows, not ticks: a tick of the wait
adds to the row in progress and a row ends after `speed` ticks, with the speed as it was when the
tick began (a speed change by another track moves the end of a wait under way, from the next tick
on); the wait left is `rows_left * speed - ticks_into_row - 1`. A wait set in the tick uses the
current speed (`dur * speed - 1`).

### 7.3 Voices

Voice `i` is track `i`; even tracks mix to the left ring, odd to the right. A sample is
`sum(s * vol >> 6) / a` truncated, stored as a byte. A voice past its end (one-shot) stops; a looped
one continues at loop start plus the overshoot. Wraps are checked at block (part) boundaries with
the rules of the port's `mix` (a voice past its end when a part starts plays the part without a wrap
and reads behind by the loop length plus the overshoot until its next wrap; one exactly at its end
wraps before its first sample). A one-shot voice that ends within a part goes back to the position it
had at the start of that part; an F6 that re-enables it plays on from there. A voice left exactly at
its end by a part (it stops at the next part's start): a raw instrument goes back to the start of the
part in which it reached its end (Stormlord, F6 at tick 3990), an 8SVX one stays at its end (Tropical
Islands, F6 at tick 3756). Only these two cases were seen; synthetic voices always loop.

### 7.4 Song end

When every track that has not stopped (`FFFE`) has reached its `FFFF`.

## 8. Output

Two rings of 0x8000 bytes (left, right). Paula: LEN 0x4000, period 0xFD, volume 64; channels 0 and
3 play the left ring, 1 and 2 the right. The mixer writes each block after the previous one; Paula's
pointer leads the mixer by about 1100 bytes and gains on it (253 cc a byte against 253.4), so what is
heard is the previous lap (silence for the first 2.3 s).

## 9. Start-up

Writes of the reference, at absolute clocks for voice count `a`:

| write | clock |
|---|---|
| DMACON 000F | 3209 + 2.126 `a` |
| LEN, PER, LOC, VOL of the 4 channels | +71 .. +86 |
| CIA-A PRA 02 (filter off) | +31472 |
| DMACON 800F | a table for `a` = 1..64 (≈ 1776 a² + 1781.5 a + 158587, ±1) |
| timer start | the first host 71051-cc slot (185688 + 71051 k) at least 5700 cc after |
| end of start (the song's start) | timer start + 21 or + 22 (a 64-entry table for `a` = 1..64, measured with built modules; the real modules agree) |

INTENA C080 (audio 0 interrupt enabled) is written but no audio interrupt changed the output.

## 10. Open

* Late divergences not explained: Tropical Islands from 249.9 s of 278.6 s (a quiet difference of a
  few units of the sum on both sides from mixer tick 12397 on, the reference about 1 lower on ~40% of
  samples; voice positions and source bytes are equal, shrinking every voice's volume by 1 helps but
  does not cure it, so the reference's volume scaling differs somewhere), A Sporting Chance from
  254.1 s of 277.6 s (alike). Later first differences also in Stormlord (359.2 s), Stormlord (synth)
  (318.6 s), Galaxy Man (327.2 s) and Robocop 3 (98.3 s), not analysed.
* Volumes above 64 (section 6).
