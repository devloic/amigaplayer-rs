# Reproduction

The logs and scripts named here are not shipped. Reference binary: the non-native build of this repository
(`cargo build --release --no-default-features --features ui`). A run script of the form `ref.sh MODULE TAG SECS`
runs it under `env -i` with HOME=/nonexistent, DELIPLAYERS/DELIGENIES = an empty
directory, EAGLEPLAYERS = the Eagleplayer 2.06 `Eagleplayers` dir, `--player MusiclineEditor
--wav ... --seconds SECS`, and writes:
* `logs/TAG.ref.log`  (DELI_WRITE_LOG: `ADDR VALUE CLOCK`, CIA/custom writes; first line `# module`)
* `logs/TAG.dma.log`  (DELI_DMA_LOG: `CLOCK CHANNEL ADDRESS WORD`)

Logs of three modules (3 s each: m1, m51, m82 of the MusicLine module corpus) were provided: `./ref.sh .../1.ml m1 3`.

Claims and where to check:
* C1 clock/floor-per-stretch, C2 entry formula: first CIA-B timer-A latch
  write in `m*.ref.log` (the `bfd400`/`bfd500` pair) and the first tick's
  interrupt-ack write (`dff09c` value 0080 line) follow section 2; consecutive
  tick clocks differ by latch*5 + (jitter-free entry).
* C3 loader formulas: the clock of the first `bfd400` write = sum of section 3
  stretches (floor each). Re-derived for 165/165 four-voice modules with an
  instruction-trace tool; only the final clocks are in the public logs.
* C4 crunch counters: stream counters defined by walking the packed delta
  stream of the SMPL chunk; exact counter definitions were not written out (open item);
  expected sums are visible as the gap between loader chunks in the logs of
  modules whose SMPL is crunched.
* C5 write order (section 5): `dff0a0..dff0d8` lines in each tick of
  `m*.ref.log`, DMA-off/on via `dff096`.
* C6 idle tick = 10885 cyc: difference between two consecutive tick entries
  minus latch*5*... in a module with an empty song (idle ticks only).
Not re-derivable from logs alone (needs an instruction trace with a clock per instruction):
per-handler costs in section 4.
