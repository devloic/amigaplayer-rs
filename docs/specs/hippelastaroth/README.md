# hippelastaroth

Behaviour specification of the replayer embedded in `astaroth.hip` (Jochen Hippel, "Astaroth"):
an early Hippel replayer that reads COSO-packed song data (two songs) with its sample table in a
third COSO block. See `SPEC.md`.

* Reference: the Eagleplayer "Hippel" (2.06) calling the module's own replayer on the host-68k
  build, write logs compared write by write (value, order, colour clock).
* Status: the spec's model reproduces every reference write, clock included, for subsongs 1-4
  over 180 s, two modified copies that take the remaining code paths, and a synthetic module.
* Reference logs, test vectors, the synthetic module and the model scripts:
  `/media/lolo/kingston2tb/claude-scratch/deliplayerrust/fixround/astaroth/spec/` (its
  README.md lists them).
* Reuse: the effects of `hippel.rs` `Tfmx` early builds, the COSO parsing of `Coso`, the write
  order of the middle builds without line waits; SPEC.md section 10 lists every difference.
