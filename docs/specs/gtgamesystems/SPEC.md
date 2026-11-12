# G&T Game Systems (`DUX.*`, `*.dux`): behaviour spec

Scope: the music format of G&T Game Systems (Thomas Sutor and Gerald B…hner, 1990), as played by the
Wanted Team Eagleplayer "G&T Game Systems player module V1.0 (8 Aug 2008)" (prefix `DUX.`). Known
modules: the seven `carver {1-6,end}.dux` of modland "GT Game Systems/_unknown/" (38-48 KB each).

Method: black box only. The reference is that Eagleplayer (SOAMC archive copy, file
`G&T Game Systems`, 3236 bytes) on this project's 68000 engine, alone in its player directory and named
with `--player "G&T Game Systems"`. Observed: the Paula register writes with their colour clocks
(`DELI_WRITE_LOG`), the audio words Paula fetched (`DELI_DMA_LOG`), and in a private engine copy (never
committed) the audio.device requests the player sends with their fields and colour clocks, the times of
the player's calls (check, init, interrupt) and of its library calls, the module bytes it reads and
when (a read watch), and the output of built modules (the layout of section 2 with edited records,
sample entries and offsets: differential probing, 29 probe modules and 14 check probes). Neither the
player nor any source of it was disassembled or read; no code or data of the player is in this text.

Times are colour clocks ("cc", 3 546 895 a second) or 68000 cycles ("cy", two to the cc).

## 1. Reference

| item | finding |
|---|---|
| player | an Eagleplayer that plays through audio.device: it opens the device three times (one IORequest per voice, units 1, 2, 4 = Paula channels 0-2) and sends `CMD_FLUSH` and `CMD_WRITE` with `ADIOF_PERVOL`; it never pokes Paula itself |
| timer | DeliTracker's default timer: a tick every 70 940 cc (timer value 14187) |
| subsongs | one (0..0) |
| song end | reported once by each voice that reaches its end (section 5); the player plays on |
| output | 3 voices, channels 0, 1, 2; the engine's output stage (A500 filter, LED as left at start) |

What reaches Paula is what this engine's audio.device emulation makes of those requests (section 8);
that, not a real Amiga's device, is the reference for the writes.

## 2. Module layout

All values big-endian.

| offset | size | meaning |
|---|---|---|
| 0 | long | module size S (the file may be longer; nothing past S is used) |
| 4 + 12·i, i = 0..7 | 12 bytes | sample entry i: offset (long, from the sample data start), length in bytes (word), volume (word), loop start (word), loop length (word) |
| 0x64, 0x68, 0x6C | longs | start of voice 0, 1, 2 (from the sample data start) |
| 0x70 | | sample data (signed bytes), then the voices |

An unused entry is all zero. In every module the entries follow each other (offset of i+1 = offset of
i + length of i) and the voices follow the samples; the player does not need either.

A voice is a list of 6-byte records `period, sample, duration` (words), closed by the end record
`FFFF FFFF FFFF`; the next voice starts right after it, and voice 2's end record ends at S.

| record | meaning |
|---|---|
| duration FFFF | end of the voice (period and sample not read) |
| period FFFF | rest for `duration` ticks (sample not read) |
| otherwise | note: `period` (written to Paula as it is), `sample` (entry number), `duration` ticks |

In the files: periods 113 to 856, durations 6 to 960 (multiples of 6), samples 0-7, rests only as a
voice's first record, one looped sample (carver 1, sample 1: loop start 0x2CA, length 0x20A2); every
other entry has loop start 0 and loop length 2. All volumes are 64. All three voices of a module last
the same number of ticks (4992 to 8448).

## 3. Check (what the reference accepts)

Reads: the long at 0; the three words at S−6; the 8 sample offsets; the 3 voice offsets; the end record
before voice 2's start and before voice 1's start. Probes (built modules) show:

- S must not exceed the file's length (S + 2 refused); bytes after S are allowed (6 bytes of FF or of
  zero after a correct S accepted); S − 2 refused (the record at S − 6 is then not FFFF FFFF FFFF);
