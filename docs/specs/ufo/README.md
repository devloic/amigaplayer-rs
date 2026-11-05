# UFO (MicroProse 1994) spec: caveats

- No real module was available; all 222 modules in mods/ are synthetic, generated against the player's own
  parsing. Chunk layouts (CHAN/BLCK/INST/BPM/DRUM) come from the player's parsing and from synthetic
  confirmation; real game files may contain chunk kinds or lengths not exercised here.
- Values and write order are reproduced exactly by a private model built from SPEC.md on 221 of 222
  modules. The failing one has tracks running off the end of the data (outside the format).
- Clock exactness is NOT reached: about half of all writes land exactly, the rest are within a few cc or one
  wait-loop iteration (about 12 cc). See SPEC section 11.
- Not determined: behaviour after the restart event beyond a few ticks (the host stops at the first end
  signal); period values for notes above 107 (data/ufo_oob_periods.bin holds what the reference reads
  from surrounding memory; the lead decides on embedding); the sample bank format was only tested with synthetic banks.
- No title, author, subsong or length information exists in the format.
- data/ufo_oob_periods.bin: out-of-table words, optional.
- A private working folder exists for the spec author's notes; implementers do not open it.
