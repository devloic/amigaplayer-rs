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
