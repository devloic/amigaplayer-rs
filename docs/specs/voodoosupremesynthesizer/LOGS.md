# Reference write logs: Voodoo Supreme Synthesizer

## How they were made

Each log is the output of the reference 68k player ("VSS", run through the host with DELI_WRITE_LOG
set), silent (dummy audio driver, WAV output only), subsong 0 of the file given. Each run lasts until
the song end (SPEC 7.3), where the reference host stops: the last tick of a log is the song-end tick.

## Format

Text, one line per write to a custom-chip register; the first line is `# module <load address hex>
<size>`. Other lines: `<register address hex> <value hex, 16 bit> <colour clock>`.

- Registers: 0xDFF0A0 + 16 x voice + 0, 2, 4, 6, 8 (LC high, LC low, LEN, PER, VOL), 0xDFF096 DMACON,
  0xDFF09A INTENA (once, at start), 0xDFF09C INTREQ (one per audio interrupt, SPEC 6.4).
- An AUDxLC write is two lines with the same clock (high word first).
- The first lines are the start-up writes (SPEC 2), then the four start-up interrupts, then the ticks
  with the audio interrupts in between.
- AUDxLC values that point at the synth buffers or the silent area are the player's own memory; they
  differ from a port's by one constant. Sample block addresses (mode D) are inside the module.

## Files

| log | module | ticks | lines | of which INTREQ |
|---|---|---|---|---|
| Qang.log | Qang (4 subsongs), subsong 0 | 6 020 | 246 804 | 126 386 |
| Qang_s1.log | the same module with subsong 1's song table offset copied into slot 0 | 2 436 | 97 811 | 37 850 |
| Qang_s2.log | same, subsong 2 | 768 | 33 150 | 17 772 |
| Qang_s3.log | same, subsong 3 | 259 | 11 285 | 6 087 |
| Silkworm.log | Silkworm | 6 914 | 258 643 | 95 427 |
| Voo_4.log .. Voo_8.log | Voo 4 .. Voo 8 | 5 506, 12 210, 4 997, 6 879, 6 290 | 224 212, 592 386, 223 738, 296 210, 285 276 | |
| syn1.log, syn2.log, syn3.log | modules/syn1.vss .. syn3.vss (synthetic, 30 s, no song end) | 1 497 each | 32 769, 29 709, 38 030 | |

The real modules are third-party data and are not included. The synthetic modules were written for this
spec: syn1 exercises mode B with copy and wrapping windows, mode C with all target and phase variants,
mode D with and without pitch correction and the return to synth mode, pitch commands 0xFE, 0x7F, 0x7E
in both directions with octave shifts, portamento up and down, calls, loops, transpose, retrigger flags
0x20, 0x40, 0x80, the OR mask on negative mode A results, rests; syn2 mode B/C extremes (no-phase mode C,
wave 1 target, full-width windows) and pitch correction; syn3 the 16-bit overflow of the 0x7E ratio
division and shifts of 4 octaves.

What the real modules cover: modes A, B (one module), C and D (five modules, including sample end and
the "ended" state), pitch command 0x7E (two modules), portamento, all commands, the song end, voices
that never select an envelope (SPEC 5).

## Validation

Two models written from SPEC.md (one by a separate agent from the spec alone) reproduce every write of all 13 runs: tick writes in address,
value, order and exact time relative to the tick call, and audio-interrupt writes in value, order and
time relative to the handler call (52 279 + 4 491 ticks, 1.09 million interrupts). The tick-call times
themselves (frame start delayed by a running interrupt handler) and the interrupt times come from the
host's Paula and were taken from the reference trace, not modelled.

`tools/replay-cmp.py` groups writes into ticks by gaps; here the INTREQ writes of the audio interrupts
fill the gaps between ticks, so its "ticks" are long stretches of the log. Its comparison still holds
(writes in order, addresses made comparable), but its tick numbers are not frame numbers.
