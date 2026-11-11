# AProSys: reference logs

All logs are register-write logs (`DELI_WRITE_LOG`: lines `ADDRESS VALUE COLOUR-CLOCK`, first line
`# module BASE SIZE`) of the reference 68k player "AProSys" (UADE player file `AProSys`, the
registered name is the same) run through the host with DELI_WRITE_LOG, rendered offline
(`--wav`, `SDL_AUDIO_DRIVER=dummy`) for a whole number of seconds covering the song end plus at
least 8 ticks. `logs/runref.sh` shows the command (set `REF_BIN`, `EAGLEPLAYERS`, `EMPTY`).

**No real AProSys module was available** (the corpus index has no file of the format), so every
module is synthetic, built from the format as described in SPEC.md. They are in `modules/`:
`aps.py` (record builders), `gen.py` (seeded random modules: `gen.py SEED OUT [mix|synth|sample]`),
and `t1.py` .. `t4.py` (hand-made tests). The `r*` modules were made by an earlier version of
`gen.py` (shorter songs, fewer instruments); use the `.aps` files as given.

## Modules

| log | module | ticks to song end | what it exercises |
|---|---|---|---|
| `t1` | hand-made | 48 | one looping sample voice, 3 silent voices, song end and what follows |
| `t2` | hand-made | 24 | chain (type 2), one-shot (type 1), loop (type 0) on four voices; host volume per channel |
| `t3` | hand-made | 1 | pitch formula terms one at a time (depth 1, depth 2, both, detune) |
| `t4` | hand-made | 42 | filter stage with bit 23 and divide overflow (447 overflows), jump budget exhausted in an envelope, mode 1 with a fresh-only source, sequence entry with count 0, zero speed multiplier, 0-row block, loop entry, envelope numbers 4-7 (next voice), period indices 47, 50, 53, muted block voice |
| `r1mix`..`r3mix` | `gen.py` (old) | 156 / 137 / 175 | sample and synth instruments mixed |
| `r1synth`..`r3synth` | `gen.py` (old) | 156 / 56 / 175 | synth instruments only |
| `r1sample`..`r3sample` | `gen.py` (old) | 75 / 96 / 127 | sample instruments only (types 0, 1, 2, 4, 5, 13, chains into type 4) |
| `g11`..`g14` | `gen.py` seeds 11-14 `mix` | 886 / 290 / 306 / 501 | long songs, 6-12 instruments, portamento speeds up to 60, volume slides 1-3 |
| `g15`..`g18` | `gen.py` seeds 15-18 `synth` | 196 / 552 / 324 / 929 | long songs, synth only |

Every synthesis mode and defined operation (mode 0 ops 0-2, mode 1 ops 0-4, mode 2 ops 1-7 and 9,
mode 3), comb and filter stages (both filter variants, the state reset), envelope jumps, arpeggio
jumps, release with and without instrument change, both event-offset modes, the row-length
override, all sequence commands and interrupt-handler runs for types 0, 1, 2, 4 and 13 occur in
these runs.

## What they confirm

