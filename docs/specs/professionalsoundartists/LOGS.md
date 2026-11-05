# Reference write logs: Professional Sound Artists

## How they were made

Each log is the output of the reference 68k player run through the host with DELI_WRITE_LOG set, silent
(dummy audio driver, WAV output only), one subsong per run. The run ends at the first song-end tick that
stops the song (restart 0xFFFF), or after a fixed number of seconds.

## Format

Text, one line per Paula write; the first line is a comment `# module <load address hex> <size>`.
Other lines: `<register address hex> <value hex, 16 bit> <clock>`.

- The register address is the full hardware address (0xDFF0A0 + 16 x voice + 0, 2, 4, 6, 8 for LC hi, LC lo,
  LEN, PER, VOL; 0xDFF096 for DMACON). An AUDxLC write is two lines with the same clock (high word first).
- The clock is in colour clocks counted from the start of the run. The first lines are the host start-up
  (volume calls and DMACON 0x000F with four volume writes of 0, SPEC section 8); the first tick follows.
- The ticks are 70 940 cc apart (SPEC section 7).

## Files

logs/ (real modules; the modules are third-party and are not shipped)

| file | content | ticks |
|---|---|---|
| title.log | title music, subsong 0 | 1499 |
| test.sub0.log | a second game module, subsong 0 | 1499 |
| test.sub1.log | same module, subsong 1 (ends with the stop sequence) | 764 |
| punisher2.log | a third module, subsong 0 | 1499 |

logs/syn_*.log with modules/syn_*.psa: synthetic modules written for this spec to reach paths the real
modules do not (synth instruments and sweeps, envelopes with all phases, volume slides with clamping,
tone portamento, pattern break, speed changes, song end with and without restart, out-of-range
instrument numbers). The name `syn_fN.sK.log` is module N, subsong K. These are 8 of the 117 synthetic runs
made; the generator is not shipped.

## Validation

A private model written from SPEC.md (semantics of sections 1 to 6, costs of sections 7 and 8) was run
against all 121 reference runs (4 real, 117 synthetic; 108 909 ticks): every register write matches in
address, value, order and clock. 0 mismatching ticks. See README.md for the limits of this check.
