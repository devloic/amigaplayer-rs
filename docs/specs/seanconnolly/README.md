# Sean Connolly: unknowns and caveats

A private working folder with the reading notes, disassembly and
scripts exists beside these files and is not for the implementer.

Spec: SPEC.md. Logs and how they were made: LOGS.md. No `data/` folder was
produced: all tables the replay needs live in the module itself.

## What is solid

* The register values and write order of the four voices match the reference
  in every tick of the real module (7 subsongs) and the four synthetic
  variations.
* The wait behaviour (10 CIA-B TOD edges) and the 8200 write time are exact
  for the real module apart from 22 of 11659 ticks.

## Unknowns and caveats

1. Only one real module (Surf Ninjas, type 2 driver) exists in the corpus.
   The table offsets in SPEC.md are specific to it. Other type-2 modules should
   have the same tables at other offsets; the original wrapper finds them by
   scanning the driver code for address loads. How to derive those offsets
   from the driver code of another module is not verified here.
2. Types 1 and 3 of the driver (different generations) are detected but not
   specified (not available).
3. Parts of the format that Surf Ninjas never uses were checked with synthetic
   modules only (written by editing the real module, not by the original
   composer): rests, tie, slides, absolute pitch sequences, wave-offset token,
   fade, modes 2 and 3, noise waves, inactive-voice handling. Their timing
   costs are weaker.
4. The F cost model (SPEC.md 9.2) is a fit of many events to measured data and
   not true instruction timing; use it as a set. Costs of the noise
   regeneration depend on which voice triggers it and only voice 0 was
   separated. The inactive-voice DMACON write time is known for consecutive
   inactive voices only.
5. The noise seed is the beam-position word read at each init; it depends on
   host start timing. It affects audio only, not the register write log,
   because the noise data are fetched by DMA from module memory.
6. The reference host only emulated TOD as a scanline counter (LOGS.md). The
   original hardware TOD may differ; the replay timing is therefore
   host-defined.
7. The first four ticks of each run emit literal register-image bytes from the
   module file (LC not module-relative); a comparison must allow that.
8. Subsong lengths do not exist in the format; end detection is by voice 0
   track loop or all voices off. Title is the module's info text only.
9. The wrapper issues N+1 inits for subsong N; the extra stop groups in the
   logs before the first play tick are host behaviour.