- the record before voice 1 and before voice 2, and the last record (at S − 6), must be FFFF FFFF FFFF
  (a period or a duration of 0 in any of them refused);
- a sample offset of 0x100000 (past S) refused; an odd sample offset accepted;
- voice 0's start is not checked (moved by 2: accepted); a voice 0 of only its end record accepted;
- voice 1's start moved back by 6: refused.

## 4. Start (init)

- Every voice's tick counter is 1 and its record pointer at its start, so the first tick reads the first
  record of every voice.
- The init reads the 8 entries' lengths, then voice 0's durations up to and including its end record
  (a song length; nothing else follows from it in the output), then the header again.

Song start: with the check called at clock c0 (the engine's start), the first tick is at

    c0 + 413 + ceil((12883 + 43·n0 + 8·nz) / 2) + 1622 + 70940

where n0 = voice 0's records up to and including its end record and nz = the entries with a non-zero
length. (413 cc check, the init's cycles as counted, 1622 cc of the three DeliTracker calls after it.)
Fitted on the 7 modules and checked on 9 probe modules: equal on all.

## 5. Tick

For each voice 0, 1, 2 (the "prepare" pass): the counter is decremented (16 bits); unless it is now 0
nothing more happens. Otherwise the record at the pointer is read:

1. duration FFFF: the song end is reported (one report per voice that ends), the pointer goes back to
   the voice's start, and its first record is taken **without** this test (a voice whose first record
   is the end record then rests for 65 535 ticks);
2. period FFFF: a rest: `AbortIO` on the voice's request; counter = duration; pointer + 6;
3. a note: the voice's request is filled: data = sample data start + entry offset, length = entry
   length sign-extended to a long (0x9000 gives 0xFFFF9000), period, volume = entry volume AND 0x3FF
   (0x8040 gives 64, 0xFFFF gives 1023, 0x100 gives 256), cycles 1; counter = duration; pointer + 6.

A duration of 0 is 65 536 ticks (the counter wraps); 1 is the next tick.

Then the "issue" pass, voices 0, 1, 2: for each voice that took a note, `CMD_FLUSH`, then `CMD_WRITE`.

Then the "loop" pass, voices 0, 1, 2: for each voice that took a note whose entry has a loop start other
than 0, the request is changed to data = (sample start) + loop start (sign-extended word), length = loop
length (sign-extended word), cycles 0 (for ever), and sent again with `CMD_WRITE`. The loop length is not
tested: entries (0, 4), (0, 0), (0, 1), (0, 3), (0, 1000) send no second write; (2, 2), (4, 0), (1, 2),
(998, 2), (1, 3), (FFFF, 2), (500, FFF0), (8000, 10) do.

Sample numbers 8 and above (none in the files) read the player's own memory after its copy of the
table: entry 8 gave data = start + voice 0's offset, length 0, volume 150; others data 0 or garbage
with length 0, volumes changing from run to run. Not reproducible.

## 6. Times of a tick

The time the player's code takes is a sum of fixed costs along the path, in 68000 cycles as the engine
counts them; the clock of each library call (`BeginIO`, `AbortIO`, the song-end report) is the cycles
since the previous one halved and rounded down; the tick's end likewise. From the tick's start:

| part | cycles |
|---|---|
| entry | 786 |
| prepare, voice not due | 458 |
| prepare, note | 2458 (voice 0), 2498 (voice 1), 2548 (voice 2) |
| prepare, rest | 1070, `AbortIO`, 918 |
| prepare, end record | 150, song-end report, then the first record's path with 48 (note) or 46 (rest) cycles more (the `AbortIO` of a rest 1116 cycles after the report) |
| issue, a note | 546, `CMD_FLUSH`, 484, `CMD_WRITE`, 348 |
| issue, no note | 402 |
| loop, a note with a loop | 852, `CMD_WRITE`, 285 |
| loop, a note without | 473 |
| loop, no note | 424 |
| exit | 74 |

The note cost and nothing else depends on the voice; none depends on the record's values (periods,
samples, volumes, durations, probes with values from 0 to FFFF). Checked on every tick of the 7
modules (200 s each, 52 500 ticks) and of the 29 probes: every call and the tick's end at the measured
clock. An idle tick takes 2356 cc; three notes 6958 cc.

