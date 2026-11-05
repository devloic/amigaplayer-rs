# Professional Sound Artists: unknowns and caveats

- Validation independence: the model that reproduces the logs was written by the same agent that wrote the
  spec, in a second pass from the spec text. That pass found one spec error (the effect checks are executed
  for a period-only entry; they cost time but cannot match) which is fixed. It is not a clean-room
  test by a different person.
- Real coverage is 3 modules / 4 subsongs. Everything else (synth sweeps, envelope phases, volume-slide
  clamp, portamento, pattern break, restart/stop at song end, subsong switching) was exercised with
  synthetic modules only; the reference player run on them is the truth, so the behaviour is right, but
  no real module is known to use some of those paths.
- The three real modules are third-party data and are not included, only their logs.
- The first-tick time (SPEC section 8) and the host-call costs are host behaviour (the harness's cycle
  accounting), not part of the original format. A player that only needs correct sound can ignore them;
  they are needed only to reproduce write clocks exactly.
- The run ends at the first stop at song end; what the reference does after that is not covered.
- Out-of-range reads (instrument number beyond the table, loop offsets outside the module) read whatever
  follows in the module image; one synthetic module covers a case, the general rule is "plain memory
  read" (SPEC section 9).
- Subsong switching does not clear voice state in the reference; the logs start every subsong fresh.
- data/psa_synth_wave.bin holds the 516 bytes of the synth waveform memory (player data); the lead decides
  on embedding it.
- Working notes, scripts and the validation model are in a private folder next to this file.
