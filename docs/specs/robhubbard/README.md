# Rob Hubbard: unknowns and caveats

- Absolute phase: the clock of the first tick (140 000 to 250 000 cc after reset) and the 256-cycle
  entry wrapper cost are host properties. Only tick-relative timing is verified.
- Not covered: "Rob Hubbard 2" (`rh2.*.dum` + `rh.ins`) is a different file layout and was not
  analysed. A fourth 1991 code generation (byte volume override, known from the Eagleplayer) has no
  module in the corpus; its details are unknown.
- Paths never executed by any reference run, real or synthetic: the stop command 0x85 (the host
  rewrites it to 0x84 so it is unreachable); the generation B rest command is covered by the
  synthetic edit only. Zero-length events (counter wraps to 65535), note indices above 48 (reads
  past the period table), and a vibrato divisor producing a divide overflow are specified from the
  arithmetic but not logged.
- The no-instrument start (a note before any instrument select) reads memory beyond the host's
  8-byte zero block; the exact value is not reproducible and not specified.
- Host mute, volume and balance settings are ignored; logs use defaults.
- The format has no title or author. "Length" in the host's info is the largest pattern count of a
  subsong and cannot be turned into time; the song-end event (all four voices wrapped) is the only
  loop point.
- Layout finding (SPEC 1.3) was checked on the five corpus modules only; modules with a different
  init-code shape need manual inspection.
- Detection (SPEC 1.2) accepts the five corpus files; other compilers of the same driver may
  differ in header words.
- Waveform modulation changes sample memory permanently and in place, so replay after a restart in
  the same process differs unless the module is reloaded.
- Generation D (SPEC 1.8, `sanxion-demo.rh`): one module, one subsong. Its slot timing is a fit to the
  reference's logs, not a cycle model (a write can be one colour clock off); paths seen only once (the
  song-end wrap slots) are off by up to 6. Commands 0x83 and unknown ones, a second subsong, instrument
  numbers past the bank and the song-end loop have no reference run. `skateordie.rh` (demozoo) has the
  same header words but the 0x26-byte voice record and 32-byte instrument records with synth waveforms;
  it is read as generation B and its logs differ from the reference from tick 3456 on (a loop address
  0x20 off), so it is a further variant, not covered.
- Cycle tables in SPEC section 8 are the reference machine's static costs; they reproduce the logs
  exactly but a sample-accurate port also needs the host's Paula DMA timing, which this spec does
  not cover.