## 7. The engine's audio.device, as it treats these requests

Each voice has one request, sent again for every write: the device reads its fields when it uses them,
not when it got them. Per channel the device keeps the request playing, the one latched in the
registers, and a queue.

- `CMD_FLUSH`: DMA off (DMACON write), the channel's state cleared.
- `CMD_WRITE`: channel idle (nothing playing or latched): LCH, LCL, LEN (= length / 2) written, DMA off,
  DMA on; channel playing but nothing latched: LCH, LCL, LEN written; otherwise queued.
- `AbortIO`: if the channel holds the request, the queue is emptied, a latched copy dropped, and if it
  was playing, playing ends and DMA goes off (a request latched but not yet started is dropped without a
  DMA write).
- Service: at the end of every output sample, at every frame (vertical blank, every 71 051 cc), and after
  every tick's code, the device looks at each channel whose interrupt request bit is set while it plays
  or has a latched request: it clears the bit, and if a request is latched it starts (when it is not the
  playing one: PER and VOL written, its pass count 0), counts a pass, and after `cycles` passes (0: never)
  takes the next queued request into the registers; with nothing latched it turns DMA off and clears the
  bit again. Interrupt requests raised since Paula was last brought up to date are not seen: after the
  tick's code Paula has only run to the tick's last register write.

Consequence (a defect of the engine's device, not of the player): an interrupt request a stopped channel
raised earlier stays set; the next note's `CMD_WRITE` (DMA off and on in one colour clock) is taken at
the tick's end as the start of its pass, the real start (Paula's interrupt at the next line) as the end
of its only pass, and DMA goes off a few colour clocks later: from a channel's third note on, most notes
play 3 audio words. The reference's output (and so the port's) sounds of short clicks on channels that
play notes back to back. See section 10.

## 8. Write order

A tick's register writes are the device's: per note `DMACON` off (flush) at the flush's clock, then
`AUDxLCH`, `AUDxLCL`, `AUDxLEN`, `DMACON` off, `DMACON` on at the write's clock (or the pointer and
length alone, section 7), a loop write's pointer and length (when latched at once); `AUDxPER`, `AUDxVOL`
when a pass starts at a service; `DMACON` off when a channel ends.

## 9. Unobserved / not established

- Sample numbers 8 and above (section 5): the player's memory.
- Lengths and offsets that point outside the module: the reference plays what lies there.
- What the init's song length (section 4) is used for: nothing in the writes.
- The engine's other service points (CIA interrupts) are not used by this player.

## 10. Check of the port against the reference

`src/replay/gtgamesystems.rs` follows this text and models section 7 itself. The reference's audio is
what the engine's device makes of the requests; if the engine's device is changed, the port's model
(and this section 7) must change with it.

Results (write logs of the reference and of the port, same build, module base 0x45e0 on both):

| module | Paula writes | song end | result |
|---|---|---|---|
| carver 1.dux | 10323 | 146.0 s | every write equal in value, order and colour clock |
| carver 2.dux | 15060 | 115.3 s | equal |
| carver 3.dux | 14239 | 115.3 s | equal |
| carver 4.dux | 11939 | 130.6 s | equal |
| carver 5.dux | 18761 | 169.0 s | equal |
| carver 6.dux | 15204 | 123.0 s | equal |
| carver end.dux | 6841 | 99.9 s | equal |

WAV and DMA logs are bit-identical too. With the song end ignored on both sides, the 7 modules also
stay equal for 360 s past it (257,527 writes). Of 29 synthetic probe modules, 27 are equal over 8 s.
The two that differ use sample numbers 8 and up (section 5), where the reference reads player
memory.
