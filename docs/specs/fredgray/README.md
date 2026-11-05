# Fred Gray - caveats

- Modules used: the four real modules in the archive (Black Lamp, Star Goose,
  Victory Road, Eco), 20 subsongs in total. No synthetic modules.
- Register values and write order, including DMA pointer/length writes, were
  reproduced exactly by a private model for all subsongs over 400 s or until
  song end. Colour-clock timing is not register-exact: the cost model is
  standard cycles / 2 plus up to 25 colour clocks of unexplained jitter.
- The start phase (time to the first init and to the first tick) is given as a measured rule in SPEC section 12, residual at most 2 cc for the first init and about 5 cc for the first tick.
- Unknown: origin of that jitter; exactly when the host stops after song end
  (0-2 ticks after section 0 becomes current).
- The code that runs the sections lives in each module, so an implementer needs
  a small 68000-subset interpreter (SPEC section 7). A fifth module using the
  same driver is expected to work if its tables are located as in SPEC 2.6, but
  none was available to test.
- The wrapper's glide patch is reproduced (glide bit has no effect).
- No data/ files were produced; nothing needed from the player binary is
  absent from the modules. The lead decides on any embedding.
- Private analysis notes, scripts and the disassembly exist in a separate
  folder that the implementer must not open.
