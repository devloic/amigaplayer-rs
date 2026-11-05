# Fred Gray - timing and reference logs

Logs in `logs/` come from the reference 68k player run through the host with
DELI_WRITE_LOG. Files: `<module>.<subsong>.log` (blacklamp 0-3, stargoose 0-4,
victoryroad 0-7, eco 0-3). Each holds the first 1500 ticks of the first run that
produces sound; columns are register address, value, colour clock. The
subsongs that are "empty" (blacklamp.1, stargoose.0, victoryroad.0, eco.0) hold
only the init and stop writes. Longer (400 s) runs were used for validation but
are not shipped.

## Global timing

- Tick period: exactly 70940 colour clocks (50 Hz).
- First write of a tick = tick start + a module-dependent constant plus the
  cost of the work done before it: the section routine, three channel stages.
  Measured first-write offsets: Black Lamp about 2600-3200 (varies with the
  step test), Star Goose about 4100, Victory Road about 4300, Eco about 6800
  (wave-buffer recompute adds ~650).
- Execution cost rule: each 68000 instruction's standard cycle count divided
  by 2 gives colour clocks; a write is stamped at the point it occurs. The
  residual against this rule is 12-25 colour clocks of jitter that I could not
  attribute (host stamping granularity). A replayer reaching register-exact
  output must accept this error or copy gaps from the tables below.
- Shifts cost 6 + 2n cycles; the divide used in the period stage uses the
  Cwik timing.

## Write gaps inside the output stage (colour clocks, constant per module)

Period writes ch0, ch1, ch2, then volume writes ch0, ch1, ch2 follow each other
at fixed gaps (Black Lamp: ~8 per register pair of long moves; see the logs,
which are the authority: take gaps from the first tick of any log).
Pointer high/low word writes of one long write share one colour clock.
Star Goose / Victory Road DMA switching: length write follows the pointer by
19 colour clocks; observed offsets from the first write of a tick: Star Goose
length 8 at +397, length 128 at +471/+496/+628/+629; Victory Road length 8 at
+459, length 128 at +533/+665/+666. Eco: channel-2 pointer/length writes occur
at the start of the next tick's sequencer (about +68.9k to +70.6k).

## Per-event costs (colour clocks per call, excluding callees; Black Lamp song 0)

channel stage entry 50; envelope step 36 (no change) / 139 (update); slide 28;
vibrato 76-96; instrument stage 37 idle, ~177 when loading; period stage
132-142 (depends on shift and divide); output stage 112; tail 51; sequencer
control 60; step counter 45/50; voice select 79; advance 62; play entry 29/77.
Star Goose: output stage 282 typical, envelope 38/160. Victory Road: output
stage 288/320/333/365. Eco: wave synth 653, output 131, channel entry 53.
These are approximate; every number was measured, not derived from a spec.
