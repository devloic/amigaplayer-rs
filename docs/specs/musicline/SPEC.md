# MusicLine Editor replayer: timing behaviour spec (clean-room, spec side)

> **Sanitised for publication.** This copy was reviewed before it was added to the repository. Removed or rewritten: scratch names and references to private material only; the spec text was already behaviour-only. Numbers, formulas and behaviours are unchanged. References to `logs/` files, scripts and traces name data that is not shipped (see `docs/specs/README.md`).

Scope: 4-voice modules (play mode 0) of the MusicLine Eagleplayer, as run by this
repo's own 68000 engine (reference = engine + EP206 `MusiclineEditor`, non-native
build `--no-default-features --features ui`). 8-voice modules (play mode 1) have a
different timer start and are NOT covered. No addresses, labels or instruction
sequences appear here by design. Every number below was measured on the reference
and is reproducible with the commands in LOGS.md. "cc" = engine clock unit used by
the write/DMA logs (colour clocks); "cyc" = 68000 cycles.

## 1. Clock model (exact)

1. The reference engine prices each 68000 instruction from a static table
   (e.g. Bcc taken 9 (word form 11), dbra 10, mulu 58, muls 54, divs 142).
2. Bus clock = `epoch_clk + floor((cyc - epoch_cyc)/2)`.
3. The epoch is re-based at every call into the host library (A-line trap).
   A host call costs 0 cyc. So each stretch of player code between two host calls
   contributes `floor(stretch_cyc/2)` cc; an odd stretch loses 0.5 cc.
   Loader time = sum over stretches of floor(stretch/2). An implementation that
   keeps one running cycle counter and halves it once is off by up to 0.5 cc per
   host call; it must floor per stretch.
4. Interrupt handlers run to completion. An interrupt that becomes due while a
   host call (or another handler) is executing is taken late, at its end.

## 2. CIA / interrupt entry (exact)

* CIA-B timer A is the tick; CIA-B timer B is the DMA-restart wait. Timers count
  E-clocks (1 E = 5 cc).
* Tick entry clock = `(floor(W/5) + latch + 1) * 5`, W = clock at which the timer
  was (re)started (the write that latches/starts it), latch = value written.
* First tick with no prior latch: `5*floor(W_start/5) + 327680`.
* Reference uses START_PHASE=100 at repo HEAD; the native-feature build lays
  memory out differently and a layout-dependent pointer check in the player then
  costs +15 cyc per tick. Calibrate against the non-native build only.
* Entry-latency jitter (about 42 cc alternation seen in earlier work) is NOT
  random: it comes from rule 1.4 (late take after a host call) and from the
  floor in 1.3. With exact stretch pricing it vanishes.

## 3. Loader and start-up (exact, validated 165/165 four-voice modules)

Module: header, then chunks TUNE / PART / ARPG / INST / SMPL / INFO (in file
order). Each chunk visit begins with a fixed 70-cyc stretch and is then cut into
stretches by the host calls (file reads, allocations). Measured stretch lists
(cyc), all validated against write-log clocks:

* Pre-chunk init: 24 fixed stretches (sum dominated by one 63609-cyc stretch:
  a fixed-size clear). Fixed, data-independent.
* TUNE: 70, 126, 145, then if the tune has 0 channels 366; else 166, then per
  non-empty channel-size entry `102` (first) or `119` (later), +76 per
  preceding empty entry, followed by a `2941` stretch (the per-channel copy);
  tail = 95 when all 8 channel slots are used, else `115 + 29*(8-nch)`, +76 per
  trailing empty entry.
* PART: 70, 165, 117, then `154 + 241*L + 28*W` where L = number of lines in the
  part and W = total number of set bits in the six-bit voice mask of each line
  (each set bit consumes one 2-byte entry).