- A private model written from SPEC.md reproduces every log write for write (address, value,
  order) from the start to 3 ticks after the song end: 74 000 writes and 678 interrupt-handler
  runs over the 21 logs, 0 differences. Sample addresses compare as module offsets; synthesis
  buffer addresses compare by one constant per run (the area's base), the silence buffer by
  another. Interrupt-handler runs were placed where the reference has them (their moment is
  Paula's business, SPEC §10); their writes were compared.
- A second, independent model written by another agent from SPEC.md alone (no access to the
  original) also matches all 21 logs, 0 differences; it found one typo in the period table
  (index 21 is 508), since corrected, and asked for the clarifications now in §4, §5.3, §6.1 and
  §10.
- The synthesized buffer bytes: DMA logs (`DELI_DMA_LOG`) of r1synth, r3mix and g11-g18 (40 s
  runs): 216 000 audio DMA words fetched from the synthesis area equal the model's buffers, 0
  differences (fetches during a tick handler skipped). In these runs the area was initially zero.
- Host volume: AUDxVOL equals `b & 127` on all four channels (host per-channel volume 64).
- Start-up does not depend on the module's path: the same module loaded from an 8-character and
  a 95-character path gives identical logs (times included).

## Timing (colour clocks)

- **Tick**: 70 940 cc apart (DeliTracker's default 50 Hz timer). A tick whose handler is still
  running when the next one is due (heavy synthesis: up to about 77 000 cc in `g13`) delays the
  next tick, which then runs immediately after; no tick is lost and the following ticks are back
  on the 70 940 grid.
- **Start**: the first write lands 2 956 cc after the log origin when the sequence begins with a
  filter command (the CIA write), then DMACON `0x8200` at 4 530; without a filter command
  DMACON `0x8200` is at 4 152. The first tick's first write (PER0) is at 78 403 cc in `t1`
  (voice 0 without envelopes or arpeggio) and up to about 84 500 with more work before it.
- **Voice update** (from one write to the next):
  - PER(v) to VOL(v): 444, 471, 498, 497 cc for voices 0-3 (constant).
  - VOL(v) to PER(v+1) with no sample action: 986 cc typical (envelope/arpeggio work adds,
    1 020-1 314 seen); VOL(v) to VOL(v+1) when voice v+1 has channel volume 0: 348 / 219 / 215 cc.
  - VOL(v) to LOC(v): type 4: 492 cc; type 5: 382-383 cc; synthesis: see below. LOC high and low
    words are written at the same clock. LOC(v) to PER(v+1): about 940-1 100 cc.
  - Synthesis, VOL to LOC (mode/op, comb, filter, n bytes, typical): mode 3 nothing: 345;
    m2 op5 n8: 1 103; m2 op6 n16: 1 285, n64: 1 934; m1 op3 n8: 1 041; m0 op2 n32: 1 880;
    m2 op2 n48: 3 384; m0 op1 + filter n4: 2 192; m0 op2 + filter n8: 2 955, n32: 8 825;
    m2 op4 + filter n32: 8 180; m1 op4 + filter n48: 15 922; m0 op0 + comb n4: 1 926, n32: 7 764;
    m1 op0 + comb n32: 8 187; m2 op1 + comb n48: 12 663; m1 op3 + comb + filter n16: 8 503,
    n32: 14 813; m2 op2 + comb + filter n48: 23 402; m2 op4 + comb + filter n64: 29 016;
    m2 op9 + comb + filter n64: 29 342; m1 op7 + comb + filter n64: 26 620. The cost grows about
    linearly with `n` for each configuration and varies by a few dozen cc with the data
    (divisions).
- **Row** (after the last voice-update write): DMACON `0x8200` about 580 cc after the last
  write; per voice: type 3/4/5: LEN, LOC (+125..165), INTENA (+100..200), DMACON (+25);
  keyed voice: DMACON off; then about 1 170-1 240 cc to the next voice's first write. Keyed
  voices' INTENA enable and AUDxDAT are 31 cc apart.
- **Interrupt runs** after a tick: the first handler write (INTENA disable) about 330-450 cc after
  the last AUDxDAT write; LOC +115..235, LEN +31..110, DMACON +95..110, INTENA enable +32.
- **DMA off to on** of a keyed voice (row-time DMACON off to the handler's DMACON on): 1 067 to
  about 3 100 cc, longer the more voices follow it in the row.

## Timing, exact (68000 cycles)

Measured later on the reference's own cycle counter (one colour clock is two cycles), on the clocks
at which its guest calls start and return, and on its audio DMA fetches; 615 modules (the 21 above,
234 probes, 360 random ones). A port that follows these rules writes every register at the reference's
cycle. Each rule is a count per piece of work; none is per module.

- **Calls.** The start-up, each tick and each audio interrupt handler is one call. A call starts on a
  whole colour clock and returns on one (an odd cycle of its end is dropped). The song starts where the
  start-up call returns: the host's output sample grid and DMA line grid restart there. The first tick
  is called 70 940 cc after that and then every 70 940 cc, or, when a call is still running, when it
  returns. An audio interrupt requested while a call runs is taken when that call returns (channel 0
  first); otherwise the host takes it at the end of the output sample in which it was requested (or
  at once in a host step where a vertical blank was already due). So the time a handler starts depends
  on where the calls before it started and ended, not only on the writes.
- **Tick.** 116 cycles from the call to the voices' work. Then per voice, counted from the previous
  voice's end: a silent voice's VOL after 672 / 695 / 718 / 708 cycles (voices 0-3); a playing voice's
  PER after 1 765 plus the portamento step (up 123, up and reached 120, down 123, down and reached
  173, already there 56), an arpeggio row 9, each arpeggio byte 81, arpeggio jump 73, envelope step
  69, envelope point loaded 435 (envelope 1's 549), envelope jump 222 (envelope 1's 208). These hold
  after a plain VOL (and at the tick's start); after a voice that ended with a LOC the voice comes 88 /
  57 / 26 cycles sooner (voices 1-3; 2 more after a moving sample's), after a silent voice 279 sooner.
  PER to VOL 888 / 942 / 996 / 994. VOL to LOC: type 4 984, type 5 765, synthesis as below.
- **Abandoned update** (§6.2): an envelope that runs out of jumps ends the voice 500 cycles after the
  point where the PER count starts (29 more for each envelope after envelope 0), plus the portamento,
  arpeggio and envelope work done; an arpeggio that runs out ends all the voices 78 cycles after it,
  plus that work. What follows comes as after a plain VOL.
- **Synthesis** (VOL to LOC): every stage is a constant plus a count per byte and the stages add up
  (see `synth_cost_h` in the port for the table: generator or mixer by mode and operation, source
  record types 4/5/1, comb 924 + 417 n, the two filter variants); a length code of 0 (128 bytes) costs
  8 more.
- **Row.** DMACON `0x8200` 1 164 cycles after a LOC (2 less after a moving sample's), 1 182 after a
  plain VOL, 903 after a silent voice's VOL. Then each voice costs, whatever its number: idle 433, note
  1 454, release 1 341 (instrument change +545, with the record-relative bit +10, counter zero -244),
  arpeggio bit +68, row byte 2 bit 7 +10, bit 6 +14, retrigger +905 (first row of a block +115, with
  the block's envelope-3 bit +174), block mute on a row with a note +39, channel-volume update: set 0,
  up 119, up and clamped 116, down 119, down and clamped 167, already there 54. A write comes 70 cycles
  (after the row start) or 86 (after another write of the row) plus the work of the voices since; a
  LEN write of type 3 / 4 / 5 comes 95 / 76 / 49 sooner.
- **Call ends.** A tick returns after its last write: AUDxDAT of a key tail 637 / 567 / 497 / 427
  (channels 0-3), LOC 744 (moving sample 742), plain VOL 762, silent VOL 483, a row's sample or synth
  start 766 plus the work of the voices after it, a row with no write but its start 750 plus that
  work, the song end's DMACON 232. The start-up call returns 111 cycles sooner than a tick would
  after the same row. A handler returns 176 cycles after its last write.
- **Memory.** The 2-byte silence buffer is allocated first and the 1024-byte synthesis area after it
  (8 bytes higher in the reference host). A moving sample start that an envelope takes out of the
  module reads host memory: below the module the reference host keeps, among other things, the
  module's path, so the sound (not the register writes) of such a module depends on the path it was
  loaded from.