* ARPG: 70, 196, 93, 177, 61 (constant).
* INST: 70, 227, 193, 41 (constant per instrument).
* INFO: 70, 317, 41.
* SMPL (i-th sample, ninst = instruments seen so far, match = instruments whose
  sample-number field equals i):
  - uncompressed: 70, 278, (164 wave | 148 sample), 89, 176, then final stretch
    `(364 sample | 15540 wave) + 58*ninst + 104*match`.
  - delta-crunched (stored size != original size): 70, 278, (164|148), 89, 160,
    356, a decode stretch, then final `(349 | 15525) + 58*ninst + 104*match`.
    Decode stretch = `237 + 64*L + 164*(E0+E1) + 83*H + 64*Lo + 8*(NH+NL) + 10*EV`
    where, walking the packed stream: L = literal bytes, E0/E1 = escape
    records with run count 0/1... (see LOGS.md claim C4 for the exact counters
    and the script that produced them), H/Lo = high/low nibble deltas emitted,
    NH/NL = nibbles with the sign bit set (sign-extension path), EV = records
    whose run count is even. A "wave" sample is one with original size 0x100.
* Start-clock prediction: first-tick clock from section 2 with
  `W_start` = clock at start = loader sum + init stretch; the init stretch is
  `372668` cyc plus the cost of the first sequencer row. Given the measured init
  stretch the prediction is exact on 165/165 modules.

## 4. Tick anatomy (what is exact, what is a range)

Order inside one tick (all handlers back-to-back, no host calls):
1. Entry; acknowledge write (INTREQ bit for CIA-B/level 6 equivalent, value $80
   to the interrupt-request register) at +50 cyc after entry.
2. Sequencer/row stage: 4 voice row steps (one per voice).
3. Per-voice effect pipeline. For voices whose restart bit is set the first
   group of 9 part-effect/slide handlers is skipped; then 7 handlers always
   run: vibrato/tremolo-type, envelope/loop, then the five instrument effects
   selected by bits of the effect byte: transform (bit0), phase (bit1), mix
   (bit2, contains an inner loop), resonance (bit3), filter (bit4).
4. Per-voice period computation.
5. Period and volume writes for voices that are NOT restarting.
6. DMA-off write for restarting voices plus the timer-B latch writes.
Then timer-B interrupts (one per wait) write the restarting voices' registers and
DMA-on, and later the loop-start/length registers.

Measured costs (cyc):
* An idle tick (no row change, no restart, no active instrument effects) is
  exactly 10885 cyc from entry to return. All per-tick offsets in the open
  port should be measured as deviation from this.
* Voice row step, no new note: 27 on the straight path; typical new-note paths
  54, 90..102, 114, 141/164, 150..166, 190, 236/267 (the data-dependent paths
  are selected by which note/instrument/command fields are non-zero).
* Per-voice effect handler entry+exit overhead: 25 cyc when the handler is
  disabled; an enabled handler adds a per-iteration cost (232..393 depending on
  parameter signature; exact per-signature table not derived, see README).
* Instrument start (restart) tail: about 1890..1940 cyc for a plain start and
  about 2190..2320 cyc when the start also initialises one or more effects.
* Retrigger pricing differs by instrument kind (wave vs sample): the wave
  path pays the extra effect initialisation; see unknowns.

## 5. Paula write order (exact within a tick)

Non-restarting voice: period write then volume write, in voice order 0..3, after
all effect handlers. Restarting voice: nothing in the tick except the common
DMA-off; the timer-B interrupt that follows writes, in order: pointer/length of
the new sample, period, volume, DMA-on; a later timer-B interrupt writes the
loop pointer and length (so the hardware latches the loop after the first
buffer fetch). The DMA-off write precedes the timer-B latch writes.

## 6. Validation method

All claims were checked against the reference using DELI_WRITE_LOG and
DELI_DMA_LOG with HOME and the player/genie dirs empty (isolated run), the
non-native binary, `env -i`. A full instruction trace (clock per instruction)
was used only to derive the numbers; the logs in `logs/` (not shipped) are enough to re-check
section 2 and section 5 and the first-tick clock of section 3.
